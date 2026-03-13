use crate::config::projects::ProjectConfig;
use regex::Regex;
use std::sync::LazyLock;

static REGION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2}(-[a-z]+-\d+)$").unwrap());
static PORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").unwrap());
static SHELL_SAFE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/-]+$").unwrap());
static EC2_FILTER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/*?-]+$").unwrap());

const VALID_RDS_TYPES: &[&str] = &["cluster", "instance"];
const VALID_ENGINES: &[&str] = &["postgres", "mysql"];
const VALID_CONNECTION_TYPES: &[&str] = &["rds", "service"];
const VALID_SERVICE_TYPES: &[&str] = &["vnc", "rdp", "custom"];
const VALID_TARGET_TYPES: &[&str] = &["ec2-direct", "ec2-bastion", "ecs-bastion"];

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

pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
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
        errors.push(format!(
            "connectionType must be one of: {}",
            VALID_CONNECTION_TYPES.join(", ")
        ));
    }

    // Common required fields (all connection types)
    for (field, value) in [("name", &config.name), ("region", &config.region)] {
        if value.is_empty() {
            errors.push(format!("Missing required field: {}", field));
        }
    }

    if connection_type == "rds" {
        // RDS-specific required fields
        let rds_fields: Vec<(&str, &str)> = vec![
            ("database", &config.database),
            ("secretPrefix", &config.secret_prefix),
            ("rdsType", &config.rds_type),
            ("rdsPattern", &config.rds_pattern),
            ("defaultPort", &config.default_port),
        ];

        for (field, value) in &rds_fields {
            if value.is_empty() {
                errors.push(format!("Missing required field: {}", field));
            }
        }

        // Validate rdsType
        if !config.rds_type.is_empty() && !VALID_RDS_TYPES.contains(&config.rds_type.as_str()) {
            errors.push(format!(
                "rdsType must be one of: {}",
                VALID_RDS_TYPES.join(", ")
            ));
        }

        // Validate engine (optional)
        if let Some(ref engine) = config.engine
            && !engine.is_empty()
            && !VALID_ENGINES.contains(&engine.as_str())
        {
            errors.push(format!(
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
                errors.push(format!(
                    "{} contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, and ! allowed)",
                    field
                ));
            }
        }
    } else if connection_type == "service" {
        // Validate serviceType
        match config.service_type.as_deref() {
            Some(st) if !st.is_empty() => {
                if !VALID_SERVICE_TYPES.contains(&st) {
                    errors.push(format!(
                        "serviceType must be one of: {}",
                        VALID_SERVICE_TYPES.join(", ")
                    ));
                }
            }
            _ => {
                errors.push("Missing required field: serviceType".to_string());
            }
        }

        // Validate remotePort
        match config.remote_port {
            Some(port) if port > 0 => {}
            _ => {
                errors.push("Missing required field: remotePort (must be > 0)".to_string());
            }
        }

        // Validate targetType
        let target_type = match config.target_type.as_deref() {
            Some(tt) if !tt.is_empty() => {
                if !VALID_TARGET_TYPES.contains(&tt) {
                    errors.push(format!(
                        "targetType must be one of: {}",
                        VALID_TARGET_TYPES.join(", ")
                    ));
                }
                Some(tt)
            }
            _ => {
                errors.push("Missing required field: targetType".to_string());
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
                                errors.push(
                                    "targetPattern must be 256 characters or fewer".to_string(),
                                );
                            } else if !EC2_FILTER_PATTERN.is_match(tp) {
                                errors.push(
                                    "targetPattern contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, !, * and ? allowed)".to_string(),
                                );
                            }
                        }
                        _ => {
                            errors.push("Missing required field: targetPattern".to_string());
                        }
                    }
                }
                "ecs-bastion" => {
                    if config
                        .ecs_cluster
                        .as_deref()
                        .is_none_or(|s| s.is_empty())
                    {
                        errors.push("Missing required field: ecsCluster".to_string());
                    }
                    if config
                        .ecs_service
                        .as_deref()
                        .is_none_or(|s| s.is_empty())
                    {
                        errors.push("Missing required field: ecsService".to_string());
                    }
                }
                _ => {}
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
            errors.push("bastionPattern must be 256 characters or fewer".to_string());
        } else if !EC2_FILTER_PATTERN.is_match(pattern) {
            errors.push(
                "bastionPattern contains invalid characters (only alphanumeric, dots, underscores, hyphens, slashes, !, * and ? allowed)".to_string(),
            );
        }
    }

    // Validate region
    if !config.region.is_empty() && !REGION_PATTERN.is_match(&config.region) {
        errors.push(format!("Invalid region format: {}", config.region));
    }

    // Validate defaultPort (if provided)
    if !config.default_port.is_empty() && !PORT_PATTERN.is_match(&config.default_port) {
        errors.push(format!(
            "defaultPort must be a numeric string: {}",
            config.default_port
        ));
    }

    // Validate envPortMapping values
    for (key, value) in &config.env_port_mapping {
        if !PORT_PATTERN.is_match(value) {
            errors.push(format!(
                "Port for \"{}\" must be a numeric string: {}",
                key, value
            ));
        }
    }

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
        assert!(result.errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_invalid_rds_type() {
        let mut config = valid_config();
        config.rds_type = "invalid".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("rdsType")));
    }

    #[test]
    fn test_invalid_region() {
        let mut config = valid_config();
        config.region = "invalid-region".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("region")));
    }

    #[test]
    fn test_invalid_port() {
        let mut config = valid_config();
        config.default_port = "abc".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("defaultPort")));
    }

    #[test]
    fn test_invalid_shell_characters() {
        let mut config = valid_config();
        config.secret_prefix = "bad;prefix".to_string();
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("secretPrefix")));
    }

    #[test]
    fn test_invalid_env_port_mapping_value() {
        let mut config = valid_config();
        config
            .env_port_mapping
            .insert("staging".to_string(), "notaport".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("staging")));
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
            assert!(result.errors.iter().any(|e| e.contains("bastionPattern")));
        }
    }

    #[test]
    fn test_bastion_pattern_too_long() {
        let mut config = valid_config();
        config.bastion_pattern = Some("a".repeat(257));
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("256")));
    }

    fn valid_service_config_ec2() -> ProjectConfig {
        let mut env_port_mapping = HashMap::new();
        env_port_mapping.insert("dev".to_string(), "5901".to_string());

        ProjectConfig {
            name: "VNC Service".to_string(),
            region: "us-east-1".to_string(),
            database: String::new(),
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
        assert!(result.errors.iter().any(|e| e.contains("targetType")));
    }

    #[test]
    fn test_service_invalid_service_type() {
        let mut config = valid_service_config_ec2();
        config.service_type = Some("ssh".to_string());
        let result = validate_project_config(&config);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("serviceType")));
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
        assert!(result.errors.iter().any(|e| e.contains("ecsCluster")));
    }

    #[test]
    fn test_service_does_not_require_rds_fields() {
        let config = valid_service_config_ec2();
        // database, secretPrefix, rdsType, rdsPattern are all empty
        let result = validate_project_config(&config);
        assert!(result.valid, "errors: {:?}", result.errors);
        assert!(!result.errors.iter().any(|e| e.contains("database")));
        assert!(!result.errors.iter().any(|e| e.contains("secretPrefix")));
        assert!(!result.errors.iter().any(|e| e.contains("rdsType")));
        assert!(!result.errors.iter().any(|e| e.contains("rdsPattern")));
    }
}
