use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub name: String,
    pub region: String,
    pub database: String,
    #[serde(rename = "secretPrefix")]
    pub secret_prefix: String,
    #[serde(rename = "rdsType")]
    pub rds_type: String,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(rename = "rdsPattern")]
    pub rds_pattern: String,
    #[serde(rename = "profileFilter")]
    pub profile_filter: Option<String>,
    #[serde(rename = "envPortMapping")]
    pub env_port_mapping: HashMap<String, String>,
    #[serde(rename = "defaultPort")]
    pub default_port: String,
}

fn get_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".rds-ssm-connect")
        .join("projects.json")
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
