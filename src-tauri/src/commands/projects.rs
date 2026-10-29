use crate::config::accounts::{self, ProfileAccount};
use crate::config::aws_config;
use crate::config::environments::{ResolvedEnvironment, resolve_environments};
use crate::config::migration::propose_migration;
use crate::config::preferences::{self, Preferences};
use crate::config::project_file::{
    self, FileError, ProjectEntry, ProjectFile, list_backups_at, salvage_config,
};
use crate::config::projects::{self, ProjectConfig, get_config_path};
use crate::config::validation::{self, FieldError};
use crate::error::AppError;
use serde::Serialize;

/// Everything the app shows about `projects.json`.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsOverview {
    pub projects: Vec<ProjectSummary>,
    /// Set when the file as a whole cannot be read; `projects` is empty then.
    pub file_error: Option<FileError>,
    pub needs_migration: bool,
    /// Backup file paths, newest first.
    pub backups: Vec<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub key: String,
    pub name: String,
    pub connection_type: String,
    pub databases: Option<Vec<String>>,
    pub environments: Vec<ResolvedEnvironment>,
    pub problems: Vec<FieldError>,
    /// Why the project could not be read, for a broken project.
    pub broken: Option<String>,
    /// At least one environment has an AWS profile.
    pub reachable: bool,
}

/// One project of the file, for the Settings editor.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntryDto {
    pub key: String,
    /// The project, or for a broken one whatever of it could be read.
    pub config: Option<ProjectConfig>,
    /// The project's JSON as written, for a broken project.
    pub raw: Option<String>,
    pub error: Option<String>,
    pub problems: Vec<FieldError>,
}

fn connection_type(config: &ProjectConfig) -> String {
    if config.connection_type.is_empty() {
        "rds".to_string()
    } else {
        config.connection_type.clone()
    }
}

/// A summary per project, in key order. A broken project is summarized from
/// what could be read of it.
pub(crate) fn project_summaries(
    file: &ProjectFile,
    accounts: &[ProfileAccount],
    prefs: &Preferences,
) -> Vec<ProjectSummary> {
    let configs = file.valid_configs();
    file.entries
        .iter()
        .map(|(key, entry)| {
            let (config, problems, broken) = match entry {
                ProjectEntry::Valid { config, problems } => {
                    (Some(config.clone()), problems.clone(), None)
                }
                ProjectEntry::Broken { raw, error } => {
                    (salvage_config(raw), Vec::new(), Some(error.clone()))
                }
            };
            let environments = config
                .as_ref()
                .map(|c| resolve_environments(c, &configs, accounts, prefs))
                .unwrap_or_default();
            ProjectSummary {
                key: key.clone(),
                name: config
                    .as_ref()
                    .map_or_else(|| key.clone(), |c| c.name.clone()),
                connection_type: config
                    .as_ref()
                    .map_or_else(|| "rds".to_string(), connection_type),
                databases: config.as_ref().and_then(|c| c.databases.clone()),
                reachable: environments.iter().any(|e| e.profile.is_some()),
                environments,
                problems,
                broken,
            }
        })
        .collect()
}

/// True when a legacy project has at least one environment to create.
pub(crate) fn needs_migration(file: &ProjectFile, accounts: &[ProfileAccount]) -> bool {
    propose_migration(&file.valid_configs(), accounts)
        .iter()
        .any(|p| !p.environments.is_empty())
}

pub(crate) fn project_entries(file: &ProjectFile) -> Vec<ProjectEntryDto> {
    file.entries
        .iter()
        .map(|(key, entry)| match entry {
            ProjectEntry::Valid { config, problems } => ProjectEntryDto {
                key: key.clone(),
                config: Some(config.clone()),
                raw: None,
                error: None,
                problems: problems.clone(),
            },
            ProjectEntry::Broken { raw, error } => ProjectEntryDto {
                key: key.clone(),
                config: salvage_config(raw),
                raw: Some(raw.get().to_string()),
                error: Some(error.clone()),
                problems: Vec::new(),
            },
        })
        .collect()
}

/// Every AWS profile with its account, from local files only.
pub(crate) async fn load_profile_accounts() -> Vec<ProfileAccount> {
    let profiles = aws_config::read_aws_config().await.unwrap_or_else(|e| {
        log::warn!("Failed to read AWS config: {}", e);
        Vec::new()
    });
    accounts::profile_accounts(&profiles, &accounts::load_account_cache().await)
}

#[tauri::command]
pub async fn list_projects() -> Result<ProjectsOverview, AppError> {
    let backups = list_backups_at(&get_config_path())
        .await
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let file = match project_file::load_project_file().await {
        Ok(file) => file,
        Err(e) => {
            return Ok(ProjectsOverview {
                projects: Vec::new(),
                file_error: Some(e),
                needs_migration: false,
                backups,
            });
        }
    };
    let accounts = load_profile_accounts().await;
    let prefs = preferences::load_preferences().await;
    Ok(ProjectsOverview {
        projects: project_summaries(&file, &accounts, &prefs),
        file_error: None,
        needs_migration: needs_migration(&file, &accounts),
        backups,
    })
}

#[tauri::command]
pub async fn list_project_entries() -> Result<Vec<ProjectEntryDto>, AppError> {
    Ok(project_entries(&project_file::load_project_file().await?))
}

#[tauri::command]
pub async fn validate_project(config: ProjectConfig) -> Result<Vec<FieldError>, AppError> {
    Ok(validation::validate_project_config(&config).errors)
}

#[tauri::command]
pub async fn save_project_config(key: String, config: ProjectConfig) -> Result<(), AppError> {
    let result = validation::validate_project_config(&config);
    if !result.valid {
        return Err(AppError::Config(format!(
            "Validation failed: {}",
            result.messages().join(", ")
        )));
    }

    projects::save_project_config(&key, config).await
}

/// Replace one project with raw JSON. Refused, with nothing written, unless
/// the JSON is a valid project; a broken project it replaces is backed up.
#[tauri::command]
pub async fn save_project_raw(key: String, json: String) -> Result<(), AppError> {
    project_file::save_project_raw_at(&get_config_path(), &key, &json).await
}

#[tauri::command]
pub async fn delete_project_config(key: String) -> Result<(), AppError> {
    projects::delete_project_config(&key).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::accounts::ProfileAccount;
    use crate::config::preferences::Preferences;
    use crate::config::project_file::ProjectFile;
    use serde_json::json;

    const PROD: &str = "231030592327";
    const DEV: &str = "073813807522";

    fn account_project(name: &str) -> ProjectConfig {
        serde_json::from_value(json!({
            "name": name,
            "region": "us-west-1",
            "database": "db",
            "rdsType": "instance",
            "rdsPattern": "covered-db",
            "profileFilter": "__connection-app-3.8__",
            "envPortMapping": {},
            "defaultPort": "5432",
            "environments": [
                {"name": "prod", "accountId": PROD, "port": "5460"},
                {"name": "dev", "accountId": DEV, "port": "5462"}
            ]
        }))
        .unwrap()
    }

    fn legacy_project(filter: &str) -> ProjectConfig {
        serde_json::from_value(json!({
            "name": "Legacy",
            "region": "us-west-1",
            "database": "db",
            "rdsType": "instance",
            "rdsPattern": "legacy-db",
            "profileFilter": filter,
            "envPortMapping": {"staging": "5461"},
            "defaultPort": "5432"
        }))
        .unwrap()
    }

    fn pa(profile: &str, account: Option<&str>) -> ProfileAccount {
        ProfileAccount {
            profile: profile.to_string(),
            account_id: account.map(str::to_string),
        }
    }

    #[test]
    fn reachable_requires_a_resolved_profile() {
        let mut file = ProjectFile::default();
        file.upsert("covered", account_project("Covered"));
        let mut other = account_project("Other");
        for env in other.environments.as_mut().unwrap() {
            env.account_id = "999999999999".to_string();
        }
        file.upsert("other", other);

        let accounts = vec![pa("covered", Some(PROD))];
        let summaries = project_summaries(&file, &accounts, &Preferences::default());
        let covered = summaries.iter().find(|s| s.key == "covered").unwrap();
        assert!(covered.reachable);
        assert_eq!(covered.environments.len(), 2);
        assert_eq!(covered.environments[0].profile.as_deref(), Some("covered"));
        assert_eq!(covered.environments[1].profile, None);

        let other = summaries.iter().find(|s| s.key == "other").unwrap();
        assert!(!other.reachable);
        assert_eq!(other.environments.len(), 2);
        assert!(other.broken.is_none());
    }

    #[test]
    fn broken_project_is_summarized_from_what_could_be_read() {
        let mut file = ProjectFile::default();
        file.upsert_raw(
            "half",
            r#"{"name":"Half","region":"us-west-1","connectionType":"service","remotePort":"x"}"#,
        )
        .unwrap();
        let summaries = project_summaries(&file, &[], &Preferences::default());
        let half = &summaries[0];
        assert_eq!(half.name, "Half");
        assert_eq!(half.connection_type, "service");
        assert!(half.broken.as_deref().unwrap().contains("remotePort"));
        assert!(!half.reachable);

        let entries = project_entries(&file);
        assert_eq!(entries[0].key, "half");
        assert_eq!(entries[0].config.as_ref().unwrap().name, "Half");
        assert!(
            entries[0]
                .raw
                .as_deref()
                .unwrap()
                .contains("\"remotePort\":\"x\"")
        );
        assert!(entries[0].error.is_some());
    }

    #[test]
    fn valid_entry_has_config_and_problems_but_no_raw() {
        let mut file = ProjectFile::default();
        let mut c = account_project("Covered");
        c.environments.as_mut().unwrap()[1].name = "prod".to_string();
        file.upsert("covered", c);
        let entries = project_entries(&file);
        assert!(entries[0].config.is_some());
        assert!(entries[0].raw.is_none());
        assert!(entries[0].error.is_none());
        assert!(!entries[0].problems.is_empty());
    }

    #[test]
    fn migration_is_needed_only_when_a_legacy_project_has_an_environment_to_create() {
        let mut file = ProjectFile::default();
        file.upsert("covered", account_project("Covered"));
        let accounts = vec![pa("legacy-staging", Some(DEV)), pa("nobody", None)];
        assert!(!needs_migration(&file, &accounts));

        file.upsert("nomatch", legacy_project("unmatched"));
        assert!(!needs_migration(&file, &accounts));

        file.upsert("legacy", legacy_project("legacy"));
        assert!(needs_migration(&file, &accounts));
    }

    #[test]
    fn overview_serializes_camel_case() {
        let overview = ProjectsOverview {
            projects: Vec::new(),
            file_error: Some(FileError::NotAnObject),
            needs_migration: false,
            backups: vec!["/x/projects.json.bak-20261029-101500".to_string()],
        };
        let v = serde_json::to_value(&overview).unwrap();
        assert_eq!(v["fileError"]["kind"], "notAnObject");
        assert_eq!(v["needsMigration"], false);
        assert_eq!(v["backups"][0], "/x/projects.json.bak-20261029-101500");
    }
}
