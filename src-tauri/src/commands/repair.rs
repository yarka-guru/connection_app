//! Commands that rewrite `projects.json` as a whole: the raw file editor,
//! backups, migration of legacy projects and import. Each of them backs up
//! the current file before writing.

use crate::commands::projects::load_profile_accounts;
use crate::config::migration::{self, MigrationChoice, ProjectProposal};
use crate::config::preferences::{self, Preferences};
use crate::config::project_file::{
    self, ProjectEntry, ProjectFile, backup_project_file_at, check_raw_file, list_backups_at,
    load_project_file_at, read_raw_at, write_atomic, write_project_file_at, write_raw_at,
};
use crate::config::projects::get_config_path;
use crate::config::validation::validate_project_config;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, State};

// --- whole file and backups ---

async fn save_file_raw_at(path: &Path, text: &str) -> Result<(), AppError> {
    check_raw_file(text)?;
    backup_project_file_at(path).await?;
    write_raw_at(path, text).await?;
    Ok(())
}

/// Put `requested` back as the file, if it is one of the file's backups.
/// The current file is backed up first.
async fn restore_backup_at(path: &Path, requested: &Path) -> Result<(), AppError> {
    let not_a_backup = || {
        AppError::Config(format!(
            "{} is not a backup of projects.json",
            requested.display()
        ))
    };
    let requested = tokio::fs::canonicalize(requested)
        .await
        .map_err(|_| not_a_backup())?;
    let mut backup = None;
    for candidate in list_backups_at(path).await {
        if tokio::fs::canonicalize(&candidate).await.ok().as_ref() == Some(&requested) {
            backup = Some(candidate);
            break;
        }
    }
    let backup = backup.ok_or_else(not_a_backup)?;

    let text = tokio::fs::read_to_string(&backup)
        .await
        .map_err(|e| AppError::Config(format!("Failed to read {}: {}", backup.display(), e)))?;
    backup_project_file_at(path).await?;
    write_atomic(path, &text).await
}

#[tauri::command]
pub async fn get_project_file_raw() -> Result<String, AppError> {
    read_raw_at(&get_config_path()).await
}

/// Replace the whole file. The text must be a JSON object.
#[tauri::command]
pub async fn save_project_file_raw(text: String) -> Result<(), AppError> {
    save_file_raw_at(&get_config_path(), &text).await
}

/// `path` must be one of the backups `list_projects` returned.
#[tauri::command]
pub async fn restore_project_backup(path: String) -> Result<(), AppError> {
    restore_backup_at(&get_config_path(), Path::new(&path)).await
}

// --- migration ---

/// Apply the confirmed migration to `file` and `prefs` in memory.
///
/// Choices for projects that are not legacy (any more) are ignored. Every
/// migrated project must pass validation, otherwise nothing should be
/// written and the error lists the problems. Returns whether any project
/// was migrated.
fn migrate(
    file: &mut ProjectFile,
    choices: &[MigrationChoice],
    prefs: &mut Preferences,
) -> Result<bool, AppError> {
    let legacy: Vec<MigrationChoice> = choices
        .iter()
        .filter(|c| {
            matches!(
                file.entries.get(&c.project_key),
                Some(ProjectEntry::Valid { config, .. }) if config.is_legacy()
            )
        })
        .cloned()
        .collect();
    migration::apply_migration(file, &legacy, prefs);

    let mut migrated = false;
    let mut problems = Vec::new();
    for choice in &legacy {
        if let Some(ProjectEntry::Valid { config, .. }) = file.entries.get(&choice.project_key)
            && !config.is_legacy()
        {
            migrated = true;
            for message in validate_project_config(config).messages() {
                problems.push(format!("{}: {}", choice.project_key, message));
            }
        }
    }
    if !problems.is_empty() {
        return Err(AppError::Config(format!(
            "Cannot update projects: {}",
            problems.join("; ")
        )));
    }
    Ok(migrated)
}

#[tauri::command]
pub async fn get_migration_proposal() -> Result<Vec<ProjectProposal>, AppError> {
    let file = project_file::load_project_file().await?;
    let accounts = load_profile_accounts().await;
    Ok(migration::propose_migration(
        &file.valid_configs(),
        &accounts,
    ))
}

/// Backup, then write the migrated file and the remembered profiles. Nothing
/// is written when a migrated project would be invalid.
#[tauri::command]
pub async fn apply_migration(choices: Vec<MigrationChoice>) -> Result<(), AppError> {
    let path = get_config_path();
    let mut file = load_project_file_at(&path).await?;
    let mut prefs = preferences::load_preferences().await;
    if !migrate(&mut file, &choices, &mut prefs)? {
        return Ok(());
    }
    backup_project_file_at(&path).await?;
    write_project_file_at(&path, &file).await?;
    preferences::save_preferences(&prefs).await;
    Ok(())
}

// --- import ---

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImportStatus {
    New,
    Conflict,
    Broken,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImportChoice {
    /// Import under the project's key (for a new project: import it).
    Replace,
    /// Leave the local project; skip the imported one.
    KeepMine,
    /// Import under a new key.
    Copy,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub key: String,
    pub status: ImportStatus,
    /// The local project with this key has environments.
    pub local_migrated: bool,
    /// The imported project has no environments.
    pub imported_legacy: bool,
    pub default_choice: ImportChoice,
    /// Why the imported project could not be read, for a broken one.
    pub error: Option<String>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub path: String,
    pub items: Vec<ImportItem>,
}

/// The file the user picked for import. `apply_import` only reads this one.
#[derive(Default)]
pub struct ImportSelection(Mutex<Option<PathBuf>>);

fn import_item(key: &str, imported: &ProjectEntry, local: &ProjectFile) -> ImportItem {
    let config = match imported {
        ProjectEntry::Valid { config, .. } => config,
        ProjectEntry::Broken { error, .. } => {
            return ImportItem {
                key: key.to_string(),
                status: ImportStatus::Broken,
                local_migrated: false,
                imported_legacy: false,
                default_choice: ImportChoice::KeepMine,
                error: Some(error.clone()),
            };
        }
    };
    let local_entry = local.entries.get(key);
    let local_migrated = matches!(
        local_entry,
        Some(ProjectEntry::Valid { config, .. }) if !config.is_legacy()
    );
    let imported_legacy = config.is_legacy();
    ImportItem {
        key: key.to_string(),
        status: if local_entry.is_some() {
            ImportStatus::Conflict
        } else {
            ImportStatus::New
        },
        local_migrated,
        imported_legacy,
        // A 3.7.x teammate's file must not undo a local migration by default.
        default_choice: if local_migrated && imported_legacy {
            ImportChoice::KeepMine
        } else {
            ImportChoice::Replace
        },
        error: None,
    }
}

/// One item per imported project, in key order.
fn import_items(local: &ProjectFile, imported: &ProjectFile) -> Vec<ImportItem> {
    imported
        .entries
        .iter()
        .map(|(key, entry)| import_item(key, entry, local))
        .collect()
}

/// `<key>-imported`, or `<key>-imported-2`, `-3`, … the first that is free.
fn copy_key(key: &str, taken: impl Fn(&str) -> bool) -> String {
    let first = format!("{key}-imported");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{first}-{n}"))
        .find(|k| !taken(k))
        .expect("an unbounded range always has a free key")
}

/// Merge the imported projects into `local`. A project without a choice
/// gets its default; broken projects are skipped. Returns how many
/// projects were imported.
fn merge_import(
    local: &mut ProjectFile,
    imported: &ProjectFile,
    choices: &HashMap<String, ImportChoice>,
) -> usize {
    let items = import_items(local, imported);
    let mut count = 0;
    for item in items {
        let Some(ProjectEntry::Valid { config, .. }) = imported.entries.get(&item.key) else {
            continue;
        };
        let choice = choices
            .get(&item.key)
            .copied()
            .unwrap_or(item.default_choice);
        let key = match choice {
            ImportChoice::KeepMine => continue,
            ImportChoice::Replace => item.key,
            // Free in both files, so a later imported project cannot land on the copy.
            ImportChoice::Copy => copy_key(&item.key, |k| {
                local.entries.contains_key(k) || imported.entries.contains_key(k)
            }),
        };
        local.upsert(&key, config.clone());
        count += 1;
    }
    count
}

/// Ask for a projects file and describe what importing it would do.
#[tauri::command]
pub async fn pick_import_file(
    app_handle: AppHandle,
    selection: State<'_, ImportSelection>,
) -> Result<ImportPreview, AppError> {
    let path =
        crate::commands::system::pick_json_file(&app_handle, "Select a projects.json file").await?;
    let imported = load_project_file_at(&path).await?;
    let local = project_file::load_project_file().await?;
    let preview = ImportPreview {
        path: path.to_string_lossy().into_owned(),
        items: import_items(&local, &imported),
    };
    *selection
        .0
        .lock()
        .map_err(|_| AppError::General("Import state is unavailable".to_string()))? = Some(path);
    Ok(preview)
}

/// Import the file picked with `pick_import_file`, after backing up the
/// current file. Returns how many projects were imported.
#[tauri::command]
pub async fn apply_import(
    selection: State<'_, ImportSelection>,
    path: String,
    choices: HashMap<String, ImportChoice>,
) -> Result<usize, AppError> {
    let picked = selection
        .0
        .lock()
        .map_err(|_| AppError::General("Import state is unavailable".to_string()))?
        .clone()
        .filter(|p| *p == Path::new(&path))
        .ok_or_else(|| AppError::Config("Pick the file to import first".to_string()))?;

    let imported = load_project_file_at(&picked).await?;
    let config_path = get_config_path();
    let mut local = load_project_file_at(&config_path).await?;
    let count = merge_import(&mut local, &imported, &choices);
    if count > 0 {
        backup_project_file_at(&config_path).await?;
        write_project_file_at(&config_path, &local).await?;
    }
    if let Ok(mut guard) = selection.0.lock() {
        *guard = None;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::migration::{MigrationChoice, ProposedEnvironment};
    use crate::config::preferences::{Preferences, get_account_profile};
    use crate::config::project_file::{ProjectEntry, ProjectFile};
    use crate::config::projects::ProjectConfig;
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PROD: &str = "231030592327";
    const DEV: &str = "073813807522";

    fn legacy(name: &str) -> ProjectConfig {
        serde_json::from_value(json!({
            "name": name,
            "region": "us-west-1",
            "database": "db",
            "rdsType": "instance",
            "rdsPattern": "covered-db",
            "secretPrefix": "rds!db",
            "profileFilter": "covered",
            "envPortMapping": {"staging": "5461"},
            "defaultPort": "5460"
        }))
        .unwrap()
    }

    fn migrated(name: &str) -> ProjectConfig {
        let mut c = legacy(name);
        c.environments = Some(vec![crate::config::projects::Environment {
            name: "prod".to_string(),
            account_id: PROD.to_string(),
            port: "5460".to_string(),
            rds_pattern: None,
            region: None,
            target_pattern: None,
            ecs_cluster: None,
            ecs_service: None,
            bastion_pattern: None,
        }]);
        c
    }

    fn proposed(name: &str, account: &str, port: &str, profiles: &[&str]) -> ProposedEnvironment {
        ProposedEnvironment {
            name: name.to_string(),
            account_id: account.to_string(),
            port: port.to_string(),
            profiles: profiles.iter().map(|p| p.to_string()).collect(),
            preferred_profile: profiles[0].to_string(),
            selected: true,
        }
    }

    fn config<'a>(file: &'a ProjectFile, key: &str) -> &'a ProjectConfig {
        match &file.entries[key] {
            ProjectEntry::Valid { config, .. } => config,
            ProjectEntry::Broken { .. } => panic!("{key} is broken"),
        }
    }

    fn temp_dir(test: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "connection-app-repair-{}-{}-{}",
            test,
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    // --- migration (ruling R6) ---

    #[test]
    fn migration_applies_choices_and_remembers_profiles() {
        let mut file = ProjectFile::default();
        file.upsert("covered", legacy("Covered"));
        let mut prefs = Preferences::default();
        let choices = vec![MigrationChoice {
            project_key: "covered".to_string(),
            environments: vec![
                proposed("prod", PROD, "5460", &["covered-admin", "covered"]),
                proposed("staging", DEV, "5461", &["covered-staging"]),
            ],
        }];
        assert!(migrate(&mut file, &choices, &mut prefs).unwrap());
        let envs = config(&file, "covered").environments.clone().unwrap();
        assert_eq!(envs.len(), 2);
        assert_eq!(get_account_profile(&prefs, PROD), Some("covered-admin"));
    }

    #[test]
    fn migration_ignores_choices_for_projects_that_are_no_longer_legacy() {
        let mut file = ProjectFile::default();
        file.upsert("covered", migrated("Covered"));
        let mut prefs = Preferences::default();
        let choices = vec![MigrationChoice {
            project_key: "covered".to_string(),
            environments: vec![proposed("dev", DEV, "5462", &["covered-dev"])],
        }];
        assert!(!migrate(&mut file, &choices, &mut prefs).unwrap());
        let envs = config(&file, "covered").environments.clone().unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "prod");
    }

    #[test]
    fn migration_with_an_invalid_result_is_refused_with_field_errors() {
        let mut file = ProjectFile::default();
        file.upsert("covered", legacy("Covered"));
        let mut prefs = Preferences::default();
        let choices = vec![MigrationChoice {
            project_key: "covered".to_string(),
            environments: vec![
                proposed("prod", PROD, "5460", &["covered"]),
                proposed("prod", "12345", "5461", &["covered-staging"]),
            ],
        }];
        let err = migrate(&mut file, &choices, &mut prefs)
            .unwrap_err()
            .to_string();
        assert!(err.contains("covered"), "{err}");
        assert!(err.contains("environments[1].accountId"), "{err}");
    }

    // --- import ---

    fn imported_file() -> ProjectFile {
        let mut f = ProjectFile::default();
        f.upsert("covered", legacy("Covered (teammate)"));
        f.upsert("fresh", legacy("Fresh"));
        f.upsert_raw("bad", r#"{"name": 5}"#).unwrap();
        f
    }

    #[test]
    fn import_default_keeps_migrated_local_over_legacy_import() {
        let mut local = ProjectFile::default();
        local.upsert("covered", migrated("Covered"));
        let items = import_items(&local, &imported_file());
        let by_key: HashMap<&str, &ImportItem> =
            items.iter().map(|i| (i.key.as_str(), i)).collect();

        let covered = by_key["covered"];
        assert_eq!(covered.status, ImportStatus::Conflict);
        assert!(covered.local_migrated);
        assert!(covered.imported_legacy);
        assert_eq!(covered.default_choice, ImportChoice::KeepMine);

        assert_eq!(by_key["fresh"].status, ImportStatus::New);
        assert_eq!(by_key["bad"].status, ImportStatus::Broken);
        assert!(by_key["bad"].error.is_some());

        // Without explicit choices the defaults apply: the migrated project stays.
        let count = merge_import(&mut local, &imported_file(), &HashMap::new());
        assert_eq!(count, 1);
        assert_eq!(config(&local, "covered").name, "Covered");
        assert!(local.entries.contains_key("fresh"));
        assert!(!local.entries.contains_key("bad"));
    }

    #[test]
    fn import_conflict_between_legacy_projects_defaults_to_replace() {
        let mut local = ProjectFile::default();
        local.upsert("covered", legacy("Covered"));
        let items = import_items(&local, &imported_file());
        let covered = items.iter().find(|i| i.key == "covered").unwrap();
        assert_eq!(covered.default_choice, ImportChoice::Replace);
    }

    #[test]
    fn import_choices_replace_keep_and_copy() {
        let mut local = ProjectFile::default();
        local.upsert("covered", migrated("Covered"));
        local.upsert("fresh", migrated("Mine"));
        let choices = HashMap::from([
            ("covered".to_string(), ImportChoice::Copy),
            ("fresh".to_string(), ImportChoice::Replace),
            ("bad".to_string(), ImportChoice::Replace),
        ]);
        let count = merge_import(&mut local, &imported_file(), &choices);
        assert_eq!(count, 2);
        assert_eq!(config(&local, "covered").name, "Covered");
        assert_eq!(
            config(&local, "covered-imported").name,
            "Covered (teammate)"
        );
        assert_eq!(config(&local, "fresh").name, "Fresh");
        assert!(!local.entries.contains_key("bad"));
    }

    #[test]
    fn import_copy_key_is_unique() {
        let taken = ["covered", "covered-imported"];
        let key = copy_key("covered", |k| taken.contains(&k));
        assert_eq!(key, "covered-imported-2");
        assert_eq!(copy_key("other", |k| taken.contains(&k)), "other-imported");
    }

    #[test]
    fn import_copy_does_not_take_a_key_another_imported_project_uses() {
        let mut local = ProjectFile::default();
        local.upsert("covered", migrated("Covered"));
        let mut imported = ProjectFile::default();
        imported.upsert("covered", legacy("Theirs"));
        imported.upsert("covered-imported", legacy("Also theirs"));
        let choices = HashMap::from([("covered".to_string(), ImportChoice::Copy)]);
        assert_eq!(merge_import(&mut local, &imported, &choices), 2);
        assert_eq!(config(&local, "covered-imported").name, "Also theirs");
        assert_eq!(config(&local, "covered-imported-2").name, "Theirs");
    }

    #[test]
    fn import_items_serialize_camel_case() {
        let local = ProjectFile::default();
        let v = serde_json::to_value(import_items(&local, &imported_file())).unwrap();
        let fresh = v
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == "fresh")
            .unwrap();
        assert_eq!(fresh["status"], "new");
        assert_eq!(fresh["defaultChoice"], "replace");
        assert_eq!(fresh["importedLegacy"], true);
        assert_eq!(fresh["localMigrated"], false);
        let choice: ImportChoice = serde_json::from_value(json!("keepMine")).unwrap();
        assert_eq!(choice, ImportChoice::KeepMine);
    }

    // --- whole-file save and restore ---

    #[tokio::test]
    async fn restore_accepts_only_a_listed_backup_and_backs_up_first() {
        let dir = temp_dir("restore");
        let path = dir.join("projects.json");
        let backup = dir.join("projects.json.bak-20200101-000000");
        std::fs::write(&path, "{\"current\": 1}").unwrap();
        std::fs::write(&backup, "{\"old\": 1}").unwrap();
        let outside = dir.join("other.json");
        std::fs::write(&outside, "{\"evil\": 1}").unwrap();

        let err = restore_backup_at(&path, &outside).await.unwrap_err();
        assert!(err.to_string().contains("not a backup"), "{err}");
        let err = restore_backup_at(&path, &dir.join("missing.bak-20200101-000000"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a backup"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"current\": 1}");

        // The same backup reached through `..` is still that backup.
        let roundabout = dir
            .join("..")
            .join(dir.file_name().unwrap())
            .join(backup.file_name().unwrap());
        restore_backup_at(&path, &roundabout).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"old\": 1}");
        let names = file_names(&dir);
        assert_eq!(names.len(), 4, "{names:?}");
        let new_backup = names
            .iter()
            .find(|n| n.starts_with("projects.json.bak-") && !n.ends_with("20200101-000000"))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(new_backup)).unwrap(),
            "{\"current\": 1}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn whole_file_save_checks_the_text_then_backs_up() {
        let dir = temp_dir("save-raw");
        let path = dir.join("projects.json");
        std::fs::write(&path, "{\"current\": 1}").unwrap();

        assert!(save_file_raw_at(&path, "[1]").await.is_err());
        assert_eq!(file_names(&dir), vec!["projects.json"]);

        save_file_raw_at(&path, "{\"next\": 1}\n").await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"next\": 1}\n");
        let names = file_names(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join(&names[1])).unwrap(),
            "{\"current\": 1}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
