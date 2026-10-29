use crate::config::projects::ProjectConfig;
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

static REGION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2}(-[a-z]+-\d+)$").unwrap());
static PORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").unwrap());
static SHELL_SAFE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/-]+$").unwrap());
static EC2_FILTER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/*?-]+$").unwrap());
static SSH_USERNAME_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._-]+$").unwrap());
static SSH_KEY_PATH_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._/~-]+$").unwrap());

/// True for names safe to use in keys, labels and filters (the rule
/// environment names are validated against).
pub fn is_shell_safe(value: &str) -> bool {
    SHELL_SAFE_PATTERN.is_match(value)
}

const VALID_RDS_TYPES: &[&str] = &["cluster", "instance"];
const VALID_ENGINES: &[&str] = &["postgres", "mysql"];
const VALID_CONNECTION_TYPES: &[&str] = &["rds", "service"];
const VALID_SERVICE_TYPES: &[&str] = &["vnc", "rdp", "custom", "ssh"];
const VALID_TARGET_TYPES: &[&str] = &["ec2-direct", "ec2-bastion", "ecs-bastion"];
const VALID_AUTH_TYPES: &[&str] = &["secrets", "iam"];
/// Pattern for secret path/ARN (allows colons for ARN format, plus alphanumeric, dots, hyphens, slashes, !)
static SECRET_PATH_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/:+-]+$").unwrap());
/// Pattern for validating dot-notation field names (e.g. "credentials.username")
static FIELD_NAME_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_.]+$").unwrap());

const REQUIRED_FIELDS: &[&str] = &[
    "name",
    "region",
    "database",
    "secretPrefix",
    "rdsType",
    "rdsPattern",
    "envPortMapping",
    "defaultPort",
];

/// A problem with one field of a project.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<FieldError>,
}

impl ValidationResult {
    /// Errors as "<field>: <message>" lines (just the message for project-level errors).
    pub fn messages(&self) -> Vec<String> {
        self.errors
            .iter()
            .map(|e| {
                if e.field.is_empty() {
                    e.message.clone()
                } else {
                    format!("{}: {}", e.field, e.message)
                }
            })
            .collect()
    }
}

fn err(errors: &mut Vec<FieldError>, field: &str, message: impl Into<String>) {
    errors.push(FieldError {
        field: field.to_string(),
        message: message.into(),
    });
}

static ACCOUNT_ID_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{12}$").unwrap());

fn validate_environments(
    config: &ProjectConfig,
    connection_type: &str,
    errors: &mut Vec<FieldError>,
) {
    if config.password_only == Some(true) && config.auth_type == "iam" {
        err(
            errors,
            "passwordOnly",
            "passwordOnly cannot be combined with authType \"iam\"",
        );
    }

    let Some(envs) = config.environments.as_ref() else {
        return;
    };
    let mut names = std::collections::HashSet::new();
    let mut ports = std::collections::HashSet::new();
    for (i, env) in envs.iter().enumerate() {
        let f = |name: &str| format!("environments[{}].{}", i, name);

        if env.name.is_empty() {
            err(errors, &f("name"), "Missing required field: name");
        } else if !SHELL_SAFE_PATTERN.is_match(&env.name) {
            err(
                errors,
                &f("name"),
                "name contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ! allowed)",
            );
        } else if !names.insert(env.name.as_str()) {
            err(errors, &f("name"), format!("duplicate environment name: {}", env.name));
        }

        if !ACCOUNT_ID_PATTERN.is_match(&env.account_id) {
            err(errors, &f("accountId"), "accountId must be exactly 12 digits");
        }

        if !PORT_PATTERN.is_match(&env.port) {
            err(errors, &f("port"), format!("port must be a numeric string: {}", env.port));
        } else if !ports.insert(env.port.as_str()) {
            err(errors, &f("port"), format!("duplicate port within project: {}", env.port));
        }

        if let Some(v) = env.rds_pattern.as_deref()
            && !v.is_empty()
            && connection_type == "rds"
            && !SHELL_SAFE_PATTERN.is_match(v)
        {
            err(
                errors,
                &f("rdsPattern"),
                "rdsPattern contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ! allowed)",
            );
        }
        if let Some(v) = env.region.as_deref()
            && !v.is_empty()
            && !REGION_PATTERN.is_match(v)
        {
            err(errors, &f("region"), format!("Invalid region format: {}", v));
        }
        for (key, v) in [
            ("targetPattern", env.target_pattern.as_deref()),
            ("bastionPattern", env.bastion_pattern.as_deref()),
        ] {
            if let Some(v) = v
                && !v.is_empty()
            {
                if v.len() > 256 {
                    err(errors, &f(key), format!("{key} must be 256 characters or fewer"));
                } else if !EC2_FILTER_PATTERN.is_match(v) {
                    err(
                        errors,
                        &f(key),
                        format!("{key} contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, !, * and ? allowed)"),
                    );
                }
            }
        }
    }
}

pub fn validate_project_config(config: &ProjectConfig) -> ValidationResult {
    let mut errors = Vec::new();

    // Determine connection type (default to "rds" if empty)
    let connection_type = if config.connection_type.is_empty() {
        "rds"
    } else {
        config.connection_type.as_str()
    };

    // Validate connectionType
    if !VALID_CONNECTION_TYPES.contains(&connection_type) {
        err(&mut errors, "connectionType", format!(
            "connectionType must be one of: {}",
            VALID_CONNECTION_TYPES.join(", ")
        ));
    }

    // Common required fields (all connection types)
    for (field, value) in [("name", &config.name), ("region", &config.region)] {
        if value.is_empty() {
            err(&mut errors, field, format!("Missing required field: {}", field));
        }
    }

    if connection_type == "rds" {
        // Determine auth type (default to "secrets" if empty)
        let auth_type = if config.auth_type.is_empty() {
            "secrets"
        } else {
            config.auth_type.as_str()
        };

        // Validate authType
        if !VALID_AUTH_TYPES.contains(&auth_type) {
            err(&mut errors, "authType", format!(
                "authType must be one of: {}",
                VALID_AUTH_TYPES.join(", ")
            ));
        }

        // RDS-specific required fields (always required regardless of auth type)
        for (field, value) in [
            ("database", &config.database),
            ("rdsType", &config.rds_type),
            ("rdsPattern", &config.rds_pattern),
            ("defaultPort", &config.default_port),
        ] {
            if value.is_empty() {
                err(&mut errors, field, format!("Missing required field: {}", field));
            }
        }

        // secretPrefix is required only for "secrets" auth type (unless secretPath is set)
        if auth_type == "secrets"
            && config.secret_prefix.is_empty()
            && config.secret_path.as_deref().is_none_or(|s| s.is_empty())
        {
            err(&mut errors, "secretPrefix", "Missing required field: secretPrefix (required when authType is \"secrets\" and secretPath is not set)".to_string());
        }

        // IAM-specific validation
        if auth_type == "iam"
            && config.iam_username.as_deref().is_none_or(|s| s.is_empty())
        {
            err(&mut errors, "iamUsername",
                "Missing required field: iamUsername (required when authType is \"iam\")"
                    .to_string(),
            );
        }

        // Validate rdsType
        if !config.rds_type.is_empty() && !VALID_RDS_TYPES.contains(&config.rds_type.as_str()) {
            err(&mut errors, "rdsType", format!(
                "rdsType must be one of: {}",
                VALID_RDS_TYPES.join(", ")
            ));
        }

        // Validate engine (optional)
        if let Some(ref engine) = config.engine
            && !engine.is_empty()
            && !VALID_ENGINES.contains(&engine.as_str())
        {
            err(&mut errors, "engine", format!(
                "engine must be one of: {}",
                VALID_ENGINES.join(", ")
            ));
        }

        // Validate shell-safe fields
        for (field, value) in [
            ("secretPrefix", &config.secret_prefix),
            ("rdsPattern", &config.rds_pattern),
            ("database", &config.database),
        ] {
            if !value.is_empty() && !SHELL_SAFE_PATTERN.is_match(value) {
                err(&mut errors, field, format!(
                    "{} contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ! allowed)",
                    field
                ));
            }
        }

        // Validate databases array (if provided)
        if let Some(ref databases) = config.databases {
            let mut seen = std::collections::HashSet::new();
            for (i, db) in databases.iter().enumerate() {
                if db.trim().is_empty() {
                    err(&mut errors, &format!("databases[{}]", i), format!(
                        "databases[{}]: entries must not be empty",
                        i
                    ));
                } else if !SHELL_SAFE_PATTERN.is_match(db) {
                    err(&mut errors, &format!("databases[{}]", i), format!(
                        "databases[{}] contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ! allowed)",
                        i
                    ));
                }
                if !seen.insert(db) {
                    err(&mut errors, "databases", format!(
                        "databases contains duplicate entry: {}",
                        db
                    ));
                }
            }
        }

        // Validate secretPath (if provided, must be non-empty and safe)
        if let Some(ref secret_path) = config.secret_path {
            if secret_path.is_empty() {
                err(&mut errors, "secretPath", "secretPath must be non-empty if provided".to_string());
            } else if !SECRET_PATH_PATTERN.is_match(secret_path) {
                err(&mut errors, "secretPath",
                    "secretPath contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, colons, +, and ! allowed)".to_string(),
                );
            }
        }

        // Validate secretUsernameField (if provided, must be non-empty)
        if let Some(ref field) = config.secret_username_field {
            if field.is_empty() {
                err(&mut errors, "secretUsernameField", "secretUsernameField must be non-empty if provided".to_string());
            } else if !FIELD_NAME_PATTERN.is_match(field) {
                err(&mut errors, "secretUsernameField",
                    "secretUsernameField contains invalid characters (only alphanumeric, dots, and underscores allowed)".to_string(),
                );
            }
        }

        // Validate secretPasswordField (if provided, must be non-empty)
        if let Some(ref field) = config.secret_password_field {
            if field.is_empty() {
                err(&mut errors, "secretPasswordField", "secretPasswordField must be non-empty if provided".to_string());
            } else if !FIELD_NAME_PATTERN.is_match(field) {
                err(&mut errors, "secretPasswordField",
                    "secretPasswordField contains invalid characters (only alphanumeric, dots, and underscores allowed)".to_string(),
                );
            }
        }
    } else if connection_type == "service" {
        // Validate serviceType
        match config.service_type.as_deref() {
            Some(st) if !st.is_empty() => {
                if !VALID_SERVICE_TYPES.contains(&st) {
                    err(&mut errors, "serviceType", format!(
                        "serviceType must be one of: {}",
                        VALID_SERVICE_TYPES.join(", ")
                    ));
                }
            }
            _ => {
                err(&mut errors, "serviceType", "Missing required field: serviceType".to_string());
            }
        }

        // Validate remotePort
        match config.remote_port {
            Some(port) if port > 0 => {}
            _ => {
                err(&mut errors, "remotePort", "Missing required field: remotePort (must be > 0)".to_string());
            }
        }

        // Validate targetType
        let target_type = match config.target_type.as_deref() {
            Some(tt) if !tt.is_empty() => {
                if !VALID_TARGET_TYPES.contains(&tt) {
                    err(&mut errors, "targetType", format!(
                        "targetType must be one of: {}",
                        VALID_TARGET_TYPES.join(", ")
                    ));
                }
                Some(tt)
            }
            _ => {
                err(&mut errors, "targetType", "Missing required field: targetType".to_string());
                None
            }
        };

        // Target-type-specific validation
        if let Some(tt) = target_type {
            match tt {
                "ec2-direct" | "ec2-bastion" => {
                    match config.target_pattern.as_deref() {
                        Some(tp) if !tp.is_empty() => {
                            if tp.len() > 256 {
                                err(&mut errors, "targetPattern",
                                    "targetPattern must be 256 characters or fewer".to_string(),
                                );
                            } else if !EC2_FILTER_PATTERN.is_match(tp) {
                                err(&mut errors, "targetPattern",
                                    "targetPattern contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, !, * and ? allowed)".to_string(),
                                );
                            }
                        }
                        _ => {
                            err(&mut errors, "targetPattern", "Missing required field: targetPattern".to_string());
                        }
                    }
                }
                "ecs-bastion" => {
                    if config
                        .ecs_cluster
                        .as_deref()
                        .is_none_or(|s| s.is_empty())
                    {
                        err(&mut errors, "ecsCluster", "Missing required field: ecsCluster".to_string());
                    }
                    if config
                        .ecs_service
                        .as_deref()
                        .is_none_or(|s| s.is_empty())
                    {
                        err(&mut errors, "ecsService", "Missing required field: ecsService".to_string());
                    }
                }
                _ => {}
            }
        }

        // SSH-specific validation
        if config.service_type.as_deref() == Some("ssh") {
            // sshUsername is optional but must be safe if provided
            if let Some(ref username) = config.ssh_username
                && !username.is_empty() && !SSH_USERNAME_PATTERN.is_match(username) {
                    err(&mut errors, "sshUsername",
                        "sshUsername contains invalid characters (only alphanumeric, dots, underscores, and hyphens allowed)".to_string(),
                    );
                }

            // sshKeyPath is optional but must be a safe path if provided
            if let Some(ref key_path) = config.ssh_key_path
                && !key_path.is_empty() && !SSH_KEY_PATH_PATTERN.is_match(key_path) {
                    err(&mut errors, "sshKeyPath",
                        "sshKeyPath contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ~ allowed)".to_string(),
                    );
                }

            // remotePort should default to 22 for SSH; warn if not set
            if config.remote_port.is_none() {
                err(&mut errors, "remotePort", "Missing required field: remotePort (use 22 for SSH)".to_string());
            }
        }
    }

    // envPortMapping is required (checked as non-empty in JS; we check the field exists via struct type)
    let _ = REQUIRED_FIELDS; // keep reference for documentation

    // Validate bastionPattern (EC2 filter pattern if provided — allows * and ?)
    if let Some(ref pattern) = config.bastion_pattern
        && !pattern.is_empty()
    {
        if pattern.len() > 256 {
            err(&mut errors, "bastionPattern", "bastionPattern must be 256 characters or fewer".to_string());
        } else if !EC2_FILTER_PATTERN.is_match(pattern) {
            err(&mut errors, "bastionPattern",
                "bastionPattern contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, !, * and ? allowed)".to_string(),
            );
        }
    }

    // Validate region
    if !config.region.is_empty() && !REGION_PATTERN.is_match(&config.region) {
        err(&mut errors, "region", format!("Invalid region format: {}", config.region));
    }

    // Validate defaultPort (if provided)
    if !config.default_port.is_empty() && !PORT_PATTERN.is_match(&config.default_port) {
        err(&mut errors, "defaultPort", format!(
            "defaultPort must be a numeric string: {}",
            config.default_port
        ));
    }

    // Legacy projects only: account-based projects take ports from environments
    if config.is_legacy() {
        for (key, value) in &config.env_port_mapping {
            if !PORT_PATTERN.is_match(value) {
                err(
                    &mut errors,
                    &format!("envPortMapping.{}", key),
                    format!("Port for \"{}\" must be a numeric string: {}", key, value),
                );
            }
        }
    }

    validate_environments(config, connection_type, &mut errors);

    ValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn valid_config() -> ProjectConfig {
        let mut env_port_mapping = HashMap::new();
        env_port_mapping.insert("dev".to_string(), "5433".to_string());
        env_port_mapping.insert("prod".to_string(), "5434".to_string());

        ProjectConfig {
            name: "Test Project".to_string(),
            region: "us-east-1".to_string(),
            database: "mydb".to_string(),
            databases: None,
            secret_prefix: "rds!cluster".to_string(),
            rds_type: "cluster".to_string(),
            engine: Some("postgres".to_string()),
            rds_pattern: "my-cluster".to_string(),
            profile_filter: Some("test-".to_string()),
            env_port_mapping,
            default_port: "5432".to_string(),
            bastion_pattern: None,
            connection_type: "rds".to_string(),
            service_type: None,
            remote_port: None,
            target_type: None,
            target_pattern: None,
            ecs_cluster: None,
            ecs_service: None,
            secret_path: None,
            secret_username_field: None,
            secret_password_field: None,
            auth_type: "secrets".to_string(),
            iam_username: None,
            multiplexed: None,
            ssh_username: None,
            ssh_key_path: None,
            environments: None,
            password_only: None,
        }
    }

    #[test]
    fn test_valid_config() {
        let result = validate_project_config(&valid_config());
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_missing_required_field() {
        let mut config = valid_config();
        config.name = String::new();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_invalid_rds_type() {
        let mut config = valid_config();
        config.rds_type = "invalid".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("rdsType")));
    }

    #[test]
    fn test_invalid_region() {
        let mut config = valid_config();
        config.region = "invalid-region".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("region")));
    }

    #[test]
    fn test_invalid_port() {
        let mut config = valid_config();
        config.default_port = "abc".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("defaultPort")));
    }

    #[test]
    fn test_invalid_shell_characters() {
        let mut config = valid_config();
        config.secret_prefix = "bad;prefix".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretPrefix")));
    }

    #[test]
    fn test_invalid_env_port_mapping_value() {
        let mut config = valid_config();
        config
            .env_port_mapping
            .insert("staging".to_string(), "notaport".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("staging")));
    }

    #[test]
    fn test_bastion_pattern_none_is_valid() {
        let config = valid_config(); // bastion_pattern: None
        let result = validate_project_config(&config);
        assert!(result.valid);
    }

    #[test]
    fn test_bastion_pattern_valid_wildcards() {
        for pattern in ["*bastion*", "my-bastion-host", "bastion*", "bastion?01", "*jump*"] {
            let mut config = valid_config();
            config.bastion_pattern = Some(pattern.to_string());
            let result = validate_project_config(&config);
            assert!(result.valid, "expected valid for pattern: {}", pattern);
        }
    }

    #[test]
    fn test_bastion_pattern_invalid_injection() {
        for pattern in ["bastion;rm -rf /", "bastion$(whoami)", "bastion host", "a\"b", "a'b"] {
            let mut config = valid_config();
            config.bastion_pattern = Some(pattern.to_string());
            let result = validate_project_config(&config);
            assert!(!result.valid, "expected invalid for pattern: {}", pattern);
            assert!(result.messages().iter().any(|e| e.contains("bastionPattern")));
        }
    }

    #[test]
    fn test_bastion_pattern_too_long() {
        let mut config = valid_config();
        config.bastion_pattern = Some("a".repeat(257));
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("256")));
    }

    fn valid_service_config_ec2() -> ProjectConfig {
        let mut env_port_mapping = HashMap::new();
        env_port_mapping.insert("dev".to_string(), "5901".to_string());

        ProjectConfig {
            name: "VNC Service".to_string(),
            region: "us-east-1".to_string(),
            database: String::new(),
            databases: None,
            secret_prefix: String::new(),
            rds_type: String::new(),
            engine: None,
            rds_pattern: String::new(),
            profile_filter: Some("vnc-".to_string()),
            env_port_mapping,
            default_port: "5900".to_string(),
            bastion_pattern: None,
            connection_type: "service".to_string(),
            service_type: Some("vnc".to_string()),
            remote_port: Some(5900),
            target_type: Some("ec2-direct".to_string()),
            target_pattern: Some("*vnc-server*".to_string()),
            ecs_cluster: None,
            ecs_service: None,
            secret_path: None,
            secret_username_field: None,
            secret_password_field: None,
            auth_type: "secrets".to_string(),
            iam_username: None,
            multiplexed: None,
            ssh_username: None,
            ssh_key_path: None,
            environments: None,
            password_only: None,
        }
    }

    #[test]
    fn test_valid_service_vnc_ec2_direct() {
        let result = validate_project_config(&valid_service_config_ec2());
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_valid_service_rdp_ecs_bastion() {
        let mut config = valid_service_config_ec2();
        config.service_type = Some("rdp".to_string());
        config.target_type = Some("ecs-bastion".to_string());
        config.target_pattern = None;
        config.ecs_cluster = Some("my-cluster".to_string());
        config.ecs_service = Some("bastion-service".to_string());
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_service_missing_target_type() {
        let mut config = valid_service_config_ec2();
        config.target_type = None;
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("targetType")));
    }

    #[test]
    fn test_service_invalid_service_type() {
        let mut config = valid_service_config_ec2();
        config.service_type = Some("ftp".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("serviceType")));
    }

    #[test]
    fn test_service_missing_ecs_cluster_for_ecs_bastion() {
        let mut config = valid_service_config_ec2();
        config.target_type = Some("ecs-bastion".to_string());
        config.target_pattern = None;
        config.ecs_cluster = None;
        config.ecs_service = Some("bastion-service".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("ecsCluster")));
    }

    #[test]
    fn test_databases_valid() {
        let mut config = valid_config();
        config.databases = Some(vec!["db1".to_string(), "db2".to_string()]);
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_databases_empty_entry() {
        let mut config = valid_config();
        config.databases = Some(vec!["db1".to_string(), "".to_string()]);
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("databases[1]")));
    }

    #[test]
    fn test_databases_duplicate() {
        let mut config = valid_config();
        config.databases = Some(vec!["db1".to_string(), "db1".to_string()]);
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn test_databases_invalid_characters() {
        let mut config = valid_config();
        config.databases = Some(vec!["db;drop".to_string()]);
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("databases[0]")));
    }

    #[test]
    fn test_service_does_not_require_rds_fields() {
        let config = valid_service_config_ec2();
        // database, secretPrefix, rdsType, rdsPattern are all empty
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
        assert!(!result.messages().iter().any(|e| e.contains("database")));
        assert!(!result.messages().iter().any(|e| e.contains("secretPrefix")));
        assert!(!result.messages().iter().any(|e| e.contains("rdsType")));
        assert!(!result.messages().iter().any(|e| e.contains("rdsPattern")));
    }

    // --- Custom Secret Path tests ---

    #[test]
    fn test_secret_path_valid() {
        let mut config = valid_config();
        config.secret_path = Some("arn:aws:secretsmanager:us-east-1:123456789012:secret:my-secret".to_string());
        // secretPrefix can be empty when secretPath is set
        config.secret_prefix = String::new();
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_secret_path_empty_string_invalid() {
        let mut config = valid_config();
        config.secret_path = Some(String::new());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretPath")));
    }

    #[test]
    fn test_secret_path_shell_unsafe_invalid() {
        let mut config = valid_config();
        config.secret_path = Some("bad;secret".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretPath")));
    }

    #[test]
    fn test_secret_username_field_valid() {
        let mut config = valid_config();
        config.secret_username_field = Some("credentials.user".to_string());
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_secret_username_field_empty_invalid() {
        let mut config = valid_config();
        config.secret_username_field = Some(String::new());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretUsernameField")));
    }

    #[test]
    fn test_secret_password_field_empty_invalid() {
        let mut config = valid_config();
        config.secret_password_field = Some(String::new());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretPasswordField")));
    }

    #[test]
    fn test_secret_field_invalid_characters() {
        let mut config = valid_config();
        config.secret_username_field = Some("user;name".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretUsernameField")));
    }

    // --- IAM Auth tests ---

    #[test]
    fn test_auth_type_iam_valid() {
        let mut config = valid_config();
        config.auth_type = "iam".to_string();
        config.iam_username = Some("db_admin".to_string());
        config.secret_prefix = String::new(); // not required for IAM
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_auth_type_iam_missing_username() {
        let mut config = valid_config();
        config.auth_type = "iam".to_string();
        config.iam_username = None;
        config.secret_prefix = String::new();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("iamUsername")));
    }

    #[test]
    fn test_auth_type_invalid() {
        let mut config = valid_config();
        config.auth_type = "invalid".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("authType")));
    }

    #[test]
    fn test_auth_type_secrets_requires_secret_prefix() {
        let mut config = valid_config();
        config.auth_type = "secrets".to_string();
        config.secret_prefix = String::new();
        config.secret_path = None;
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("secretPrefix")));
    }

    #[test]
    fn test_auth_type_secrets_with_secret_path_no_prefix_ok() {
        let mut config = valid_config();
        config.auth_type = "secrets".to_string();
        config.secret_prefix = String::new();
        config.secret_path = Some("my-direct-secret".to_string());
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_auth_type_iam_does_not_require_secret_prefix() {
        let mut config = valid_config();
        config.auth_type = "iam".to_string();
        config.iam_username = Some("admin".to_string());
        config.secret_prefix = String::new();
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
        assert!(!result.messages().iter().any(|e| e.contains("secretPrefix")));
    }

    // --- SSH tests ---

    fn valid_ssh_config() -> ProjectConfig {
        let mut env_port_mapping = HashMap::new();
        env_port_mapping.insert("dev".to_string(), "2222".to_string());

        ProjectConfig {
            name: "SSH Tunnel".to_string(),
            region: "us-east-1".to_string(),
            database: String::new(),
            databases: None,
            secret_prefix: String::new(),
            rds_type: String::new(),
            engine: None,
            rds_pattern: String::new(),
            profile_filter: Some("ssh-".to_string()),
            env_port_mapping,
            default_port: "2222".to_string(),
            bastion_pattern: None,
            connection_type: "service".to_string(),
            service_type: Some("ssh".to_string()),
            remote_port: Some(22),
            target_type: Some("ec2-bastion".to_string()),
            target_pattern: Some("*my-server*".to_string()),
            ecs_cluster: None,
            ecs_service: None,
            secret_path: None,
            secret_username_field: None,
            secret_password_field: None,
            auth_type: "secrets".to_string(),
            iam_username: None,
            multiplexed: None,
            ssh_username: Some("ubuntu".to_string()),
            ssh_key_path: Some("~/.ssh/id_rsa".to_string()),
            environments: None,
            password_only: None,
        }
    }

    #[test]
    fn test_valid_ssh_service() {
        let result = validate_project_config(&valid_ssh_config());
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_ssh_default_username() {
        let mut config = valid_ssh_config();
        config.ssh_username = None;
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
    }

    #[test]
    fn test_ssh_invalid_username() {
        let mut config = valid_ssh_config();
        config.ssh_username = Some("user;rm -rf /".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("sshUsername")));
    }

    #[test]
    fn test_ssh_invalid_key_path() {
        let mut config = valid_ssh_config();
        config.ssh_key_path = Some("/path/to/key;bad".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.messages().iter().any(|e| e.contains("sshKeyPath")));
    }

    #[test]
    fn test_ssh_valid_key_path_patterns() {
        for path in ["~/.ssh/id_rsa", "~/.ssh/id_ed25519", "/home/user/.ssh/key"] {
            let mut config = valid_ssh_config();
            config.ssh_key_path = Some(path.to_string());
            let result = validate_project_config(&config);
            assert!(result.valid, "expected valid for path: {}", path);
        }
    }

    fn env(name: &str, account: &str, port: &str) -> crate::config::projects::Environment {
        crate::config::projects::Environment {
            name: name.to_string(),
            account_id: account.to_string(),
            port: port.to_string(),
            rds_pattern: None,
            region: None,
            target_pattern: None,
            ecs_cluster: None,
            ecs_service: None,
            bastion_pattern: None,
        }
    }

    fn account_config(envs: Vec<crate::config::projects::Environment>) -> ProjectConfig {
        let mut c = valid_config();
        c.env_port_mapping.clear();
        c.profile_filter = Some("__connection-app-3.8__".to_string());
        c.environments = Some(envs);
        c
    }

    fn fields(r: &ValidationResult) -> Vec<&str> {
        r.errors.iter().map(|e| e.field.as_str()).collect()
    }

    #[test]
    fn bad_account_id_points_at_field() {
        let c = account_config(vec![env("dev", "07381380752", "5460")]);
        let r = validate_project_config(&c);
        assert!(!r.valid);
        assert_eq!(fields(&r), vec!["environments[0].accountId"]);
    }

    #[test]
    fn duplicate_env_names_and_ports_within_project() {
        let c = account_config(vec![
            env("dev", "073813807520", "5460"),
            env("dev", "073813807521", "5460"),
        ]);
        let r = validate_project_config(&c);
        assert_eq!(
            fields(&r),
            vec!["environments[1].name", "environments[1].port"]
        );
    }

    #[test]
    fn same_port_in_two_projects_is_valid() {
        let a = account_config(vec![env("dev", "073813807520", "5460")]);
        let b = account_config(vec![env("dev", "073813807521", "5460")]);
        assert!(validate_project_config(&a).valid);
        assert!(validate_project_config(&b).valid);
    }

    #[test]
    fn password_only_with_iam_is_invalid() {
        let mut c = account_config(vec![env("dev", "073813807520", "5460")]);
        c.auth_type = "iam".to_string();
        c.iam_username = Some("u".to_string());
        c.password_only = Some(true);
        let r = validate_project_config(&c);
        assert_eq!(fields(&r), vec!["passwordOnly"]);
        c.password_only = Some(false);
        assert!(validate_project_config(&c).valid);
    }

    #[test]
    fn env_name_with_space_is_rejected() {
        let c = account_config(vec![env("my env", "073813807520", "5460")]);
        let r = validate_project_config(&c);
        assert_eq!(fields(&r), vec!["environments[0].name"]);
    }

    #[test]
    fn existing_rules_carry_field_names() {
        let mut c = valid_config();
        c.region = String::new();
        let r = validate_project_config(&c);
        assert_eq!(fields(&r), vec!["region"]);
        assert_eq!(r.messages(), vec!["region: Missing required field: region"]);
    }

    #[test]
    fn non_legacy_project_skips_env_port_mapping() {
        let mut c = account_config(vec![env("dev", "073813807520", "5460")]);
        c.env_port_mapping.insert("x".to_string(), "bad".to_string());
        assert!(validate_project_config(&c).valid);
    }

    #[test]
    fn environment_overrides_use_project_patterns() {
        let mut e = env("dev", "073813807520", "5460");
        e.rds_pattern = Some("bad;pat".to_string());
        e.region = Some("nope".to_string());
        e.bastion_pattern = Some("a b".to_string());
        let r = validate_project_config(&account_config(vec![e]));
        assert_eq!(
            fields(&r),
            vec![
                "environments[0].rdsPattern",
                "environments[0].region",
                "environments[0].bastionPattern"
            ]
        );
    }
}
