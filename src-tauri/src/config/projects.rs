use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

fn default_connection_type() -> String {
    "rds".to_string()
}

fn default_auth_type() -> String {
    "secrets".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub name: String,
    pub region: String,
    #[serde(default)]
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub databases: Option<Vec<String>>,
    #[serde(rename = "secretPrefix", default)]
    pub secret_prefix: String,
    #[serde(rename = "rdsType", default)]
    pub rds_type: String,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(rename = "rdsPattern", default)]
    pub rds_pattern: String,
    #[serde(rename = "profileFilter")]
    pub profile_filter: Option<String>,
    #[serde(rename = "envPortMapping")]
    pub env_port_mapping: HashMap<String, String>,
    #[serde(rename = "defaultPort")]
    pub default_port: String,
    #[serde(rename = "bastionPattern", default)]
    pub bastion_pattern: Option<String>,

    // Connection type: "rds" (default) or "service"
    #[serde(rename = "connectionType", default = "default_connection_type")]
    pub connection_type: String,

    // Service-specific fields (used when connectionType == "service")
    #[serde(rename = "serviceType", default)]
    pub service_type: Option<String>,
    #[serde(rename = "remotePort", default)]
    pub remote_port: Option<u16>,
    #[serde(rename = "targetType", default)]
    pub target_type: Option<String>,
    #[serde(rename = "targetPattern", default)]
    pub target_pattern: Option<String>,
    #[serde(rename = "ecsCluster", default)]
    pub ecs_cluster: Option<String>,
    #[serde(rename = "ecsService", default)]
    pub ecs_service: Option<String>,

    // Custom secret path fields
    /// Direct secret ARN or name (bypasses prefix-based search)
    #[serde(rename = "secretPath", default)]
    pub secret_path: Option<String>,
    /// JSON field name for username in secret (default: "username")
    #[serde(rename = "secretUsernameField", default)]
    pub secret_username_field: Option<String>,
    /// JSON field name for password in secret (default: "password")
    #[serde(rename = "secretPasswordField", default)]
    pub secret_password_field: Option<String>,

    // IAM authentication fields
    /// Authentication type: "secrets" (default) or "iam"
    #[serde(rename = "authType", default = "default_auth_type")]
    pub auth_type: String,
    /// IAM username for RDS IAM auth
    #[serde(rename = "iamUsername", default)]
    pub iam_username: Option<String>,

    // Multiplexing: enable smux protocol for multiple TCP connections per tunnel
    #[serde(default)]
    pub multiplexed: Option<bool>,

    // SSH-specific fields (used when serviceType == "ssh")
    #[serde(rename = "sshUsername", default)]
    pub ssh_username: Option<String>,
    #[serde(rename = "sshKeyPath", default)]
    pub ssh_key_path: Option<String>,
}

pub const DEFAULT_BASTION_PATTERN: &str = "*bastion*";

impl ProjectConfig {
    /// Returns the effective database name.
    /// If `selected_database` is provided (from multi-database selection), use it.
    /// Otherwise fall back to the single `database` field.
    pub fn effective_database<'a>(&'a self, selected_database: Option<&'a str>) -> &'a str {
        if let Some(db) = selected_database
            && !db.is_empty() {
                return db;
            }
        &self.database
    }

    /// Returns the bastion Name tag filter pattern, defaulting to `*bastion*`.
    pub fn bastion_pattern(&self) -> &str {
        self.bastion_pattern
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_BASTION_PATTERN)
    }
}

/// New config directory name.
const CONFIG_DIR: &str = ".connection-app";
/// Old config directory name (pre-v3.1 used this).
const LEGACY_CONFIG_DIR: &str = ".rds-ssm-connect";

fn get_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CONFIG_DIR)
}

fn get_config_path() -> PathBuf {
    get_config_dir().join("projects.json")
}

/// Migrate legacy `~/.rds-ssm-connect/` to `~/.connection-app/` if needed.
/// Copies (not moves) projects.json so the old CLI still works until the user
/// upgrades it too. Runs once — skips if the new directory already has a config.
pub fn migrate_legacy_config() {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let legacy_dir = home.join(LEGACY_CONFIG_DIR);
    let new_dir = home.join(CONFIG_DIR);
    let new_config = new_dir.join("projects.json");
    let legacy_config = legacy_dir.join("projects.json");

    // Skip if new config already exists or legacy doesn't exist
    if new_config.exists() || !legacy_config.exists() {
        return;
    }

    // Create new directory and copy projects.json
    if let Err(e) = std::fs::create_dir_all(&new_dir) {
        log::warn!("Failed to create {}: {}", new_dir.display(), e);
        return;
    }
    if let Err(e) = std::fs::copy(&legacy_config, &new_config) {
        log::warn!(
            "Failed to migrate {} → {}: {}",
            legacy_config.display(),
            new_config.display(),
            e
        );
        return;
    }
    log::info!(
        "Migrated config from {} → {}",
        legacy_dir.display(),
        new_dir.display()
    );
}

pub async fn load_project_configs() -> Result<HashMap<String, ProjectConfig>, AppError> {
    let path = get_config_path();

    match tokio::fs::read_to_string(&path).await {
        Ok(data) => {
            let configs: HashMap<String, ProjectConfig> =
                serde_json::from_str(&data).map_err(|e| AppError::Config(e.to_string()))?;
            Ok(configs)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(AppError::Config(format!(
            "Failed to read project configs: {}",
            e
        ))),
    }
}

async fn save_all_configs(configs: &HashMap<String, ProjectConfig>) -> Result<(), AppError> {
    let path = get_config_path();

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            AppError::Config(format!("Failed to create config directory: {}", e))
        })?;
    }

    let json = serde_json::to_string_pretty(configs)?;
    tokio::fs::write(&path, format!("{}\n", json))
        .await
        .map_err(|e| AppError::Config(format!("Failed to write project configs: {}", e)))?;

    Ok(())
}

pub async fn save_project_config(
    key: &str,
    config: ProjectConfig,
) -> Result<(), AppError> {
    let mut configs = load_project_configs().await?;
    configs.insert(key.to_string(), config);
    save_all_configs(&configs).await
}

pub async fn delete_project_config(key: &str) -> Result<(), AppError> {
    let mut configs = load_project_configs().await?;
    configs.remove(key);
    save_all_configs(&configs).await
}

/// Get profiles matching a project's filter from the full list of AWS profiles.
pub fn get_profiles_for_project(
    all_profiles: &[String],
    project_config: &ProjectConfig,
    all_project_configs: &HashMap<String, ProjectConfig>,
) -> Vec<String> {
    let filtered: Vec<&String> = if let Some(ref filter) = project_config.profile_filter {
        all_profiles
            .iter()
            .filter(|env| env.starts_with(filter.as_str()))
            .collect()
    } else {
        // No filter — return profiles that don't match any other project's filter
        let other_filters: Vec<&str> = all_project_configs
            .values()
            .filter_map(|config| config.profile_filter.as_deref())
            .collect();

        all_profiles
            .iter()
            .filter(|env| !other_filters.iter().any(|f| env.starts_with(f)))
            .collect()
    };

    // Further restrict to profiles matching an envPortMapping suffix
    if !project_config.env_port_mapping.is_empty() {
        let mut suffixes: Vec<&String> = project_config.env_port_mapping.keys().collect();
        suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));

        filtered
            .into_iter()
            .filter(|env| {
                suffixes
                    .iter()
                    .any(|suffix| env.ends_with(suffix.as_str()) || env.as_str() == suffix.as_str())
            })
            .cloned()
            .collect()
    } else {
        filtered.into_iter().cloned().collect()
    }
}

/// Get local port number based on environment suffix matching.
pub fn get_local_port(profile: &str, project_config: &ProjectConfig) -> String {
    let mut suffixes: Vec<&String> = project_config.env_port_mapping.keys().collect();
    suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let matched = suffixes.iter().find(|suffix| {
        profile.ends_with(suffix.as_str()) || profile == suffix.as_str()
    });

    match matched {
        Some(suffix) => project_config
            .env_port_mapping
            .get(suffix.as_str())
            .cloned()
            .unwrap_or_else(|| project_config.default_port.clone()),
        None => project_config.default_port.clone(),
    }
}

/// Get the default RDS port for the project's database engine.
pub fn get_default_port_for_engine(project_config: &ProjectConfig) -> String {
    match project_config.engine.as_deref() {
        Some("mysql") => "3306".to_string(),
        _ => "5432".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(bastion_pattern: Option<String>) -> ProjectConfig {
        ProjectConfig {
            name: "Test".to_string(),
            region: "us-east-1".to_string(),
            database: "db".to_string(),
            databases: None,
            secret_prefix: "rds!cluster".to_string(),
            rds_type: "cluster".to_string(),
            engine: None,
            rds_pattern: "pattern".to_string(),
            profile_filter: None,
            env_port_mapping: HashMap::new(),
            default_port: "5432".to_string(),
            bastion_pattern,
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
        }
    }

    #[test]
    fn test_bastion_pattern_default() {
        let config = test_config(None);
        assert_eq!(config.bastion_pattern(), "*bastion*");
    }

    #[test]
    fn test_bastion_pattern_custom() {
        let config = test_config(Some("*jump-box*".to_string()));
        assert_eq!(config.bastion_pattern(), "*jump-box*");
    }

    #[test]
    fn test_bastion_pattern_empty_string_falls_back() {
        let config = test_config(Some(String::new()));
        assert_eq!(config.bastion_pattern(), "*bastion*");
    }
}
