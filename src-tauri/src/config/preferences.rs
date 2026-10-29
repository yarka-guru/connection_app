use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Preferences {
    /// Map of "project_key:profile" → preferred bastion instance ID
    #[serde(default)]
    pub bastion_preferences: HashMap<String, String>,
    /// Map of AWS account ID → profile chosen for it
    #[serde(default)]
    pub account_profiles: HashMap<String, String>,
}

fn preferences_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".connection-app")
        .join("preferences.json")
}

pub async fn load_preferences() -> Preferences {
    let path = preferences_path();

    match tokio::fs::read_to_string(&path).await {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Preferences::default(),
    }
}

pub async fn save_preferences(prefs: &Preferences) {
    let path = preferences_path();

    if let Some(parent) = path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        log::warn!("Failed to create preferences directory: {}", e);
        return;
    }

    match serde_json::to_string_pretty(prefs) {
        Ok(json) => {
            if let Err(e) = tokio::fs::write(&path, format!("{}\n", json)).await {
                log::warn!("Failed to write preferences: {}", e);
            }
        }
        Err(e) => {
            log::warn!("Failed to serialize preferences: {}", e);
        }
    }
}

pub fn get_preferred_bastion<'a>(
    prefs: &'a Preferences,
    project_key: &str,
    profile: &str,
) -> Option<&'a str> {
    let key = format!("{}:{}", project_key, profile);
    prefs.bastion_preferences.get(&key).map(|s| s.as_str())
}

pub fn set_preferred_bastion(
    prefs: &mut Preferences,
    project_key: &str,
    profile: &str,
    instance_id: &str,
) {
    let key = format!("{}:{}", project_key, profile);
    prefs
        .bastion_preferences
        .insert(key, instance_id.to_string());
}

pub fn get_account_profile<'a>(p: &'a Preferences, account_id: &str) -> Option<&'a str> {
    p.account_profiles.get(account_id).map(|s| s.as_str())
}

pub fn set_account_profile(p: &mut Preferences, account_id: &str, profile: &str) {
    p.account_profiles
        .insert(account_id.to_string(), profile.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_profile_round_trip() {
        let mut p = Preferences::default();
        assert_eq!(get_account_profile(&p, "111111111111"), None);
        set_account_profile(&mut p, "111111111111", "a");
        set_account_profile(&mut p, "111111111111", "b");
        assert_eq!(get_account_profile(&p, "111111111111"), Some("b"));
    }

    #[test]
    fn old_preferences_file_still_loads() {
        let p: Preferences =
            serde_json::from_str(r#"{"bastion_preferences":{"k:p":"i-1"}}"#).unwrap();
        assert!(p.account_profiles.is_empty());
        assert_eq!(p.bastion_preferences.len(), 1);
    }
}
