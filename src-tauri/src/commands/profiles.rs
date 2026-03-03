use crate::config::aws_config::{self, AwsProfile};
use crate::config::projects;
use crate::error::AppError;

#[tauri::command]
pub async fn list_profiles(project_key: String) -> Result<Vec<String>, AppError> {
    let all_profiles = aws_config::read_aws_profile_names().await;
    let configs = projects::load_project_configs().await?;
    let project_config = configs
        .get(&project_key)
        .ok_or_else(|| AppError::Config(format!("Unknown project: {}", project_key)))?;

    Ok(projects::get_profiles_for_project(
        &all_profiles,
        project_config,
        &configs,
    ))
}

#[tauri::command]
pub async fn read_aws_config() -> Result<Vec<AwsProfile>, AppError> {
    aws_config::read_aws_config().await
}

#[tauri::command]
pub async fn save_aws_profile(profile: AwsProfile) -> Result<(), AppError> {
    aws_config::save_aws_profile(profile).await
}

#[tauri::command]
pub async fn delete_aws_profile(profile_name: String) -> Result<(), AppError> {
    aws_config::delete_aws_profile(&profile_name).await
}

#[tauri::command]
pub async fn get_raw_aws_config() -> Result<String, AppError> {
    aws_config::get_raw_aws_config().await
}

#[tauri::command]
pub async fn save_raw_aws_config(content: String) -> Result<(), AppError> {
    aws_config::save_raw_aws_config(&content).await
}
