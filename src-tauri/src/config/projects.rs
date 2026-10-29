use crate::config::project_file;
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

/// An environment is one AWS account of a project, with optional per-account
/// overrides of the project-level settings.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Environment {
    pub name: String,
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub port: String,
    #[serde(rename = "rdsPattern", default, skip_serializing_if = "Option::is_none")]
    pub rds_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(rename = "targetPattern", default, skip_serializing_if = "Option::is_none")]
    pub target_pattern: Option<String>,
    #[serde(rename = "ecsCluster", default, skip_serializing_if = "Option::is_none")]
    pub ecs_cluster: Option<String>,
    #[serde(rename = "ecsService", default, skip_serializing_if = "Option::is_none")]
    pub ecs_service: Option<String>,
    #[serde(rename = "bastionPattern", default, skip_serializing_if = "Option::is_none")]
    pub bastion_pattern: Option<String>,
}

/// How database credentials are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Auto,
    IamOnly,
    PasswordOnly,
}

/// `profileFilter` value written for new projects so 3.7.x lists no profiles for them.
pub const HIDE_FROM_LEGACY_FILTER: &str = "__connection-app-3.8__";

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
    #[serde(rename = "envPortMapping", default)]
    pub env_port_mapping: HashMap<String, String>,
    #[serde(rename = "defaultPort", default)]
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

    // Account-based environments (3.8+). Absent or empty means a legacy project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<Environment>>,
    /// Skip IAM auth and always use the secret's password.
    #[serde(rename = "passwordOnly", default, skip_serializing_if = "Option::is_none")]
    pub password_only: Option<bool>,
}

pub const DEFAULT_BASTION_PATTERN: &str = "*bastion*";

impl ProjectConfig {
    /// True when the project has no account-based environments.
    pub fn is_legacy(&self) -> bool {
        self.environments.as_ref().is_none_or(|e| e.is_empty())
    }

    /// A clone of this project with the environment's overrides applied.
    pub fn for_environment(&self, env: &Environment) -> ProjectConfig {
        let mut c = self.clone();
        if let Some(v) = &env.rds_pattern {
            c.rds_pattern = v.clone();
        }
        if let Some(v) = &env.region {
            c.region = v.clone();
        }
        if let Some(v) = &env.target_pattern {
            c.target_pattern = Some(v.clone());
        }
        if let Some(v) = &env.ecs_cluster {
            c.ecs_cluster = Some(v.clone());
        }
        if let Some(v) = &env.ecs_service {
            c.ecs_service = Some(v.clone());
        }
        if let Some(v) = &env.bastion_pattern {
            c.bastion_pattern = Some(v.clone());
        }
        c
    }

    pub fn auth_mode(&self) -> AuthMode {
        if self.auth_type == "iam" {
            AuthMode::IamOnly
        } else if self.password_only == Some(true) {
            AuthMode::PasswordOnly
        } else {
            AuthMode::Auto
        }
    }

    /// True when the project has no secret configured (empty `secretPrefix`
    /// and no `secretPath`).
    pub fn has_no_secret_configured(&self) -> bool {
        self.secret_prefix.trim().is_empty()
            && self.secret_path.as_deref().is_none_or(|s| s.trim().is_empty())
    }

    /// Automatic auth with no secret configured signs in as "IAM DB User".
    pub fn uses_iam_without_secret(&self) -> bool {
        self.auth_mode() == AuthMode::Auto
            && self.has_no_secret_configured()
            && self.iam_username.as_deref().is_some_and(|u| !u.trim().is_empty())
    }

    /// How a connect signs in: like [`auth_mode`](Self::auth_mode), except
    /// that Automatic with no secret configured is IAM only for `iamUsername`.
    pub fn connect_auth_mode(&self) -> AuthMode {
        if self.uses_iam_without_secret() {
            AuthMode::IamOnly
        } else {
            self.auth_mode()
        }
    }

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

    /// PostgreSQL RDS connections go through the local auth proxy, which signs
    /// in with an IAM token or the secret's password as the database needs.
    /// MySQL connections stay a raw tunnel (IAM projects get a one-off token).
    pub fn uses_pg_auth_proxy(&self) -> bool {
        self.connection_type != "service" && self.engine.as_deref() != Some("mysql")
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

pub(crate) fn get_config_path() -> PathBuf {
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

/// Valid projects of `projects.json`. Broken projects are left out; a file
/// that cannot be read as a whole is an error.
pub async fn load_project_configs() -> Result<HashMap<String, ProjectConfig>, AppError> {
    Ok(project_file::load_project_file().await?.valid_configs())
}

/// Save one project. Broken projects in the file are written back unchanged;
/// a broken project this replaces is backed up with the file first.
pub async fn save_project_config(
    key: &str,
    config: ProjectConfig,
) -> Result<(), AppError> {
    project_file::save_project_at(&get_config_path(), key, config).await
}

/// Delete one project. A broken project is backed up with the file first.
pub async fn delete_project_config(key: &str) -> Result<(), AppError> {
    project_file::delete_project_at(&get_config_path(), key).await
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

    // envPortMapping is a per-profile port override, not a whitelist. It used to
    // additionally filter this list, which meant adding an AWS profile through
    // the UI left it invisible until envPortMapping was hand-edited in
    // projects.json — a file the UI never asks anyone to open, and a rule
    // nothing in the UI stated. get_local_port already falls back to
    // default_port for an unmapped profile; the extra filtering made that
    // branch unreachable.
    //
    // profileFilter is the mechanism for deciding which profiles belong to a
    // project.
    filtered.into_iter().cloned().collect()
}

/// The `envPortMapping` port for a profile, if one of its suffixes matches
/// (longest suffix first). `None` means `get_local_port` falls back to the
/// project's default port.
pub fn mapped_port(profile: &str, project_config: &ProjectConfig) -> Option<String> {
    let mut suffixes: Vec<&String> = project_config.env_port_mapping.keys().collect();
    suffixes.sort_by_key(|s| std::cmp::Reverse(s.len()));

    suffixes
        .iter()
        .find(|suffix| profile.ends_with(suffix.as_str()) || profile == suffix.as_str())
        .and_then(|suffix| project_config.env_port_mapping.get(suffix.as_str()).cloned())
}

/// Get local port number based on environment suffix matching.
pub fn get_local_port(profile: &str, project_config: &ProjectConfig) -> String {
    mapped_port(profile, project_config).unwrap_or_else(|| project_config.default_port.clone())
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
            environments: None,
            password_only: None,
        }
    }

    fn env(name: &str, account: &str, port: &str) -> Environment {
        Environment {
            name: name.to_string(),
            account_id: account.to_string(),
            port: port.to_string(),
            rds_pattern: None,
            region: None,
            target_pattern: None,
            ecs_cluster: None,
            ecs_service: None,
            bastion_pattern: None,
        }
    }

    // A copy of the 3.7.10 struct's required/typed fields, test-only.
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Legacy37 {
        name: String,
        region: String,
        #[serde(rename = "envPortMapping")]
        env_port_mapping: HashMap<String, String>,
        #[serde(rename = "defaultPort")]
        default_port: String,
        #[serde(rename = "authType", default)]
        auth_type: String,
        #[serde(rename = "profileFilter")]
        profile_filter: Option<String>,
    }

    #[test]
    fn new_project_serializes_readable_by_3_7() {
        let mut c = test_config(None);
        c.profile_filter = Some(HIDE_FROM_LEGACY_FILTER.into());
        c.env_port_mapping = HashMap::new();
        c.environments = Some(vec![env("dev", "073813807522", "5462")]);
        c.password_only = Some(true);
        let json = serde_json::to_string(&c).unwrap();
        let old: Legacy37 = serde_json::from_str(&json).unwrap();
        assert!(old.env_port_mapping.is_empty());
        assert_eq!(old.auth_type, "secrets");
        assert_eq!(old.profile_filter.as_deref(), Some(HIDE_FROM_LEGACY_FILTER));
        assert!(json.contains("\"accountId\":\"073813807522\""));
        assert!(json.contains("\"passwordOnly\":true"));
    }

    #[test]
    fn missing_env_port_mapping_and_default_port_still_load() {
        let json = r#"{"name":"P","region":"us-east-1","profileFilter":null}"#;
        let c: ProjectConfig = serde_json::from_str(json).unwrap();
        assert!(c.env_port_mapping.is_empty());
        assert_eq!(c.default_port, "");
    }

    #[test]
    fn for_environment_applies_only_set_overrides() {
        let mut c = test_config(Some("bast".into()));
        c.rds_pattern = "covered-db".into();
        c.region = "us-west-1".into();
        let mut e = env("dev", "073813807522", "5462");
        e.rds_pattern = Some("covered-db-2".into());
        let r = c.for_environment(&e);
        assert_eq!(r.rds_pattern, "covered-db-2");
        assert_eq!(r.region, "us-west-1");
        assert_eq!(r.bastion_pattern.as_deref(), Some("bast"));
    }

    #[test]
    fn auth_mode_mapping() {
        let mut c = test_config(None);
        assert_eq!(c.auth_mode(), AuthMode::Auto);
        c.password_only = Some(false);
        assert_eq!(c.auth_mode(), AuthMode::Auto);
        c.password_only = Some(true);
        assert_eq!(c.auth_mode(), AuthMode::PasswordOnly);
        c.auth_type = "iam".into();
        assert_eq!(c.auth_mode(), AuthMode::IamOnly);
    }

    #[test]
    fn automatic_without_secret_uses_iam() {
        let mut c = test_config(None);
        c.secret_prefix = String::new();
        c.secret_path = None;
        c.iam_username = Some("app_user".into());
        assert!(c.uses_iam_without_secret());
        assert_eq!(c.auth_mode(), AuthMode::Auto);
        assert_eq!(c.connect_auth_mode(), AuthMode::IamOnly);
    }

    #[test]
    fn automatic_with_a_secret_keeps_the_secret() {
        let mut c = test_config(None);
        c.iam_username = Some("app_user".into());
        c.secret_prefix = "rds!cluster".into();
        assert!(!c.uses_iam_without_secret());
        assert_eq!(c.connect_auth_mode(), AuthMode::Auto);

        c.secret_prefix = String::new();
        c.secret_path = Some("arn:aws:secretsmanager:us-east-1:1:secret:x".into());
        assert!(!c.uses_iam_without_secret());
        assert_eq!(c.connect_auth_mode(), AuthMode::Auto);
    }

    #[test]
    fn no_secret_and_no_iam_user_is_not_iam() {
        let mut c = test_config(None);
        c.secret_prefix = String::new();
        c.secret_path = None;
        c.iam_username = Some("  ".into());
        assert!(!c.uses_iam_without_secret());
        c.iam_username = None;
        assert!(!c.uses_iam_without_secret());
    }

    #[test]
    fn password_only_without_secret_is_not_iam() {
        let mut c = test_config(None);
        c.secret_prefix = String::new();
        c.iam_username = Some("app_user".into());
        c.password_only = Some(true);
        assert!(!c.uses_iam_without_secret());
        assert_eq!(c.connect_auth_mode(), AuthMode::PasswordOnly);
    }

    #[test]
    fn is_legacy() {
        let mut c = test_config(None);
        assert!(c.is_legacy());
        c.environments = Some(vec![]);
        assert!(c.is_legacy());
        c.environments = Some(vec![env("dev", "073813807522", "5462")]);
        assert!(!c.is_legacy());
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

    /// A profile that matches the project's profileFilter must be listed even
    /// when no envPortMapping key matches it. Reported from the field: adding a
    /// new AWS profile through the UI left it invisible, and the only remedy was
    /// hand-editing envPortMapping in projects.json — a file the UI never asks
    /// you to touch.
    ///
    /// The port mapping is a per-profile port override, which is exactly what
    /// the Settings UI calls it. get_local_port already falls back to
    /// default_port for an unmapped profile; treating the mapping as a
    /// whitelist made that branch unreachable.
    #[test]
    fn profile_without_port_mapping_is_still_listed() {
        let mut config = test_config(None);
        config.profile_filter = Some("covered".to_string());
        config.env_port_mapping =
            HashMap::from([("covered".to_string(), "5460".to_string())]);

        let all = vec![
            "covered".to_string(),
            "covered-dev".to_string(),
            "other-account".to_string(),
        ];
        let configs = HashMap::from([("covered".to_string(), config.clone())]);

        let listed = get_profiles_for_project(&all, &config, &configs);

        assert!(
            listed.contains(&"covered-dev".to_string()),
            "covered-dev matches profileFilter and must be listed; got {listed:?}"
        );
        assert!(listed.contains(&"covered".to_string()));
        assert!(!listed.contains(&"other-account".to_string()));
    }

    /// An unmapped profile falls back to default_port rather than being dropped.
    #[test]
    fn unmapped_profile_uses_default_port() {
        let mut config = test_config(None);
        config.env_port_mapping =
            HashMap::from([("covered".to_string(), "5460".to_string())]);

        assert_eq!(get_local_port("covered-dev", &config), "5432");
        assert_eq!(get_local_port("covered", &config), "5460");
    }

    #[test]
    fn iam_auth_fields_round_trip_and_default_to_secrets() {
        let json = r#"{"name":"Covered DB","region":"us-west-1","database":"covered_db",
            "rdsType":"instance","rdsPattern":"covered-db","profileFilter":"covered-",
            "envPortMapping":{},"defaultPort":"5432",
            "authType":"iam","iamUsername":"covered_app","multiplexed":true}"#;
        let config: ProjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.auth_type, "iam");
        assert_eq!(config.iam_username.as_deref(), Some("covered_app"));

        let reparsed: ProjectConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(reparsed.auth_type, "iam");
        assert_eq!(reparsed.iam_username.as_deref(), Some("covered_app"));
        assert_eq!(reparsed.multiplexed, Some(true));

        let legacy: ProjectConfig = serde_json::from_str(
            r#"{"name":"x","region":"us-west-1","profileFilter":null,"envPortMapping":{},"defaultPort":"5432"}"#,
        )
        .unwrap();
        assert_eq!(legacy.auth_type, "secrets");
        assert_eq!(legacy.iam_username, None);
    }

    #[test]
    fn auth_proxy_applies_to_postgres_rds_only() {
        let mut config = test_config(None);
        assert!(config.uses_pg_auth_proxy(), "engine defaults to postgres");
        config.auth_type = "iam".to_string();
        assert!(config.uses_pg_auth_proxy());
        config.engine = Some("postgres".to_string());
        assert!(config.uses_pg_auth_proxy());
        config.engine = Some("mysql".to_string());
        assert!(!config.uses_pg_auth_proxy());
        config.engine = None;
        config.connection_type = "service".to_string();
        assert!(!config.uses_pg_auth_proxy());
    }
}
