use crate::config::accounts::{self, ProfileAccount};
use crate::config::aws_config::{self, AwsProfile};
use crate::config::preferences;
use crate::error::AppError;

const DEFAULT_DETECT_REGION: &str = "us-east-1";

fn check_account_id(account_id: &str) -> Result<(), AppError> {
    if account_id.len() == 12 && account_id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "AWS account ID must be exactly 12 digits: '{}'",
            account_id
        )))
    }
}

/// An unsaved profile for `account_id`, with the SSO settings of the first
/// SSO profile in the config.
pub(crate) fn profile_template(
    profiles: &[AwsProfile],
    account_id: &str,
) -> Result<AwsProfile, AppError> {
    check_account_id(account_id)?;
    let sso = profiles.iter().find(|p| p.sso_start_url.is_some());
    Ok(AwsProfile {
        name: String::new(),
        region: None,
        source_profile: None,
        role_arn: None,
        mfa_serial: None,
        sso_start_url: sso.and_then(|p| p.sso_start_url.clone()),
        sso_region: sso.and_then(|p| p.sso_region.clone()),
        sso_account_id: Some(account_id.to_string()),
        sso_role_name: sso.and_then(|p| p.sso_role_name.clone()),
        raw_content: String::new(),
    })
}

/// Every profile with its AWS account, when known.
#[tauri::command]
pub async fn list_aws_accounts() -> Result<Vec<ProfileAccount>, AppError> {
    let profiles = aws_config::read_aws_config().await?;
    let cache = accounts::load_account_cache().await;
    Ok(accounts::profile_accounts(&profiles, &cache))
}

/// Remember which profile to use for an account.
#[tauri::command]
pub async fn set_account_profile(account_id: String, profile: String) -> Result<(), AppError> {
    check_account_id(&account_id)?;
    if profile.trim().is_empty() {
        return Err(AppError::Config("Profile name is empty".to_string()));
    }
    let mut prefs = preferences::load_preferences().await;
    preferences::set_account_profile(&mut prefs, &account_id, &profile);
    preferences::save_preferences(&prefs).await;
    Ok(())
}

/// Ask AWS which account a profile signs in to, and cache the answer.
#[tauri::command]
pub async fn detect_profile_account(profile: String) -> Result<String, AppError> {
    let profiles = aws_config::read_aws_config().await?;
    let found = profiles
        .iter()
        .find(|p| p.name == profile)
        .ok_or_else(|| AppError::Config(format!("Unknown AWS profile: {}", profile)))?;
    let region = found
        .region
        .as_deref()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or(DEFAULT_DETECT_REGION);
    crate::aws::operations::detect_profile_account(&profile, region).await
}

/// A pre-filled, unsaved profile for an account. Nothing is written.
#[tauri::command]
pub async fn aws_profile_template(account_id: String) -> Result<AwsProfile, AppError> {
    let profiles = aws_config::read_aws_config().await?;
    profile_template(&profiles, &account_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> AwsProfile {
        AwsProfile {
            name: name.to_string(),
            region: Some("us-west-1".to_string()),
            source_profile: None,
            role_arn: None,
            mfa_serial: None,
            sso_start_url: None,
            sso_region: None,
            sso_account_id: None,
            sso_role_name: None,
            raw_content: format!("[profile {name}]\n"),
        }
    }

    #[test]
    fn template_copies_sso_settings_from_the_first_sso_profile() {
        let mut first = profile("first-sso");
        first.sso_start_url = Some("https://d-9067000000.awsapps.com/start".to_string());
        first.sso_region = Some("us-east-1".to_string());
        first.sso_role_name = Some("AdministratorAccess".to_string());
        first.sso_account_id = Some("111111111111".to_string());
        let mut second = profile("second-sso");
        second.sso_start_url = Some("https://other.awsapps.com/start".to_string());
        let profiles = vec![profile("static-keys"), first, second];

        let t = profile_template(&profiles, "073813807522").unwrap();
        assert_eq!(t.name, "");
        assert_eq!(t.sso_account_id.as_deref(), Some("073813807522"));
        assert_eq!(
            t.sso_start_url.as_deref(),
            Some("https://d-9067000000.awsapps.com/start")
        );
        assert_eq!(t.sso_region.as_deref(), Some("us-east-1"));
        assert_eq!(t.sso_role_name.as_deref(), Some("AdministratorAccess"));
        assert_eq!(t.region, None);
        assert_eq!(t.raw_content, "");
    }

    #[test]
    fn template_without_sso_profiles_has_only_the_account() {
        let t = profile_template(&[profile("static-keys")], "073813807522").unwrap();
        assert_eq!(t.sso_account_id.as_deref(), Some("073813807522"));
        assert_eq!(t.sso_start_url, None);
    }

    #[test]
    fn template_rejects_a_malformed_account() {
        assert!(profile_template(&[], "07381380752").is_err());
        assert!(profile_template(&[], "07381380752x").is_err());
    }
}
