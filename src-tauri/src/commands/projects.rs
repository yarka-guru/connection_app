use crate::config::projects::{self, ProjectConfig};
use crate::config::validation;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Project {
    pub key: String,
    pub name: String,
    #[serde(rename = "connectionType")]
    pub connection_type: String,
}

#[tauri::command]
pub async fn list_projects() -> Result<Vec<Project>, AppError> {
    let all_profiles = crate::config::aws_config::read_aws_profile_names().await;
    if all_profiles.is_empty() {
        return Ok(Vec::new());
    }

    let configs = projects::load_project_configs().await?;

    let result: Vec<Project> = configs
        .iter()
        .filter(|(_key, config)| {
            let matching =
                projects::get_profiles_for_project(&all_profiles, config, &configs);
            !matching.is_empty()
        })
        .map(|(key, config)| Project {
            key: key.clone(),
            name: config.name.clone(),
            connection_type: if config.connection_type.is_empty() {
                "rds".to_string()
            } else {
                config.connection_type.clone()
            },
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn list_project_configs() -> Result<HashMap<String, ProjectConfig>, AppError> {
    projects::load_project_configs().await
}

#[tauri::command]
pub async fn save_project_config(
    key: String,
    config: ProjectConfig,
) -> Result<(), AppError> {
    let result = validation::validate_project_config(&config);
    if !result.valid {
        return Err(AppError::Config(format!(
            "Validation failed: {}",
            result.errors.join(", ")
        )));
    }

    projects::save_project_config(&key, config).await
}

#[tauri::command]
pub async fn delete_project_config(key: String) -> Result<(), AppError> {
    projects::delete_project_config(&key).await
}
