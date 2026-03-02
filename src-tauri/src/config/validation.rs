use crate::config::projects::ProjectConfig;
use regex::Regex;
use std::sync::LazyLock;

static REGION_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]{2}(-[a-z]+-\d+)$").unwrap());
static PORT_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").unwrap());
static SHELL_SAFE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9._!/-]+$").unwrap());

const VALID_RDS_TYPES: &[&str] = &["cluster", "instance"];
const VALID_ENGINES: &[&str] = &["postgres", "mysql"];

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

    // Check required fields
    let field_values: Vec<(&str, &str)> = vec![
        ("name", &config.name),
        ("region", &config.region),
        ("database", &config.database),
        ("secretPrefix", &config.secret_prefix),
        ("rdsType", &config.rds_type),
        ("rdsPattern", &config.rds_pattern),
        ("defaultPort", &config.default_port),
    ];

    for (field, value) in &field_values {
        if value.is_empty() {
            errors.push(format!("Missing required field: {}", field));
        }
    }

    // envPortMapping is required (checked as non-empty in JS; we check the field exists via struct type)
    let _ = REQUIRED_FIELDS; // keep reference for documentation

    // Validate rdsType
    if !config.rds_type.is_empty() && !VALID_RDS_TYPES.contains(&config.rds_type.as_str()) {
        errors.push(format!(
            "rdsType must be one of: {}",
            VALID_RDS_TYPES.join(", ")
        ));
    }

    // Validate engine (optional)
    if let Some(ref engine) = config.engine {
        if !engine.is_empty() && !VALID_ENGINES.contains(&engine.as_str()) {
            errors.push(format!(
                "engine must be one of: {}",
                VALID_ENGINES.join(", ")
            ));
        }
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

    // Validate region
    if !config.region.is_empty() && !REGION_PATTERN.is_match(&config.region) {
        errors.push(format!("Invalid region format: {}", config.region));
    }

    // Validate defaultPort
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
}
