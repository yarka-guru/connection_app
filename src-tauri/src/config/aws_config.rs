use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AwsProfile {
    pub name: String,
    pub region: Option<String>,
    #[serde(rename = "sourceProfile")]
    pub source_profile: Option<String>,
    #[serde(rename = "roleArn")]
    pub role_arn: Option<String>,
    #[serde(rename = "mfaSerial")]
    pub mfa_serial: Option<String>,
    #[serde(rename = "ssoStartUrl")]
    pub sso_start_url: Option<String>,
    #[serde(rename = "ssoRegion")]
    pub sso_region: Option<String>,
    #[serde(rename = "ssoAccountId")]
    pub sso_account_id: Option<String>,
    #[serde(rename = "ssoRoleName")]
    pub sso_role_name: Option<String>,
    #[serde(rename = "rawContent")]
    pub raw_content: String,
}

/// Get the AWS directory path (~/.aws/).
/// Respects AWS_CONFIG_FILE env var (returns its parent directory).
pub fn get_aws_dir() -> PathBuf {
    if let Ok(config_path) = std::env::var("AWS_CONFIG_FILE") {
        if let Some(parent) = PathBuf::from(&config_path).parent() {
            return parent.to_path_buf();
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aws")
}

pub fn get_aws_config_path() -> PathBuf {
    if let Ok(config_path) = std::env::var("AWS_CONFIG_FILE") {
        return PathBuf::from(config_path);
    }
    get_aws_dir().join("config")
}

/// Read AWS config and return structured AwsProfile list (for settings UI).
pub async fn read_aws_config() -> Result<Vec<AwsProfile>, AppError> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(Vec::new());
    }

    let content = tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| AppError::Config(format!("Failed to read AWS config: {}", e)))?;

    let mut profiles = Vec::new();
    let mut current_profile: Option<String> = None;
    let mut current_content = String::new();
    let mut current_region = None;
    let mut current_source_profile = None;
    let mut current_role_arn = None;
    let mut current_mfa_serial = None;
    let mut current_sso_start_url = None;
    let mut current_sso_region = None;
    let mut current_sso_account_id = None;
    let mut current_sso_role_name = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(name) = current_profile.take() {
                profiles.push(AwsProfile {
                    name,
                    region: current_region.take(),
                    source_profile: current_source_profile.take(),
                    role_arn: current_role_arn.take(),
                    mfa_serial: current_mfa_serial.take(),
                    sso_start_url: current_sso_start_url.take(),
                    sso_region: current_sso_region.take(),
                    sso_account_id: current_sso_account_id.take(),
                    sso_role_name: current_sso_role_name.take(),
                    raw_content: current_content.trim().to_string(),
                });
                current_content = String::new();
            }

            let section = &trimmed[1..trimmed.len() - 1];
            let profile_name = if let Some(stripped) = section.strip_prefix("profile ") {
                stripped.to_string()
            } else if section == "default" {
                "default".to_string()
            } else {
                section.to_string()
            };
            current_profile = Some(profile_name);
        } else if current_profile.is_some() && !trimmed.is_empty() {
            current_content.push_str(line);
            current_content.push('\n');

            if !trimmed.starts_with('#') && !trimmed.starts_with(';') {
                if let Some((key, value)) = trimmed.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "region" => current_region = Some(value.to_string()),
                        "source_profile" => current_source_profile = Some(value.to_string()),
                        "role_arn" => current_role_arn = Some(value.to_string()),
                        "mfa_serial" => current_mfa_serial = Some(value.to_string()),
                        "sso_start_url" => current_sso_start_url = Some(value.to_string()),
                        "sso_region" => current_sso_region = Some(value.to_string()),
                        "sso_account_id" => current_sso_account_id = Some(value.to_string()),
                        "sso_role_name" => current_sso_role_name = Some(value.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }

    if let Some(name) = current_profile {
        profiles.push(AwsProfile {
            name,
            region: current_region,
            source_profile: current_source_profile,
            role_arn: current_role_arn,
            mfa_serial: current_mfa_serial,
            sso_start_url: current_sso_start_url,
            sso_region: current_sso_region,
            sso_account_id: current_sso_account_id,
            sso_role_name: current_sso_role_name,
            raw_content: current_content.trim().to_string(),
        });
    }

    Ok(profiles)
}

/// Simple parser: returns HashMap<profile_name, HashMap<key, value>>
/// Used by credential resolution and SSO modules.
pub async fn parse_aws_config() -> HashMap<String, HashMap<String, String>> {
    let config_path = get_aws_config_path();

    let content = match tokio::fs::read_to_string(&config_path).await {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let mut profiles: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_profile: Option<String> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let mut section_name = line[1..line.len() - 1].trim().to_string();
            if let Some(stripped) = section_name.strip_prefix("profile ") {
                section_name = stripped.trim().to_string();
            }
            current_profile = Some(section_name.clone());
            profiles.entry(section_name).or_default();
            continue;
        }

        if let Some(ref profile) = current_profile {
            if let Some(eq_index) = line.find('=') {
                let key = line[..eq_index].trim().to_string();
                let value = line[eq_index + 1..].trim().to_string();
                if let Some(map) = profiles.get_mut(profile) {
                    map.insert(key, value);
                }
            }
        }
    }

    profiles
}

/// Read AWS config profile names (simple list for project/profile matching).
pub async fn read_aws_profile_names() -> Vec<String> {
    let config_path = get_aws_config_path();

    let content = match tokio::fs::read_to_string(&config_path).await {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('[') && l.ends_with(']'))
        .map(|l| {
            let section = &l[1..l.len() - 1];
            section
                .strip_prefix("profile ")
                .unwrap_or(section)
                .trim()
                .to_string()
        })
        .collect()
}

/// Match a section name against a profile name.
fn section_matches_profile(section: &str, profile_name: &str) -> bool {
    if profile_name == "default" {
        section == "default"
    } else {
        section == format!("profile {}", profile_name) || section == profile_name
    }
}

pub async fn save_aws_profile(profile: AwsProfile) -> Result<(), AppError> {
    let config_path = get_aws_config_path();

    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Config(format!("Failed to create .aws directory: {}", e)))?;
    }

    let existing_content = if tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        tokio::fs::read_to_string(&config_path)
            .await
            .map_err(|e| AppError::Config(format!("Failed to read AWS config: {}", e)))?
    } else {
        String::new()
    };

    let mut new_content = String::new();
    let mut found = false;
    let mut in_target_profile = false;
    let profile_header = if profile.name == "default" {
        "[default]".to_string()
    } else {
        format!("[profile {}]", profile.name)
    };

    for line in existing_content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if in_target_profile {
                in_target_profile = false;
            }

            let section = &trimmed[1..trimmed.len() - 1];
            if section_matches_profile(section, &profile.name) {
                in_target_profile = true;
                found = true;
                new_content.push_str(&profile_header);
                new_content.push('\n');
                new_content.push_str(&profile.raw_content);
                new_content.push('\n');
                continue;
            }
        }

        if !in_target_profile {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    if !found {
        if !new_content.is_empty() && !new_content.ends_with("\n\n") {
            new_content.push('\n');
        }
        new_content.push_str(&profile_header);
        new_content.push('\n');
        new_content.push_str(&profile.raw_content);
        new_content.push('\n');
    }

    tokio::fs::write(&config_path, new_content.trim_end())
        .await
        .map_err(|e| AppError::Config(format!("Failed to write AWS config: {}", e)))?;

    Ok(())
}

pub async fn delete_aws_profile(profile_name: &str) -> Result<(), AppError> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(());
    }

    let content = tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| AppError::Config(format!("Failed to read AWS config: {}", e)))?;

    let mut new_content = String::new();
    let mut in_target_profile = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = &trimmed[1..trimmed.len() - 1];
            in_target_profile = section_matches_profile(section, profile_name);

            if !in_target_profile {
                new_content.push_str(line);
                new_content.push('\n');
            }
        } else if !in_target_profile {
            new_content.push_str(line);
            new_content.push('\n');
        }
    }

    tokio::fs::write(&config_path, new_content.trim_end())
        .await
        .map_err(|e| AppError::Config(format!("Failed to write AWS config: {}", e)))?;

    Ok(())
}

pub async fn get_raw_aws_config() -> Result<String, AppError> {
    let config_path = get_aws_config_path();

    if !tokio::fs::try_exists(&config_path).await.unwrap_or(false) {
        return Ok(String::new());
    }

    tokio::fs::read_to_string(&config_path)
        .await
        .map_err(|e| AppError::Config(format!("Failed to read AWS config: {}", e)))
}

pub async fn save_raw_aws_config(content: &str) -> Result<(), AppError> {
    let config_path = get_aws_config_path();

    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Config(format!("Failed to create .aws directory: {}", e)))?;
    }

    tokio::fs::write(&config_path, content)
        .await
        .map_err(|e| AppError::Config(format!("Failed to write AWS config: {}", e)))
}
