use crate::config::aws_config::AwsProfile;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

static ROLE_ARN_ACCOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^arn:aws[a-z-]*:iam::(\d{12}):").unwrap());

fn is_account_id(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| b.is_ascii_digit())
}

/// The AWS account a profile points at, when its config says so.
pub fn account_from_profile(p: &AwsProfile) -> Option<String> {
    if let Some(id) = p.sso_account_id.as_deref().map(str::trim)
        && is_account_id(id)
    {
        return Some(id.to_string());
    }
    p.role_arn
        .as_deref()
        .and_then(|arn| ROLE_ARN_ACCOUNT.captures(arn.trim()))
        .map(|c| c[1].to_string())
}

/// Lowercase hex SHA-256 of the profile's raw config section.
pub fn profile_fingerprint(p: &AwsProfile) -> String {
    Sha256::digest(p.raw_content.as_bytes())
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct AccountCache {
    #[serde(default)]
    pub entries: HashMap<String, CachedAccount>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedAccount {
    pub account_id: String,
    pub fingerprint: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileAccount {
    pub profile: String,
    pub account_id: Option<String>,
}

fn cache_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".connection-app")
        .join("profile-accounts.json")
}

pub async fn load_account_cache() -> AccountCache {
    match tokio::fs::read_to_string(cache_path()).await {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => AccountCache::default(),
    }
}

pub async fn save_account_cache(c: &AccountCache) {
    let path = cache_path();

    if let Some(parent) = path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        log::warn!("Failed to create account cache directory: {}", e);
        return;
    }

    match serde_json::to_string_pretty(c) {
        Ok(json) => {
            // TODO(task4): use write_atomic
            if let Err(e) = tokio::fs::write(&path, format!("{}\n", json)).await {
                log::warn!("Failed to write account cache: {}", e);
            }
        }
        Err(e) => log::warn!("Failed to serialize account cache: {}", e),
    }
}

/// Account of every profile, in `~/.aws/config` order. A profile whose config
/// does not name its account falls back to the cache, if the profile has not
/// changed since the account was detected.
pub fn profile_accounts(profiles: &[AwsProfile], cache: &AccountCache) -> Vec<ProfileAccount> {
    profiles
        .iter()
        .map(|p| {
            let account_id = account_from_profile(p).or_else(|| {
                cache
                    .entries
                    .get(&p.name)
                    .filter(|e| e.fingerprint == profile_fingerprint(p))
                    .map(|e| e.account_id.clone())
            });
            ProfileAccount {
                profile: p.name.clone(),
                account_id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::aws_config::AwsProfile;

    fn profile(name: &str, raw: &str) -> AwsProfile {
        AwsProfile {
            name: name.to_string(),
            region: None,
            source_profile: None,
            role_arn: None,
            mfa_serial: None,
            sso_start_url: None,
            sso_region: None,
            sso_account_id: None,
            sso_role_name: None,
            raw_content: raw.to_string(),
        }
    }

    #[test]
    fn sso_account_id_wins() {
        let mut p = profile("a", "");
        p.sso_account_id = Some("073813807522".into());
        p.role_arn = Some("arn:aws:iam::111122223333:role/R".into());
        assert_eq!(account_from_profile(&p).as_deref(), Some("073813807522"));
    }

    #[test]
    fn sso_session_style_profile() {
        let mut p = profile("a", "");
        p.sso_account_id = Some("231030592327".into());
        p.sso_start_url = None;
        assert_eq!(account_from_profile(&p).as_deref(), Some("231030592327"));
    }

    #[test]
    fn role_arn_partitions() {
        for arn in [
            "arn:aws:iam::111122223333:role/R",
            "arn:aws-cn:iam::111122223333:role/R",
            "arn:aws-us-gov:iam::111122223333:role/R",
        ] {
            let mut p = profile("a", "");
            p.role_arn = Some(arn.into());
            assert_eq!(account_from_profile(&p).as_deref(), Some("111122223333"));
        }
    }

    #[test]
    fn unknown_account() {
        let mut p = profile("keys", "");
        p.sso_account_id = Some("12345".into());
        assert_eq!(account_from_profile(&p), None);
    }

    #[test]
    fn cache_used_only_when_fingerprint_matches() {
        let mut cache = AccountCache::default();
        cache.entries.insert(
            "keys".into(),
            CachedAccount {
                account_id: "444455556666".into(),
                fingerprint: profile_fingerprint(&profile("keys", "A")),
            },
        );
        let hit = profile_accounts(&[profile("keys", "A")], &cache);
        assert_eq!(hit[0].account_id.as_deref(), Some("444455556666"));
        let miss = profile_accounts(&[profile("keys", "B")], &cache);
        assert_eq!(miss[0].account_id, None);
    }

    #[test]
    fn profile_accounts_keeps_config_order() {
        let ps = [profile("z", "1"), profile("a", "2"), profile("m", "3")];
        let out = profile_accounts(&ps, &AccountCache::default());
        let names: Vec<_> = out.iter().map(|p| p.profile.as_str()).collect();
        assert_eq!(names, ["z", "a", "m"]);
    }
}
