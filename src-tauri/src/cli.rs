use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Select};
use connection_app_lib::aws::credentials::create_aws_clients;
use connection_app_lib::aws::operations;
use connection_app_lib::aws::sso::{ensure_sso_session, CliSsoHandler};
use connection_app_lib::config::aws_config::read_aws_profile_names;
use connection_app_lib::config::projects::{
    get_default_port_for_engine, get_local_port, get_profiles_for_project, load_project_configs,
    ProjectConfig,
};
use connection_app_lib::tunnel::native::start_native_port_forwarding;
use std::collections::HashMap;

#[allow(unused_imports)]
use connection_app_lib::aws::operations::{
    find_bastion_instance, find_ec2_instance, find_ecs_task_ip,
    start_direct_port_forwarding_session, start_session,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "connection-app", about = "ConnectionApp — Secure tunneling via AWS SSM")]
#[command(version)]
struct Cli {
    /// Project name (skip interactive selection)
    #[arg(short, long)]
    project: Option<String>,

    /// AWS profile name (skip interactive selection)
    #[arg(long)]
    profile: Option<String>,

    /// Local port override
    #[arg(long)]
    port: Option<String>,

    /// Enable debug logging (RUST_LOG levels)
    #[arg(long)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List configured projects
    #[command(name = "projects")]
    Projects,

    /// List AWS profiles
    #[command(name = "profiles")]
    Profiles,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.debug {
        env_logger::Builder::new()
            .filter_level(log::LevelFilter::Debug)
            .init();
    }

    // Migrate legacy ~/.rds-ssm-connect/ → ~/.connection-app/
    connection_app_lib::config::projects::migrate_legacy_config();

    if let Some(command) = &cli.command {
        match command {
            Commands::Projects => {
                run_list_projects().await;
            }
            Commands::Profiles => {
                run_list_profiles().await;
            }
        }
        return;
    }

    // Connect flow
    if let Err(e) = run_connect(cli).await {
        eprintln!("\n  \u{274C} {}", e);
        std::process::exit(1);
    }
}

async fn run_list_projects() {
    let configs = match load_project_configs().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load projects: {}", e);
            return;
        }
    };

    if configs.is_empty() {
        eprintln!("No projects configured.");
        eprintln!("Add projects in ~/.connection-app/projects.json");
        return;
    }

    println!("\nConfigured projects:\n");
    let mut keys: Vec<&String> = configs.keys().collect();
    keys.sort();
    for key in keys {
        let config = &configs[key];
        println!(
            "  {} — {} ({})",
            key, config.name, config.region
        );
    }
    println!();
}

async fn run_list_profiles() {
    let profiles = read_aws_profile_names().await;

    if profiles.is_empty() {
        eprintln!("No AWS profiles found in ~/.aws/config");
        return;
    }

    println!("\nAWS profiles:\n");
    for profile in &profiles {
        println!("  {}", profile);
    }
    println!();
}

async fn run_connect(cli: Cli) -> Result<(), String> {
    // Load project configs
    let configs = load_project_configs()
        .await
        .map_err(|e| format!("Failed to load project configs: {}", e))?;

    if configs.is_empty() {
        return Err(
            "No projects configured.\nAdd projects in ~/.connection-app/projects.json".to_string(),
        );
    }

    // Load AWS profiles
    let all_profiles = read_aws_profile_names().await;
    if all_profiles.is_empty() {
        return Err("No AWS profiles found in ~/.aws/config".to_string());
    }

    // Select project
    let (project_key, project_config) = select_project(&cli, &configs, &all_profiles)?;

    // Get matching profiles for this project
    let matching_profiles =
        get_profiles_for_project(&all_profiles, &project_config, &configs);

    if matching_profiles.is_empty() {
        return Err(format!(
            "No matching AWS profiles found for project '{}'",
            project_key
        ));
    }

    // Select profile
    let profile = select_profile(&cli, &matching_profiles)?;

    // Determine local port
    let local_port = cli
        .port
        .unwrap_or_else(|| get_local_port(&profile, &project_config));

    eprintln!(
        "\n  \u{1F680} Connecting to {} via profile {}...\n",
        project_config.name, profile
    );

    // SSO pre-flight
    let sso_handler = CliSsoHandler;
    ensure_sso_session(&profile, &sso_handler, None)
        .await
        .map_err(|e| format!("SSO login failed: {}", e))?;

    // Create AWS clients
    let clients = create_aws_clients(&profile, &project_config.region).await;

    // Check credentials
    eprintln!("  \u{1F511} Checking credentials...");
    let cred_check = operations::check_credentials_valid(&clients).await;
    if !cred_check.valid {
        return Err(format!(
            "AWS credentials invalid: {}",
            cred_check.error.unwrap_or_else(|| "unknown".to_string())
        ));
    }

    let connection_type = if project_config.connection_type.is_empty() {
        "rds"
    } else {
        project_config.connection_type.as_str()
    };

    if connection_type == "service" {
        run_service_connect(&clients, &project_config, &local_port).await
    } else {
        // Select database if multiple are configured
        let selected_database = if let Some(ref databases) = project_config.databases {
            if databases.len() > 1 {
                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select database")
                    .items(databases)
                    .default(0)
                    .interact()
                    .map_err(|e| format!("Database selection failed: {}", e))?;
                Some(databases[selection].clone())
            } else if databases.len() == 1 {
                Some(databases[0].clone())
            } else {
                None
            }
        } else {
            None
        };

        run_rds_connect(&clients, &profile, &project_config, &local_port, selected_database.as_deref()).await
    }
}

async fn run_rds_connect(
    clients: &connection_app_lib::aws::credentials::AwsClients,
    profile: &str,
    project_config: &ProjectConfig,
    local_port: &str,
    selected_database: Option<&str>,
) -> Result<(), String> {
    let effective_db = project_config.effective_database(selected_database);

    // Find bastion instance
    eprintln!("  \u{1F50D} Finding bastion instance...");
    let instance_id = find_bastion_instance(clients, project_config.bastion_pattern(), None)
        .await
        .map_err(|e| format!("Failed to find bastion: {}", e))?;

    // Get RDS endpoint
    eprintln!("  \u{1F4E1} Getting RDS endpoint...");
    let rds_endpoint = operations::get_rds_endpoint(
        clients,
        &project_config.rds_type,
        &project_config.rds_pattern,
    )
    .await
    .map_err(|e| format!("Failed to get RDS endpoint: {}", e))?
    .ok_or_else(|| "No matching RDS endpoint found.".to_string())?;

    // Get RDS port
    let fallback_port = get_default_port_for_engine(project_config);
    let rds_port = operations::get_rds_port(
        clients,
        &project_config.rds_type,
        &project_config.rds_pattern,
        &fallback_port,
    )
    .await
    .map_err(|e| format!("Failed to get RDS port: {}", e))?;

    // Determine auth type (default to "secrets")
    let auth_type = if project_config.auth_type.is_empty() {
        "secrets"
    } else {
        project_config.auth_type.as_str()
    };

    let (username, password) = if auth_type == "iam" {
        eprintln!("  \u{1F511} Generating IAM auth token...");
        let iam_username = project_config
            .iam_username
            .as_deref()
            .ok_or("iamUsername is required when authType is \"iam\"")?;

        let rds_port_num: u16 = rds_port
            .parse()
            .map_err(|_| format!("Invalid RDS port number: {}", rds_port))?;

        let sdk_config =
            connection_app_lib::aws::credentials::build_aws_config(profile, &project_config.region)
                .await;
        let token = connection_app_lib::aws::iam_auth::generate_rds_auth_token(
            &sdk_config,
            &rds_endpoint,
            rds_port_num,
            iam_username,
        )
        .await
        .map_err(|e| format!("Failed to generate IAM auth token: {}", e))?;

        (iam_username.to_string(), token)
    } else {
        // "secrets" auth type
        eprintln!("  \u{1F4E6} Getting database credentials...");
        let db_creds = operations::get_connection_credentials(
            clients,
            &project_config.secret_prefix,
            effective_db,
            project_config.secret_path.as_deref(),
            project_config.secret_username_field.as_deref(),
            project_config.secret_password_field.as_deref(),
        )
        .await
        .map_err(|e| format!("Failed to get credentials: {}", e))?;
        (db_creds.username, db_creds.password)
    };

    // Start SSM session
    eprintln!("  \u{1F6E0}\u{FE0F}  Starting SSM session...");
    let session_response = start_session(
        clients,
        &instance_id,
        &rds_endpoint,
        &rds_port,
        local_port,
    )
    .await
    .map_err(|e| format!("Failed to start SSM session: {}", e))?;

    let (stream_url, token_value) = extract_session_info(&session_response)?;

    let port_num: u16 = local_port
        .parse()
        .map_err(|_| format!("Invalid port: {}", local_port))?;

    // Display connection info with dynamic-width box
    let masked_password = mask_password(&password);
    let rows = vec![
        ("Host",     "localhost".to_string()),
        ("Port",     local_port.to_string()),
        ("Username", username.clone()),
        ("Password", masked_password),
        ("Database", effective_db.to_string()),
        ("Endpoint", rds_endpoint.clone()),
    ];
    print_info_box(&rows);

    // Copy password to clipboard
    if try_copy_to_clipboard(&password) {
        eprintln!("  \u{1F4CB} Password copied to clipboard\n");
    }

    run_tunnel(stream_url, token_value, port_num, Some(password)).await
}

async fn run_service_connect(
    clients: &connection_app_lib::aws::credentials::AwsClients,
    project_config: &ProjectConfig,
    local_port: &str,
) -> Result<(), String> {
    let target_type = project_config
        .target_type
        .as_deref()
        .unwrap_or("ec2-direct");
    let remote_port = project_config
        .remote_port
        .map(|p| p.to_string())
        .unwrap_or_else(|| "5900".to_string());
    let service_type = project_config
        .service_type
        .as_deref()
        .unwrap_or("custom")
        .to_uppercase();

    let session_response = match target_type {
        "ec2-direct" => {
            let pattern = project_config
                .target_pattern
                .as_deref()
                .ok_or("targetPattern is required for ec2-direct")?;
            eprintln!("  \u{1F50D} Finding EC2 instance...");
            let (instance_id, _private_ip) = find_ec2_instance(clients, pattern)
                .await
                .map_err(|e| format!("Failed to find EC2 instance: {}", e))?;
            eprintln!("  \u{1F6E0}\u{FE0F}  Starting direct SSM session to {}...", instance_id);
            start_direct_port_forwarding_session(clients, &instance_id, &remote_port, local_port)
                .await
                .map_err(|e| format!("Failed to start SSM session: {}", e))?
        }
        "ec2-bastion" => {
            let pattern = project_config
                .target_pattern
                .as_deref()
                .ok_or("targetPattern is required for ec2-bastion")?;
            eprintln!("  \u{1F50D} Finding bastion and target EC2 instance...");
            let bastion_id = find_bastion_instance(clients, project_config.bastion_pattern(), None)
                .await
                .map_err(|e| format!("Failed to find bastion: {}", e))?;
            let (_instance_id, private_ip) = find_ec2_instance(clients, pattern)
                .await
                .map_err(|e| format!("Failed to find EC2 instance: {}", e))?;
            eprintln!("  \u{1F6E0}\u{FE0F}  Starting SSM session via bastion...");
            start_session(clients, &bastion_id, &private_ip, &remote_port, local_port)
                .await
                .map_err(|e| format!("Failed to start SSM session: {}", e))?
        }
        "ecs-bastion" => {
            let cluster = project_config
                .ecs_cluster
                .as_deref()
                .ok_or("ecsCluster is required for ecs-bastion")?;
            let service = project_config
                .ecs_service
                .as_deref()
                .ok_or("ecsService is required for ecs-bastion")?;
            eprintln!("  \u{1F50D} Finding bastion and ECS task...");
            let bastion_id = find_bastion_instance(clients, project_config.bastion_pattern(), None)
                .await
                .map_err(|e| format!("Failed to find bastion: {}", e))?;
            let task_ip = find_ecs_task_ip(clients, cluster, service)
                .await
                .map_err(|e| format!("Failed to find ECS task: {}", e))?;
            eprintln!("  \u{1F6E0}\u{FE0F}  Starting SSM session via bastion to ECS task {}...", task_ip);
            start_session(clients, &bastion_id, &task_ip, &remote_port, local_port)
                .await
                .map_err(|e| format!("Failed to start SSM session: {}", e))?
        }
        _ => return Err(format!("Unknown target type: {}", target_type)),
    };

    let (stream_url, token_value) = extract_session_info(&session_response)?;

    let port_num: u16 = local_port
        .parse()
        .map_err(|_| format!("Invalid port: {}", local_port))?;

    let mut rows = vec![
        ("Host",    "localhost".to_string()),
        ("Port",    local_port.to_string()),
        ("Service", service_type.clone()),
        ("Target",  target_type.to_string()),
    ];

    // Show SSH command for SSH service type
    if service_type == "SSH" {
        let ssh_user = project_config
            .ssh_username
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("ec2-user");
        let mut cmd = format!(
            "ssh -p {} {}@localhost -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null",
            local_port, ssh_user
        );
        if let Some(ref key_path) = project_config.ssh_key_path
            && !key_path.is_empty()
        {
            cmd.push_str(&format!(" -i {}", key_path));
        }
        rows.push(("SSH Cmd", cmd));
    }

    print_info_box(&rows);

    run_tunnel(stream_url, token_value, port_num, None).await
}

fn extract_session_info(
    response: &aws_sdk_ssm::operation::start_session::StartSessionOutput,
) -> Result<(String, String), String> {
    let stream_url = response
        .stream_url()
        .ok_or_else(|| "No StreamUrl in session response".to_string())?
        .to_string();
    let token_value = response
        .token_value()
        .ok_or_else(|| "No TokenValue in session response".to_string())?
        .to_string();
    Ok((stream_url, token_value))
}

fn print_info_box(rows: &[(&str, String)]) {
    let lines: Vec<String> = rows
        .iter()
        .map(|(label, value)| format!("  {:<10}{}", format!("{}:", label), value))
        .collect();
    let max_line_len = lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let box_width = max_line_len + 2;

    eprintln!("\n  \u{2705} Connected!\n");
    eprintln!("  \u{250C}{}\u{2510}", "\u{2500}".repeat(box_width));
    for line in &lines {
        eprintln!("  \u{2502}{:<box_width$}\u{2502}", line, box_width = box_width);
    }
    eprintln!("  \u{2514}{}\u{2518}\n", "\u{2500}".repeat(box_width));
}

async fn run_tunnel(
    stream_url: String,
    token_value: String,
    port_num: u16,
    password: Option<String>,
) -> Result<(), String> {
    if password.is_some() {
        eprintln!("  Commands: [p] show password  [c] copy password  [Ctrl+C] disconnect\n");
    } else {
        eprintln!("  Press Ctrl+C to disconnect.\n");
    }

    let cancel = CancellationToken::new();
    let cancel_signal = cancel.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        eprintln!("\n  \u{1F6D1} Disconnecting...");
        cancel_signal.cancel();
    });

    // Spawn interactive command reader if password is available
    if let Some(pw) = password {
        let cancel_reader = cancel.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::{BufRead, Write};
            let stdin = std::io::stdin();
            let mut stdout = std::io::stderr();
            loop {
                if cancel_reader.is_cancelled() {
                    break;
                }
                let mut input = String::new();
                if stdin.lock().read_line(&mut input).is_err() {
                    break;
                }
                let cmd = input.trim().to_lowercase();
                match cmd.as_str() {
                    "p" | "password" | "show" => {
                        let _ = writeln!(stdout, "\n  \u{1F513} Password: {}\n", pw);
                    }
                    "c" | "copy" => {
                        if try_copy_to_clipboard(&pw) {
                            let _ = writeln!(stdout, "\n  \u{1F4CB} Password copied to clipboard\n");
                        } else {
                            let _ = writeln!(stdout, "\n  \u{26A0}\u{FE0F}  Failed to copy to clipboard\n");
                        }
                    }
                    "" => {} // ignore empty lines
                    _ => {
                        let _ = writeln!(
                            stdout,
                            "  Unknown command '{}'. Use [p] show password, [c] copy password",
                            cmd
                        );
                    }
                }
            }
        });
    }

    start_native_port_forwarding(stream_url, token_value, port_num, cancel, None).await?;

    eprintln!("  \u{1F44B} Disconnected.\n");

    Ok(())
}

fn select_project(
    cli: &Cli,
    configs: &HashMap<String, ProjectConfig>,
    all_profiles: &[String],
) -> Result<(String, ProjectConfig), String> {
    if let Some(ref project_name) = cli.project {
        // Direct project selection by key
        if let Some(config) = configs.get(project_name) {
            return Ok((project_name.clone(), config.clone()));
        }
        // Try matching by name (case-insensitive)
        for (key, config) in configs {
            if config.name.to_lowercase() == project_name.to_lowercase() {
                return Ok((key.clone(), config.clone()));
            }
        }
        return Err(format!("Project '{}' not found", project_name));
    }

    // Filter to projects that have at least one matching profile
    let mut available: Vec<(String, &ProjectConfig)> = configs
        .iter()
        .filter(|(_key, config)| {
            !get_profiles_for_project(all_profiles, config, configs).is_empty()
        })
        .map(|(key, config)| (key.clone(), config))
        .collect();

    available.sort_by(|a, b| a.1.name.cmp(&b.1.name));

    if available.is_empty() {
        return Err("No projects have matching AWS profiles.".to_string());
    }

    if available.len() == 1 {
        let (key, config) = available
            .into_iter()
            .next()
            .ok_or_else(|| "No available projects".to_string())?;
        return Ok((key, config.clone()));
    }

    let items: Vec<String> = available
        .iter()
        .map(|(key, config)| format!("{} ({})", config.name, key))
        .collect();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select project")
        .items(&items)
        .default(0)
        .interact()
        .map_err(|e| format!("Selection cancelled: {}", e))?;

    let (key, config) = available
        .into_iter()
        .nth(selection)
        .ok_or_else(|| format!("Invalid selection index: {}", selection))?;
    Ok((key, config.clone()))
}

fn select_profile(cli: &Cli, matching_profiles: &[String]) -> Result<String, String> {
    if let Some(ref profile) = cli.profile {
        if matching_profiles.contains(profile) {
            return Ok(profile.clone());
        }
        return Err(format!(
            "Profile '{}' not found among matching profiles",
            profile
        ));
    }

    if matching_profiles.len() == 1 {
        return Ok(matching_profiles[0].clone());
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select environment (AWS profile)")
        .items(matching_profiles)
        .default(0)
        .interact()
        .map_err(|e| format!("Selection cancelled: {}", e))?;

    Ok(matching_profiles[selection].clone())
}

fn mask_password(password: &str) -> String {
    if password.len() <= 4 {
        return "*".repeat(password.len());
    }
    let visible = &password[..4];
    format!("{}{}", visible, "*".repeat(password.len() - 4))
}

fn try_copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        // Try xclip first, then xsel
        for cmd in &["xclip", "xsel"] {
            let args = if *cmd == "xclip" {
                vec!["-selection", "clipboard"]
            } else {
                vec!["--clipboard", "--input"]
            };
            if let Ok(mut child) = std::process::Command::new(cmd)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(ref mut stdin) = child.stdin {
                    let _ = stdin.write_all(text.as_bytes());
                }
                return child.wait().map(|s| s.success()).unwrap_or(false);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().map(|s| s.success()).unwrap_or(false);
        }
    }
    false
}
