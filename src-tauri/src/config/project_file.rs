//! Per-project loading of `projects.json`, atomic writes and backups.
//!
//! The file is parsed as a JSON object first, then each project on its own,
//! so one malformed project never hides the others. Loading never writes.

use crate::config::projects::{ProjectConfig, get_config_path};
use crate::config::validation::{FieldError, validate_project_config};
use crate::error::AppError;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// One project of `projects.json`.
// A file holds a handful of projects, so boxing `config` would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum ProjectEntry {
    /// Deserialized, after the safe fixes of [`normalize`].
    Valid {
        config: ProjectConfig,
        problems: Vec<FieldError>,
    },
    /// Could not be deserialized. `raw` is written back byte-for-byte.
    Broken { raw: Box<RawValue>, error: String },
}

#[derive(Debug, Clone, Default)]
pub struct ProjectFile {
    pub entries: BTreeMap<String, ProjectEntry>,
}

/// Why the file as a whole could not be read. Projects are not loaded at all.
#[derive(Serialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileError {
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    NotAnObject,
    Io {
        message: String,
    },
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Syntax {
                line,
                column,
                message,
            } => write!(
                f,
                "projects.json has a syntax error at line {line}, column {column}: {message}"
            ),
            FileError::NotAnObject => write!(f, "projects.json must contain a JSON object"),
            FileError::Io { message } => write!(f, "{message}"),
        }
    }
}

impl From<FileError> for AppError {
    fn from(e: FileError) -> Self {
        AppError::Config(e.to_string())
    }
}

fn syntax_error(e: serde_json::Error) -> FileError {
    let full = e.to_string();
    let location = format!(" at line {} column {}", e.line(), e.column());
    FileError::Syntax {
        line: e.line(),
        column: e.column(),
        message: full.strip_suffix(&location).unwrap_or(&full).to_string(),
    }
}

fn is_object(raw: &RawValue) -> bool {
    raw.get().trim_start().starts_with('{')
}

/// Parse `text` as a JSON object, keeping each value's raw text.
fn parse_object(text: &str) -> Result<BTreeMap<String, Box<RawValue>>, FileError> {
    let whole: &RawValue = serde_json::from_str(text).map_err(syntax_error)?;
    if !is_object(whole) {
        return Err(FileError::NotAnObject);
    }
    serde_json::from_str(text).map_err(syntax_error)
}

/// Safe fixes only: numeric ports become strings, and surrounding whitespace
/// is trimmed from every string. Returns a description of each fix.
pub(crate) fn normalize(v: &mut Value) -> Vec<String> {
    let mut fixes = Vec::new();
    if let Some(obj) = v.as_object_mut() {
        if let Some(port) = obj.get_mut("defaultPort") {
            port_to_string(port, "defaultPort", &mut fixes);
        }
        if let Some(Value::Object(mapping)) = obj.get_mut("envPortMapping") {
            for (suffix, port) in mapping.iter_mut() {
                port_to_string(port, &format!("envPortMapping.{suffix}"), &mut fixes);
            }
        }
        if let Some(Value::Array(envs)) = obj.get_mut("environments") {
            for (i, env) in envs.iter_mut().enumerate() {
                if let Some(port) = env.get_mut("port") {
                    port_to_string(port, &format!("environments[{i}].port"), &mut fixes);
                }
            }
        }
    }
    trim_strings(v, "", &mut fixes);
    fixes
}

fn port_to_string(port: &mut Value, path: &str, fixes: &mut Vec<String>) {
    if let Some(n) = port.as_u64() {
        *port = Value::String(n.to_string());
        fixes.push(format!("{path}: converted number to string"));
    }
}

fn trim_strings(v: &mut Value, path: &str, fixes: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.len() != s.len() {
                *s = trimmed.to_string();
                fixes.push(format!("{path}: trimmed surrounding whitespace"));
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                trim_strings(item, &format!("{path}[{i}]"), fixes);
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                trim_strings(item, &child, fixes);
            }
        }
        _ => {}
    }
}

/// The smallest object that deserializes: the required fields, empty.
fn probe_base() -> Map<String, Value> {
    let mut base = Map::new();
    base.insert("name".into(), Value::String(String::new()));
    base.insert("region".into(), Value::String(String::new()));
    base
}

/// Deserialize one field on its own, next to the required ones.
fn probe_field(field: &str, v: &Value) -> Result<ProjectConfig, serde_json::Error> {
    let mut probe = probe_base();
    probe.insert(field.to_string(), v.clone());
    ProjectConfig::deserialize(&Value::Object(probe))
}

/// serde_json does not say which field failed, so try each field on its own
/// and name the first that fails.
fn describe_error(value: &Value, err: serde_json::Error) -> String {
    let Some(obj) = value.as_object() else {
        return err.to_string();
    };
    if ProjectConfig::deserialize(&Value::Object(probe_base())).is_err() {
        return err.to_string();
    }
    for (field, v) in obj {
        if let Err(e) = probe_field(field, v) {
            return format!("{field}: {e}");
        }
    }
    err.to_string()
}

/// A broken project as far as it can be read: every field that deserializes
/// on its own, with an empty `name` and `region` when those cannot be read.
/// `None` when the project is not a JSON object.
pub fn salvage_config(raw: &RawValue) -> Option<ProjectConfig> {
    let mut value: Value = serde_json::from_str(raw.get()).ok()?;
    normalize(&mut value);
    let mut kept = probe_base();
    for (field, v) in value.as_object()? {
        if probe_field(field, v).is_ok() {
            kept.insert(field.clone(), v.clone());
        }
    }
    ProjectConfig::deserialize(&Value::Object(kept)).ok()
}

fn load_entry(key: &str, raw: Box<RawValue>) -> ProjectEntry {
    let mut value: Value = match serde_json::from_str(raw.get()) {
        Ok(v) => v,
        Err(e) => {
            return ProjectEntry::Broken {
                error: e.to_string(),
                raw,
            };
        }
    };
    for fix in normalize(&mut value) {
        log::debug!("projects.json: {key}: {fix}");
    }
    match ProjectConfig::deserialize(&value) {
        Ok(config) => {
            let problems = validate_project_config(&config).errors;
            ProjectEntry::Valid { config, problems }
        }
        Err(e) => ProjectEntry::Broken {
            error: describe_error(&value, e),
            raw,
        },
    }
}

/// A project as written to disk: valid ones re-serialized, broken ones verbatim.
enum Written<'a> {
    Config(&'a ProjectConfig),
    Raw(&'a RawValue),
}

impl Serialize for Written<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Written::Config(c) => c.serialize(s),
            Written::Raw(r) => r.serialize(s),
        }
    }
}

impl ProjectFile {
    pub fn valid_configs(&self) -> HashMap<String, ProjectConfig> {
        self.entries
            .iter()
            .filter_map(|(key, entry)| match entry {
                ProjectEntry::Valid { config, .. } => Some((key.clone(), config.clone())),
                ProjectEntry::Broken { .. } => None,
            })
            .collect()
    }

    pub fn upsert(&mut self, key: &str, config: ProjectConfig) {
        let problems = validate_project_config(&config).errors;
        self.entries
            .insert(key.to_string(), ProjectEntry::Valid { config, problems });
    }

    /// Replace one project with raw JSON, which must be a JSON object. It is
    /// loaded like a project read from disk, so it may end up `Broken`.
    pub fn upsert_raw(&mut self, key: &str, json: &str) -> Result<(), String> {
        let raw: Box<RawValue> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if !is_object(&raw) {
            return Err("A project must be a JSON object".to_string());
        }
        self.entries.insert(key.to_string(), load_entry(key, raw));
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }

    /// True when `key` is in the file and could not be read as a project.
    pub fn is_broken(&self, key: &str) -> bool {
        matches!(self.entries.get(key), Some(ProjectEntry::Broken { .. }))
    }

    /// Pretty-printed with a trailing newline. Broken entries are emitted from
    /// their raw JSON verbatim.
    pub fn to_json(&self) -> String {
        let out: BTreeMap<&str, Written> = self
            .entries
            .iter()
            .map(|(key, entry)| {
                let written = match entry {
                    ProjectEntry::Valid { config, .. } => Written::Config(config),
                    ProjectEntry::Broken { raw, .. } => Written::Raw(raw),
                };
                (key.as_str(), written)
            })
            .collect();
        let mut json =
            serde_json::to_string_pretty(&out).expect("project configs serialize to JSON");
        json.push('\n');
        json
    }
}

pub async fn load_project_file_at(path: &Path) -> Result<ProjectFile, FileError> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ProjectFile::default()),
        Err(e) => {
            return Err(FileError::Io {
                message: format!("Failed to read {}: {}", path.display(), e),
            });
        }
    };
    if text.trim().is_empty() {
        return Ok(ProjectFile::default());
    }
    let entries = parse_object(&text)?
        .into_iter()
        .map(|(key, raw)| {
            let entry = load_entry(&key, raw);
            (key, entry)
        })
        .collect();
    Ok(ProjectFile { entries })
}

pub async fn load_project_file() -> Result<ProjectFile, FileError> {
    load_project_file_at(&get_config_path()).await
}

fn dir_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Write `<name>.tmp-<pid>` next to the file, then rename it over the file.
/// A symlinked file is written through the link, which stays in place.
pub async fn write_atomic(path: &Path, contents: &str) -> Result<(), AppError> {
    let target = tokio::fs::canonicalize(path)
        .await
        .unwrap_or_else(|_| path.to_path_buf());
    let name = target
        .file_name()
        .ok_or_else(|| AppError::Config(format!("Not a file path: {}", path.display())))?;
    let dir = dir_of(&target);
    let tmp = dir.join(format!(
        "{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| AppError::Config(format!("Failed to create {}: {}", dir.display(), e)))?;

    let written = async {
        let mut file = tokio::fs::File::create(&tmp).await?;
        file.write_all(contents.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&tmp, &target).await
    }
    .await;

    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(AppError::Config(format!(
            "Failed to write {}: {}",
            path.display(),
            e
        )));
    }
    Ok(())
}

pub async fn write_project_file_at(path: &Path, file: &ProjectFile) -> Result<(), AppError> {
    write_atomic(path, &file.to_json()).await
}

pub async fn write_project_file(file: &ProjectFile) -> Result<(), AppError> {
    write_project_file_at(&get_config_path(), file).await
}

/// Back up the file at `path` when `key` is a broken entry that is about to
/// be replaced or removed: its raw JSON exists nowhere else.
pub async fn backup_before_replacing_at(
    path: &Path,
    file: &ProjectFile,
    key: &str,
) -> Result<Option<PathBuf>, AppError> {
    if file.is_broken(key) {
        backup_project_file_at(path).await
    } else {
        Ok(None)
    }
}

/// One project from the single-project JSON editor. Refused unless it reads
/// as a project and passes validation, so the editor never stores a project
/// that is still broken.
pub fn parse_project_raw(json: &str) -> Result<ProjectConfig, String> {
    let raw: Box<RawValue> =
        serde_json::from_str(json).map_err(|e| format!("Not valid JSON: {e}"))?;
    if !is_object(&raw) {
        return Err("A project must be a JSON object".to_string());
    }
    match load_entry("", raw) {
        ProjectEntry::Broken { error, .. } => Err(format!("Not saved: {error}")),
        ProjectEntry::Valid { problems, .. } if !problems.is_empty() => {
            let list: Vec<String> = problems
                .iter()
                .map(|p| {
                    if p.field.is_empty() {
                        p.message.clone()
                    } else {
                        format!("{}: {}", p.field, p.message)
                    }
                })
                .collect();
            Err(format!("Not saved: {}", list.join("; ")))
        }
        ProjectEntry::Valid { config, .. } => Ok(config),
    }
}

/// Save one project into the file at `path`. A broken entry it replaces is
/// backed up first; other broken entries are written back unchanged.
pub async fn save_project_at(path: &Path, key: &str, config: ProjectConfig) -> Result<(), AppError> {
    let mut file = load_project_file_at(path).await?;
    backup_before_replacing_at(path, &file, key).await?;
    file.upsert(key, config);
    write_project_file_at(path, &file).await
}

/// Replace one project with JSON from the editor. Nothing is written unless
/// the JSON is a valid project (see [`parse_project_raw`]).
pub async fn save_project_raw_at(path: &Path, key: &str, json: &str) -> Result<(), AppError> {
    let config = parse_project_raw(json).map_err(AppError::Config)?;
    save_project_at(path, key, config).await
}

/// Remove one project from the file at `path`, backing the file up first
/// when the project is broken.
pub async fn delete_project_at(path: &Path, key: &str) -> Result<(), AppError> {
    let mut file = load_project_file_at(path).await?;
    backup_before_replacing_at(path, &file, key).await?;
    file.remove(key);
    write_project_file_at(path, &file).await
}

/// Copy the file to `<name>.bak-YYYYMMDD-HHMMSS` (local time) next to it.
/// `None` when there is no file to back up. An existing backup is never
/// overwritten: a second backup within the same second keeps the first one
/// and returns its path.
pub async fn backup_project_file_at(path: &Path) -> Result<Option<PathBuf>, AppError> {
    let stamp = chrono::DateTime::<chrono::Local>::from(std::time::SystemTime::now())
        .format("%Y%m%d-%H%M%S")
        .to_string();
    backup_with_stamp(path, &stamp).await
}

async fn backup_with_stamp(path: &Path, stamp: &str) -> Result<Option<PathBuf>, AppError> {
    let name = path
        .file_name()
        .ok_or_else(|| AppError::Config(format!("Not a file path: {}", path.display())))?;
    let backup = dir_of(path).join(format!("{}.bak-{}", name.to_string_lossy(), stamp));
    let failed = |e: std::io::Error| {
        AppError::Config(format!("Failed to back up {}: {}", path.display(), e))
    };

    let contents = match tokio::fs::read(path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(failed(e)),
    };
    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .await
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(Some(backup)),
        Err(e) => return Err(failed(e)),
    };
    let written = async {
        file.write_all(&contents).await?;
        file.sync_all().await
    }
    .await;
    if let Err(e) = written {
        drop(file);
        let _ = tokio::fs::remove_file(&backup).await;
        return Err(failed(e));
    }
    Ok(Some(backup))
}

fn is_backup_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 15
        && b.iter().enumerate().all(|(i, c)| {
            if i == 8 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Backups of the file, newest first.
pub async fn list_backups_at(path: &Path) -> Vec<PathBuf> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let prefix = format!("{name}.bak-");
    let Ok(mut dir) = tokio::fs::read_dir(dir_of(path)).await else {
        return Vec::new();
    };
    let mut backups = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let file_name = entry.file_name();
        if let Some(stamp) = file_name.to_str().and_then(|f| f.strip_prefix(&prefix))
            && is_backup_stamp(stamp)
        {
            backups.push(entry.path());
        }
    }
    // The stamp sorts chronologically as text.
    backups.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    backups
}

/// The file's text, or an empty string when it does not exist.
pub async fn read_raw_at(path: &Path) -> Result<String, AppError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(AppError::Config(format!(
            "Failed to read {}: {}",
            path.display(),
            e
        ))),
    }
}

/// Ok when `text` is a JSON object, the only shape `projects.json` may have.
pub fn check_raw_file(text: &str) -> Result<(), FileError> {
    parse_object(text).map(|_| ())
}

/// Write the whole file from text, only if the text is a JSON object.
pub async fn write_raw_at(path: &Path, text: &str) -> Result<(), FileError> {
    check_raw_file(text)?;
    write_atomic(path, text).await.map_err(|e| FileError::Io {
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const GOOD: &str = r#"{"name":"Good","region":"us-east-1","profileFilter":null,"envPortMapping":{},"defaultPort":"5432","database":"db","secretPrefix":"rds!","rdsType":"cluster","rdsPattern":"c"}"#;

    /// A fresh, empty directory under the system temp dir, unique per call.
    fn temp_dir(test: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "connection-app-project-file-{}-{}-{}",
            test,
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn valid_project_with_field_problems_is_loaded_with_them() {
        let dir = temp_dir("problems");
        let path = dir.join("projects.json");
        write(
            &path,
            r#"{"p": {"name":"P","region":"nope","profileFilter":null,"envPortMapping":{},"defaultPort":"5432","database":"db","secretPrefix":"rds!","rdsType":"cluster","rdsPattern":"c"}}"#,
        );
        let file = load_project_file_at(&path).await.unwrap();
        match &file.entries["p"] {
            ProjectEntry::Valid { problems, .. } => {
                assert_eq!(problems.len(), 1);
                assert_eq!(problems[0].field, "region");
            }
            other => panic!("expected Valid, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn one_broken_project_does_not_hide_others() {
        let dir = temp_dir("broken");
        let path = dir.join("projects.json");
        write(
            &path,
            &format!(r#"{{"good": {GOOD}, "bad": {{"name": 5}}}}"#),
        );

        let file = load_project_file_at(&path).await.unwrap();

        match &file.entries["good"] {
            ProjectEntry::Valid { config, problems } => {
                assert_eq!(config.name, "Good");
                assert!(problems.is_empty());
            }
            other => panic!("good should be Valid, got {other:?}"),
        }
        match &file.entries["bad"] {
            ProjectEntry::Broken { error, .. } => {
                assert!(
                    error.starts_with("name: "),
                    "error should name the field: {error}"
                )
            }
            other => panic!("bad should be Broken, got {other:?}"),
        }
        let valid = file.valid_configs();
        assert_eq!(valid.len(), 1);
        assert!(valid.contains_key("good"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn broken_entry_survives_save_verbatim() {
        let dir = temp_dir("verbatim");
        let path = dir.join("projects.json");
        let bad = r#"{"name": 5,   "keep":  [1,2], "note": "  spaced  "}"#;
        write(&path, &format!("{{\"good\": {GOOD},\n\"bad\": {bad}}}"));

        let mut file = load_project_file_at(&path).await.unwrap();
        let mut changed = file.valid_configs()["good"].clone();
        changed.name = "Changed".into();
        file.upsert("good", changed);
        write_project_file_at(&path, &file).await.unwrap();

        let text = read(&path);
        assert!(text.contains(bad), "broken entry not verbatim:\n{text}");
        assert!(text.ends_with('\n'));

        let reloaded = load_project_file_at(&path).await.unwrap();
        assert_eq!(reloaded.valid_configs()["good"].name, "Changed");
        assert!(matches!(
            reloaded.entries["bad"],
            ProjectEntry::Broken { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_missing_and_whitespace_files_are_empty() {
        let dir = temp_dir("empty");
        let missing = dir.join("missing.json");
        assert!(
            load_project_file_at(&missing)
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        assert!(!missing.exists(), "loading must not create the file");

        for (name, text) in [("zero.json", ""), ("ws.json", "  \n\t \r\n")] {
            let path = dir.join(name);
            write(&path, text);
            let file = load_project_file_at(&path).await.unwrap();
            assert!(file.entries.is_empty(), "{name} should load as empty");
            assert_eq!(read(&path), text, "{name} must be untouched");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn top_level_array_is_not_an_object_and_file_untouched() {
        let dir = temp_dir("array");
        let path = dir.join("projects.json");
        let text = format!("[{GOOD}]\n");
        write(&path, &text);

        let err = load_project_file_at(&path).await.unwrap_err();
        assert!(matches!(err, FileError::NotAnObject), "got {err:?}");
        assert_eq!(read(&path), text);
        assert_eq!(file_names(&dir), vec!["projects.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn syntax_error_reports_line_and_column_and_file_untouched() {
        let dir = temp_dir("syntax");
        let path = dir.join("projects.json");
        let text = "{\n  \"a\": {,\n}";
        write(&path, text);

        let err = load_project_file_at(&path).await.unwrap_err();
        match &err {
            FileError::Syntax {
                line,
                column,
                message,
            } => {
                assert_eq!(*line, 2);
                assert!(*column > 0);
                assert!(!message.is_empty());
            }
            other => panic!("expected Syntax, got {other:?}"),
        }
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "syntax");
        assert_eq!(json["line"], 2);
        assert_eq!(read(&path), text);
        assert_eq!(file_names(&dir), vec!["projects.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn numeric_ports_are_normalized() {
        let dir = temp_dir("ports");
        let path = dir.join("projects.json");
        let text = r#"{"p": {"name": " P ", "region": "us-east-1", "profileFilter": null,
            "defaultPort": 5432, "envPortMapping": {"dev": 5462},
            "environments": [{"name": "dev", "accountId": "073813807522", "port": 5462}]}}"#;
        write(&path, text);

        let file = load_project_file_at(&path).await.unwrap();
        let c = &file.valid_configs()["p"];
        assert_eq!(c.default_port, "5432");
        assert_eq!(c.env_port_mapping["dev"], "5462");
        assert_eq!(c.environments.as_ref().unwrap()[0].port, "5462");
        assert_eq!(c.name, "P");
        assert_eq!(read(&path), text, "loading must not write the fixes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_describes_each_fix() {
        let mut v = serde_json::json!({
            "name": " P ",
            "region": "us-east-1",
            "defaultPort": 5432,
            "envPortMapping": {"dev": 5462},
            "environments": [{"name": "dev ", "accountId": "073813807522", "port": 5462}]
        });
        let fixes = normalize(&mut v);
        assert_eq!(fixes.len(), 5, "{fixes:?}");
        assert!(fixes.iter().any(|f| f.contains("defaultPort")));
        assert!(fixes.iter().any(|f| f.contains("envPortMapping.dev")));
        assert!(fixes.iter().any(|f| f.contains("environments[0].port")));
        assert!(fixes.iter().any(|f| f.contains("environments[0].name")));
        assert!(
            fixes
                .iter()
                .any(|f| f == "name: trimmed surrounding whitespace")
        );
        assert_eq!(v["defaultPort"], "5432");

        let mut clean = serde_json::json!({"name": "P", "defaultPort": "5432"});
        assert!(normalize(&mut clean).is_empty());
    }

    #[tokio::test]
    async fn atomic_write_leaves_no_tmp_file() {
        let dir = temp_dir("atomic");
        let path = dir.join("projects.json");
        write(&path, "old");

        write_atomic(&path, "new contents\n").await.unwrap();

        assert_eq!(read(&path), "new contents\n");
        assert_eq!(file_names(&dir), vec!["projects.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn atomic_write_through_symlink_keeps_the_link() {
        let dir = temp_dir("symlink");
        let real = dir.join("real.json");
        let link = dir.join("projects.json");
        write(&real, "old");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, "new").await.unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(read(&real), "new");
        assert_eq!(file_names(&dir), vec!["projects.json", "real.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backup_name_and_listing() {
        let dir = temp_dir("backup");
        let path = dir.join("projects.json");
        assert_eq!(backup_project_file_at(&path).await.unwrap(), None);

        write(&path, GOOD);
        write(&dir.join("projects.json.bak-20200101-000000"), "a");
        write(&dir.join("projects.json.bak-20210615-120000"), "b");
        write(&dir.join("projects.json.bak-notastamp"), "c");
        write(&dir.join("other.json.bak-20220101-000000"), "d");

        let backup = backup_project_file_at(&path).await.unwrap().unwrap();
        let name = backup.file_name().unwrap().to_str().unwrap().to_string();
        assert!(
            Regex::new(r"^projects\.json\.bak-\d{8}-\d{6}$")
                .unwrap()
                .is_match(&name),
            "bad backup name {name}"
        );
        assert_eq!(backup.parent(), Some(dir.as_path()));
        assert_eq!(read(&backup), GOOD);

        let listed = list_backups_at(&path).await;
        assert_eq!(
            listed,
            vec![
                backup,
                dir.join("projects.json.bak-20210615-120000"),
                dir.join("projects.json.bak-20200101-000000"),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_raw_refuses_invalid_json() {
        let dir = temp_dir("raw");
        let path = dir.join("projects.json");
        write(&path, "original");

        let err = write_raw_at(&path, "{ \"a\": ").await.unwrap_err();
        assert!(matches!(err, FileError::Syntax { .. }), "got {err:?}");
        let err = write_raw_at(&path, "[1, 2]").await.unwrap_err();
        assert!(matches!(err, FileError::NotAnObject), "got {err:?}");
        let err = write_raw_at(&path, "").await.unwrap_err();
        assert!(matches!(err, FileError::Syntax { .. }), "got {err:?}");
        assert_eq!(read(&path), "original");

        let text = format!("{{\"good\": {GOOD}}}\n");
        write_raw_at(&path, &text).await.unwrap();
        assert_eq!(read_raw_at(&path).await.unwrap(), text);
        assert_eq!(file_names(&dir), vec!["projects.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_raw_requires_an_object_and_remove_drops_entry() {
        let mut file = ProjectFile::default();
        assert!(file.upsert_raw("x", "[1]").is_err());
        assert!(file.upsert_raw("x", "{ nope").is_err());
        assert!(file.entries.is_empty());

        file.upsert_raw("good", GOOD).unwrap();
        file.upsert_raw("bad", r#"{"name": 5}"#).unwrap();
        assert!(matches!(file.entries["good"], ProjectEntry::Valid { .. }));
        assert!(matches!(file.entries["bad"], ProjectEntry::Broken { .. }));

        file.remove("good");
        assert!(!file.entries.contains_key("good"));
        let json = file.to_json();
        assert!(json.contains(r#"{"name": 5}"#));
        assert!(json.ends_with('\n'));
    }

    #[tokio::test]
    async fn a_second_backup_in_the_same_second_keeps_the_first() {
        let dir = temp_dir("backup-same-second");
        let path = dir.join("projects.json");
        write(&path, "first");
        let a = backup_with_stamp(&path, "20261029-101500")
            .await
            .unwrap()
            .unwrap();
        write(&path, "second");
        let b = backup_with_stamp(&path, "20261029-101500")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(read(&a), "first");
        assert_eq!(
            file_names(&dir),
            vec!["projects.json", "projects.json.bak-20261029-101500"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn backup_of_a_missing_file_is_none_and_creates_nothing() {
        let dir = temp_dir("backup-missing");
        let path = dir.join("projects.json");
        assert!(
            backup_with_stamp(&path, "20261029-101500")
                .await
                .unwrap()
                .is_none()
        );
        assert!(file_names(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_raw_file_accepts_only_an_object() {
        assert!(check_raw_file("{}").is_ok());
        assert!(matches!(check_raw_file("[]"), Err(FileError::NotAnObject)));
        assert!(matches!(
            check_raw_file("{ nope"),
            Err(FileError::Syntax { .. })
        ));
    }

    #[test]
    fn salvage_keeps_every_field_that_reads() {
        let raw = RawValue::from_string(
            r#"{"name":"Half","region":7,"defaultPort":5432,"rdsPattern":"c","remotePort":"x"}"#
                .to_string(),
        )
        .unwrap();
        let c = salvage_config(&raw).unwrap();
        assert_eq!(c.name, "Half");
        assert_eq!(c.region, "");
        assert_eq!(c.default_port, "5432");
        assert_eq!(c.rds_pattern, "c");
        assert_eq!(c.remote_port, None);

        let not_object = RawValue::from_string("[1]".to_string()).unwrap();
        assert!(salvage_config(&not_object).is_none());
    }

    fn backups_in(dir: &Path) -> Vec<String> {
        file_names(dir)
            .into_iter()
            .filter(|n| n.starts_with("projects.json.bak-"))
            .collect()
    }

    const BROKEN_FILE: &str = r#"{"bad": {"name": 5}, "good": GOOD_PLACEHOLDER}"#;

    fn broken_file() -> String {
        BROKEN_FILE.replace("GOOD_PLACEHOLDER", GOOD)
    }

    #[test]
    fn is_broken_only_for_broken_entries() {
        let mut file = ProjectFile::default();
        file.upsert_raw("good", GOOD).unwrap();
        file.upsert_raw("bad", r#"{"name": 5}"#).unwrap();
        assert!(file.is_broken("bad"));
        assert!(!file.is_broken("good"));
        assert!(!file.is_broken("missing"));
    }

    #[test]
    fn parse_project_raw_refuses_broken_or_invalid_projects() {
        assert!(parse_project_raw(GOOD).is_ok());
        let err = parse_project_raw(r#"{"name": 5}"#).unwrap_err();
        assert!(err.starts_with("Not saved:"), "{err}");
        let err = parse_project_raw(&GOOD.replace("us-east-1", "nope")).unwrap_err();
        assert!(err.contains("region"), "{err}");
        assert!(parse_project_raw("[1]").is_err());
        assert!(parse_project_raw("{ nope").unwrap_err().starts_with("Not valid JSON"));
    }

    #[tokio::test]
    async fn backup_before_replacing_backs_up_only_broken_entries() {
        let dir = temp_dir("backup-broken");
        let path = dir.join("projects.json");
        write(&path, &broken_file());
        let file = load_project_file_at(&path).await.unwrap();

        assert!(backup_before_replacing_at(&path, &file, "good").await.unwrap().is_none());
        assert!(backups_in(&dir).is_empty());

        let backup = backup_before_replacing_at(&path, &file, "bad").await.unwrap().unwrap();
        assert_eq!(read(&backup), broken_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn saving_over_a_broken_entry_backs_it_up_first() {
        let dir = temp_dir("save-over-broken");
        let path = dir.join("projects.json");
        write(&path, &broken_file());
        let config: ProjectConfig = serde_json::from_str(GOOD).unwrap();

        save_project_at(&path, "bad", config).await.unwrap();
        let backups = backups_in(&dir);
        assert_eq!(backups.len(), 1);
        assert_eq!(read(&dir.join(&backups[0])), broken_file());
        assert!(!load_project_file_at(&path).await.unwrap().is_broken("bad"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn saving_a_valid_entry_makes_no_backup() {
        let dir = temp_dir("save-valid");
        let path = dir.join("projects.json");
        write(&path, &broken_file());
        let config: ProjectConfig = serde_json::from_str(GOOD).unwrap();
        save_project_at(&path, "good", config).await.unwrap();
        assert!(backups_in(&dir).is_empty());
        // The broken entry is still written back.
        assert!(load_project_file_at(&path).await.unwrap().is_broken("bad"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn deleting_a_broken_entry_backs_it_up_first() {
        let dir = temp_dir("delete-broken");
        let path = dir.join("projects.json");
        write(&path, &broken_file());

        delete_project_at(&path, "bad").await.unwrap();
        let backups = backups_in(&dir);
        assert_eq!(backups.len(), 1);
        assert_eq!(read(&dir.join(&backups[0])), broken_file());
        let after = load_project_file_at(&path).await.unwrap();
        assert!(!after.entries.contains_key("bad"));
        assert!(after.entries.contains_key("good"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn raw_save_that_is_still_broken_writes_nothing() {
        let dir = temp_dir("raw-still-broken");
        let path = dir.join("projects.json");
        write(&path, &broken_file());

        let err = save_project_raw_at(&path, "bad", r#"{"name": 6}"#).await.unwrap_err();
        assert!(err.to_string().contains("Not saved"), "{err}");
        assert_eq!(read(&path), broken_file());
        assert!(backups_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn raw_save_that_fixes_a_broken_entry_backs_up_then_writes() {
        let dir = temp_dir("raw-fixes");
        let path = dir.join("projects.json");
        write(&path, &broken_file());

        save_project_raw_at(&path, "bad", GOOD).await.unwrap();
        let backups = backups_in(&dir);
        assert_eq!(backups.len(), 1);
        assert_eq!(read(&dir.join(&backups[0])), broken_file());
        let after = load_project_file_at(&path).await.unwrap();
        assert!(!after.is_broken("bad"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
