use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Select};
use connection_app_lib::aws::credentials::create_aws_clients;
use connection_app_lib::aws::operations;
use connection_app_lib::aws::sso::{ensure_sso_session, CliSsoHandler};
use connection_app_lib::config::accounts::{load_account_cache, profile_accounts, ProfileAccount};
use connection_app_lib::config::aws_config::{read_aws_config, read_aws_profile_names};
use connection_app_lib::config::environments::{
    account_to_verify, effective_project_config, find_environment, pick_profile,
    resolve_environments, ResolvedEnvironment,
};
use connection_app_lib::config::preferences::{load_preferences, Preferences};
use connection_app_lib::config::projects::{
    get_default_port_for_engine, load_project_configs, AuthMode, ProjectConfig,
};
use connection_app_lib::tunnel::native::{
    bind_local_listeners, start_multiplexed_port_forwarding, start_native_port_forwarding,
    LocalSource,
};
use connection_app_lib::tunnel::pg_proxy::{self, PgAuthProxy, ProxySettings, SecretRefetch, UpstreamAuth};
use std::collections::HashMap;
use std::sync::Arc;

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

    /// Environment name (skip interactive selection)
    #[arg(short, long)]
    env: Option<String>,

    /// AWS profile name; must reach the environment's account
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

const LEGACY_NOTE: &str =
    "Note: projects.json uses the old profile-name format. Open ConnectionApp to update it.";

/// Everything environment resolution needs, read once.
struct Context {
    configs: HashMap<String, ProjectConfig>,
    profiles: Vec<ProfileAccount>,
    prefs: Preferences,
}

impl Context {
    fn resolve(&self, project: &ProjectConfig) -> Vec<ResolvedEnvironment> {
        resolve_environments(project, &self.configs, &self.profiles, &self.prefs)
    }
}

async fn load_context() -> Result<Context, String> {
    let configs = load_project_configs()
        .await
        .map_err(|e| format!("Failed to load project configs: {}", e))?;
    let aws_profiles = read_aws_config()
        .await
        .map_err(|e| format!("Failed to read AWS config: {}", e))?;
    let profiles = profile_accounts(&aws_profiles, &load_account_cache().await);
    Ok(Context {
        configs,
        profiles,
        prefs: load_preferences().await,
    })
}

/// Legacy projects keep resolving through the profile-name path; nothing is written.
fn print_legacy_note(configs: &HashMap<String, ProjectConfig>) {
    if configs.values().any(|c| c.is_legacy()) {
        eprintln!("{}", LEGACY_NOTE);
    }
}

fn account_suffix(account_id: &str) -> String {
    if account_id.is_empty() {
        String::new()
    } else {
        format!(" ({})", account_id)
    }
}

async fn run_list_projects() {
    let ctx = match load_context().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };

    if ctx.configs.is_empty() {
        eprintln!("No projects configured.");
        eprintln!("Add projects in ~/.connection-app/projects.json");
        return;
    }

    print_legacy_note(&ctx.configs);
    println!("\nConfigured projects:\n");
    let mut keys: Vec<&String> = ctx.configs.keys().collect();
    keys.sort();
    for key in keys {
        let config = &ctx.configs[key];
        println!("  {} — {} ({})", key, config.name, config.region);
        for r in ctx.resolve(config) {
            println!(
                "    {}{}  port {}  {}",
                r.environment.name,
                account_suffix(&r.environment.account_id),
                r.environment.port,
                r.profile.as_deref().map_or("no profile".to_string(), |p| format!("profile {}", p)),
            );
        }
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
    let ctx = load_context().await?;

    if ctx.configs.is_empty() {
        return Err(
            "No projects configured.\nAdd projects in ~/.connection-app/projects.json".to_string(),
        );
    }
    if ctx.profiles.is_empty() {
        return Err("No AWS profiles found in ~/.aws/config".to_string());
    }
    print_legacy_note(&ctx.configs);

    // Select project
    let (project_key, base_config) = select_project(&cli, &ctx)?;

    // Select environment
    let resolved_all = ctx.resolve(&base_config);
    let resolved = choose_environment(&cli, &project_key, &resolved_all)?;
    let env_name = resolved.environment.name.clone();
    let profile = pick_profile(resolved, cli.profile.as_deref()).map_err(|e| e.to_string())?;
    let project_config = effective_project_config(&base_config, resolved);

    // Determine local port
    let local_port = cli
        .port
        .clone()
        .unwrap_or_else(|| resolved.environment.port.clone());

    eprintln!(
        "\n  \u{1F680} Connecting to {} ({}) via profile {}...\n",
        project_config.name, env_name, profile
    );

    // SSO pre-flight
    let sso_handler = CliSsoHandler;
    ensure_sso_session(&profile, &sso_handler, None)
        .await
        .map_err(|e| format!("SSO login failed: {}", e))?;

    // Create AWS clients
    let clients = create_aws_clients(&profile, &project_config.region).await;

    // The profile must sign in to the environment's account
    if let Some(expected) = account_to_verify(resolved) {
        eprintln!("  \u{1F50E} Checking AWS account...");
        let actual = operations::caller_account_id(&clients)
            .await
            .map_err(|e| format!("Failed to check AWS account: {}", e))?;
        operations::ensure_account(expected, &actual, &env_name).map_err(|e| e.to_string())?;
    }

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

        run_rds_connect(&clients, &project_key, resolved, &profile, &project_config, &local_port, selected_database.as_deref()).await
    }
}

async fn run_rds_connect(
    clients: &connection_app_lib::aws::credentials::AwsClients,
    project_key: &str,
    resolved: &ResolvedEnvironment,
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
    let rds_target = operations::get_rds_target(
        clients,
        &project_config.rds_type,
        &project_config.rds_pattern,
    )
    .await
    .map_err(|e| format!("Failed to get RDS endpoint: {}", e))?
    .ok_or_else(|| "No matching RDS endpoint found.".to_string())?;
    let fallback_secret = operations::fallback_secret_name(&rds_target.identifier);
    let rds_endpoint = rds_target.endpoint;

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

    // Automatic with no secret configured signs in as iamUsername, like IAM only.
    let auth_mode = project_config.connect_auth_mode();

    let use_proxy = project_config.uses_pg_auth_proxy();
    let rds_port_num: u16 = rds_port
        .parse()
        .map_err(|_| format!("Invalid RDS port number: {}", rds_port))?;

    let (username, password, proxy_settings) = if auth_mode == AuthMode::IamOnly {
        eprintln!("  \u{1F511} Generating IAM auth token...");
        let iam_username = project_config
            .iam_username
            .as_deref()
            .ok_or("iamUsername is required when authType is \"iam\"")?;

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

        if use_proxy {
            // The token above only proves the credential chain works;
            // the proxy mints its own on demand.
            let settings = ProxySettings {
                db_user: iam_username.to_string(),
                local_password: local_proxy_password(project_key, resolved).await?,
                auth: UpstreamAuth::IamOnly,
                sdk_config,
            };
            (iam_username.to_string(), settings.local_password.clone(), Some(settings))
        } else {
            (iam_username.to_string(), token, None)
        }
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
            Some(&fallback_secret),
        )
        .await
        .map_err(|e| format!("Failed to get credentials: {}", e))?;
        if db_creds.via_fallback {
            eprintln!(
                "  \u{21AA}\u{FE0F}  No secret matches '{}'; using {}",
                project_config.secret_prefix, db_creds.secret_name
            );
        }
        if use_proxy {
            let settings = ProxySettings {
                db_user: db_creds.username.clone(),
                local_password: local_proxy_password(project_key, resolved).await?,
                auth: {
                    let password = db_creds.password;
                    let refetch = Some(SecretRefetch {
                        source: Arc::new(operations::SecretLocator {
                            profile: profile.to_string(),
                            region: project_config.region.clone(),
                            secret_prefix: project_config.secret_prefix.clone(),
                            database: effective_db.to_string(),
                            secret_path: project_config.secret_path.clone(),
                            username_field: project_config.secret_username_field.clone(),
                            password_field: project_config.secret_password_field.clone(),
                            fallback_secret: Some(fallback_secret.clone()),
                        }),
                        key: format!("{}:{}", project_key, resolved.environment.name),
                    });
                    if auth_mode == AuthMode::PasswordOnly {
                        UpstreamAuth::PasswordOnly { password, refetch }
                    } else {
                        UpstreamAuth::Auto { password, refetch }
                    }
                },
                sdk_config: connection_app_lib::aws::credentials::build_aws_config(
                    profile,
                    &project_config.region,
                )
                .await,
            };
            (db_creds.username, settings.local_password.clone(), Some(settings))
        } else {
            (db_creds.username, db_creds.password, None)
        }
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

    let multiplexed = project_config.multiplexed.unwrap_or(true);

    // Display connection info with dynamic-width box
    let masked_password = mask_password(&password);
    let mut rows = vec![
        ("Host",     "localhost".to_string()),
        ("Port",     local_port.to_string()),
        ("Username", username.clone()),
        ("Password", masked_password),
        ("Database", effective_db.to_string()),
        ("Endpoint", rds_endpoint.clone()),
    ];

    let Some(settings) = proxy_settings else {
        print_info_box(&rows);
        if try_copy_to_clipboard(&password) {
            eprintln!("  \u{1F4CB} Password copied to clipboard\n");
        }
        return run_tunnel(stream_url, token_value, LocalSource::Listen(port_num), Some(password), multiplexed).await;
    };

    // The auth proxy owns the local port; the tunnel takes its connections as pipes.
    let (listener_v4, listener_v6) = bind_local_listeners(port_num)?;
    let (dialer, inbox) = pg_proxy::tunnel_pipes();
    let proxy = Arc::new(PgAuthProxy::for_rds(settings, &rds_endpoint, rds_port_num, Arc::new(dialer))?);

    let cancel = CancellationToken::new();
    disconnect_on_ctrl_c(&cancel);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let tunnel = tokio::spawn(forward_ports(
        stream_url,
        token_value,
        LocalSource::Pipes(inbox),
        cancel.clone(),
        Some(ready_tx),
        multiplexed,
    ));
    if !matches!(ready_rx.await, Ok(Ok(()))) {
        cancel.cancel();
        tunnel.await.map_err(|e| format!("Tunnel task failed: {}", e))??;
        return Err("Tunnel failed to start".to_string());
    }
    tokio::spawn(pg_proxy::run_listener(proxy.clone(), listener_v4, listener_v6, cancel.clone()));

    eprintln!("  \u{1F510} Checking database sign-in...");
    let method = match tokio::time::timeout(std::time::Duration::from_secs(30), proxy.probe(effective_db)).await {
        Ok(Ok(method)) => method,
        Ok(Err(e)) => {
            eprintln!("  \u{26A0}\u{FE0F}  Database sign-in check failed: {}", e);
            "check failed".to_string()
        }
        Err(_) => {
            eprintln!("  \u{26A0}\u{FE0F}  Database sign-in check timed out");
            "check timed out".to_string()
        }
    };
    let iam = method.starts_with("IAM");
    rows.push(("Auth", method));
    if iam {
        rows.push(("IAM", "IAM token is refreshed automatically".to_string()));
    }
    print_info_box(&rows);
    if try_copy_to_clipboard(&password) {
        eprintln!("  \u{1F4CB} Password copied to clipboard\n");
    }
    eprintln!("  Commands: [p] show password  [c] copy password  [Ctrl+C] disconnect\n");
    spawn_command_reader(password, &cancel);

    tunnel.await.map_err(|e| format!("Tunnel task failed: {}", e))??;
    eprintln!("  \u{1F44B} Disconnected.\n");
    Ok(())
}

async fn local_proxy_password(
    project_key: &str,
    resolved: &ResolvedEnvironment,
) -> Result<String, String> {
    connection_app_lib::config::local_passwords::local_proxy_password(
        project_key,
        &resolved.environment.name,
        &resolved.candidates,
    )
    .await
    .map_err(|e| format!("Failed to load the local proxy password: {}", e))
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

    let multiplexed = project_config.multiplexed.unwrap_or(true);
    run_tunnel(stream_url, token_value, LocalSource::Listen(port_num), None, multiplexed).await
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
    local: LocalSource,
    password: Option<String>,
    multiplexed: bool,
) -> Result<(), String> {
    if password.is_some() {
        eprintln!("  Commands: [p] show password  [c] copy password  [Ctrl+C] disconnect\n");
    } else {
        eprintln!("  Press Ctrl+C to disconnect.\n");
    }

    let cancel = CancellationToken::new();
    disconnect_on_ctrl_c(&cancel);

    // Spawn interactive command reader if password is available
    if let Some(pw) = password {
        spawn_command_reader(pw, &cancel);
    }

    forward_ports(stream_url, token_value, local, cancel, None, multiplexed).await?;

    eprintln!("  \u{1F44B} Disconnected.\n");

    Ok(())
}

fn disconnect_on_ctrl_c(cancel: &CancellationToken) {
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        eprintln!("\n  \u{1F6D1} Disconnecting...");
        cancel_signal.cancel();
    });
}

fn spawn_command_reader(pw: String, cancel: &CancellationToken) {
    let cancel_reader = cancel.clone();
    tokio::task::spawn_blocking(move || {
        run_command_reader(
            std::io::stdin().lock(),
            std::io::stderr(),
            &pw,
            &cancel_reader,
        );
    });
}

async fn forward_ports(
    stream_url: String,
    token_value: String,
    local: LocalSource,
    cancel: CancellationToken,
    ready_tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
    multiplexed: bool,
) -> Result<(), String> {
    if multiplexed {
        start_multiplexed_port_forwarding(stream_url, token_value, local, cancel, ready_tx).await
    } else {
        start_native_port_forwarding(stream_url, token_value, local, cancel, ready_tx).await
    }
}

/// Read user commands from `input` and respond on `out` until EOF, read
/// error, or cancellation.
fn run_command_reader<R: std::io::BufRead, W: std::io::Write>(
    mut input: R,
    mut out: W,
    pw: &str,
    cancel: &CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let mut line = String::new();
        match input.read_line(&mut line) {
            // Ok(0) = EOF — stdin closed (piped/backgrounded). Stop reading;
            // the tunnel itself keeps running.
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let cmd = line.trim().to_lowercase();
        match cmd.as_str() {
            "p" | "password" | "show" => {
                let _ = writeln!(out, "\n  \u{1F513} Password: {}\n", pw);
            }
            "c" | "copy" => {
                if try_copy_to_clipboard(pw) {
                    let _ = writeln!(out, "\n  \u{1F4CB} Password copied to clipboard\n");
                } else {
                    let _ = writeln!(out, "\n  \u{26A0}\u{FE0F}  Failed to copy to clipboard\n");
                }
            }
            "" => {} // ignore empty lines
            _ => {
                let _ = writeln!(
                    out,
                    "  Unknown command '{}'. Use [p] show password, [c] copy password",
                    cmd
                );
            }
        }
    }
}

fn select_project(cli: &Cli, ctx: &Context) -> Result<(String, ProjectConfig), String> {
    let configs = &ctx.configs;
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

    // Filter to projects that can be reached with at least one profile
    let mut available: Vec<(String, &ProjectConfig)> = configs
        .iter()
        .filter(|(_key, config)| ctx.resolve(config).iter().any(|r| r.profile.is_some()))
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

/// Pick the environment named by `--env` / `--profile`.
///
/// `--env` is an exact, case-sensitive match. `--profile` must be a candidate
/// of the chosen environment; without `--env` it selects the first
/// environment whose candidates contain it.
fn select_environment<'a>(
    project: &str,
    resolved: &'a [ResolvedEnvironment],
    env: Option<&str>,
    profile: Option<&str>,
) -> Result<&'a ResolvedEnvironment, String> {
    if let Some(name) = env {
        let found = find_environment(resolved, name).ok_or_else(|| {
            let names: Vec<&str> = resolved.iter().map(|r| r.environment.name.as_str()).collect();
            format!(
                "Unknown environment '{}' for {}. Available: {}",
                name,
                project,
                names.join(", ")
            )
        })?;
        if let Some(p) = profile
            && !found.candidates.iter().any(|c| c == p)
        {
            return Err(format!(
                "Profile '{}' is not a profile for environment '{}'",
                p, name
            ));
        }
        return Ok(found);
    }
    let Some(p) = profile else {
        return Err("No --env or --profile given".to_string());
    };
    let matching: Vec<&ResolvedEnvironment> = resolved
        .iter()
        .filter(|r| r.candidates.iter().any(|c| c == p))
        .collect();
    match matching.as_slice() {
        [] => Err(format!("Profile '{}' does not belong to any environment of {}", p, project)),
        [only] => Ok(only),
        several => {
            let names: Vec<&str> = several.iter().map(|r| r.environment.name.as_str()).collect();
            Err(format!(
                "Profile '{}' reaches several environments of {} ({}); pass --env",
                p,
                project,
                names.join(", ")
            ))
        }
    }
}

/// `prod — covered-prod (231030592327)`, or `dev — no profile (073813807522)`.
/// A legacy environment is named after its profile, so the name is enough.
fn environment_label(r: &ResolvedEnvironment) -> String {
    if r.legacy {
        return format!("{}{}", r.environment.name, account_suffix(&r.environment.account_id));
    }
    format!(
        "{} — {}{}",
        r.environment.name,
        r.profile.as_deref().unwrap_or("no profile"),
        account_suffix(&r.environment.account_id)
    )
}

/// Apply the flags, or ask. Environments with no profile are listed as
/// disabled, with the reason.
fn choose_environment<'a>(
    cli: &Cli,
    project_key: &str,
    resolved: &'a [ResolvedEnvironment],
) -> Result<&'a ResolvedEnvironment, String> {
    if cli.env.is_some() || cli.profile.is_some() {
        return select_environment(project_key, resolved, cli.env.as_deref(), cli.profile.as_deref());
    }

    for r in resolved.iter().filter(|r| r.profile.is_none()) {
        eprintln!(
            "  {} (disabled: no AWS profile for account {})",
            environment_label(r),
            r.environment.account_id
        );
    }
    let selectable: Vec<&ResolvedEnvironment> =
        resolved.iter().filter(|r| r.profile.is_some()).collect();
    match selectable.as_slice() {
        [] => Err(format!(
            "No matching AWS profiles found for project '{}'",
            project_key
        )),
        [only] => Ok(only),
        _ => {
            let items: Vec<String> = selectable.iter().map(|r| environment_label(r)).collect();
            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select environment")
                .items(&items)
                .default(0)
                .interact()
                .map_err(|e| format!("Selection cancelled: {}", e))?;
            Ok(selectable[selection])
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use connection_app_lib::config::projects::Environment;

    /// Regression test: the command reader must terminate at EOF.
    ///
    /// `read_line` returns `Ok(0)` at EOF; treating that as an empty command
    /// made the loop spin forever, burning a CPU core for the lifetime of the
    /// tunnel whenever stdin was closed (piped/backgrounded invocations).
    #[test]
    fn command_reader_terminates_at_eof() {
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let input = std::io::Cursor::new(b"p\n".to_vec());
            let mut out = Vec::new();
            run_command_reader(input, &mut out, "s3cret", &cancel);
            let _ = done_tx.send(out);
        });
        let out = done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("command reader did not terminate at EOF (busy-loop)");
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("s3cret"), "expected password output, got: {text}");
    }

    fn resolved(name: &str, profile: Option<&str>, candidates: &[&str]) -> ResolvedEnvironment {
        ResolvedEnvironment {
            environment: Environment {
                name: name.into(),
                account_id: "231030592327".into(),
                port: "5460".into(),
                rds_pattern: None,
                region: None,
                target_pattern: None,
                ecs_cluster: None,
                ecs_service: None,
                bastion_pattern: None,
            },
            legacy: false,
            profile: profile.map(String::from),
            candidates: candidates.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn envs() -> Vec<ResolvedEnvironment> {
        vec![
            resolved("prod", Some("p1"), &["p1", "p2"]),
            resolved("staging", Some("s1"), &["s1"]),
            resolved("dev", None, &[]),
        ]
    }

    #[test]
    fn env_flag_exact_match() {
        let e = envs();
        let r = select_environment("covered", &e, Some("staging"), None).unwrap();
        assert_eq!(r.environment.name, "staging");
    }

    #[test]
    fn unknown_env_lists_available_names() {
        let e = envs();
        let err = select_environment("covered", &e, Some("Prod"), None).unwrap_err();
        assert_eq!(
            err,
            "Unknown environment 'Prod' for covered. Available: prod, staging, dev"
        );
    }

    #[test]
    fn profile_flag_selects_its_environment() {
        let e = envs();
        let r = select_environment("covered", &e, None, Some("s1")).unwrap();
        assert_eq!(r.environment.name, "staging");
    }

    #[test]
    fn profile_reaching_several_environments_needs_env() {
        let e = vec![
            resolved("prod", Some("admin"), &["admin", "p1"]),
            resolved("prod-replica", Some("admin"), &["admin"]),
            resolved("staging", Some("s1"), &["s1"]),
        ];
        let err = select_environment("covered", &e, None, Some("admin")).unwrap_err();
        assert_eq!(
            err,
            "Profile 'admin' reaches several environments of covered (prod, prod-replica); pass --env"
        );
        let r = select_environment("covered", &e, Some("prod-replica"), Some("admin")).unwrap();
        assert_eq!(r.environment.name, "prod-replica");
        let r = select_environment("covered", &e, None, Some("p1")).unwrap();
        assert_eq!(r.environment.name, "prod");
    }

    #[test]
    fn environment_label_names_profile_and_account() {
        assert_eq!(
            environment_label(&resolved("prod", Some("p1"), &["p1"])),
            "prod — p1 (231030592327)"
        );
        assert_eq!(
            environment_label(&resolved("dev", None, &[])),
            "dev — no profile (231030592327)"
        );
    }

    #[test]
    fn profile_not_in_env_is_error() {
        let e = envs();
        assert!(select_environment("covered", &e, Some("prod"), Some("s1")).is_err());
        assert!(select_environment("covered", &e, None, Some("nope")).is_err());
        let r = select_environment("covered", &e, Some("prod"), Some("p2")).unwrap();
        assert_eq!(r.environment.name, "prod");
    }
}
