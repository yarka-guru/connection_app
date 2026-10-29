//! Stable local passwords for the IAM auth proxy, one per project + environment.
//!
//! Stored in `~/.connection-app/iam-proxy-passwords.json` with 0600 permissions.
//! Read-modify-write runs under an exclusive lock on a sibling lock file, so
//! the GUI and any number of CLI processes never lose each other's entries.

use crate::error::AppError;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "iam-proxy-passwords.json";
const LOCK_FILE_NAME: &str = ".iam-proxy-passwords.lock";
const PASSWORD_LEN: usize = 24;
const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

static FILE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn passwords_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".connection-app")
        .join(FILE_NAME)
}

/// The local proxy password for `project_key` + `environment`, created on first use.
///
/// When the environment has no entry yet, the first existing entry of
/// `legacy_profiles` (keyed `project:profile`) is copied to the environment key,
/// so a DB client that saved the old password keeps working. The old entry stays.
pub async fn local_proxy_password(
    project_key: &str,
    environment: &str,
    legacy_profiles: &[String],
) -> Result<String, AppError> {
    local_proxy_password_at(&passwords_path(), project_key, environment, legacy_profiles).await
}

async fn local_proxy_password_at(
    path: &Path,
    project_key: &str,
    environment: &str,
    legacy_profiles: &[String],
) -> Result<String, AppError> {
    let key = format!("{}:{}", project_key, environment);
    let legacy_keys: Vec<String> = legacy_profiles
        .iter()
        .map(|p| format!("{}:{}", project_key, p))
        .collect();
    let path = path.to_path_buf();
    let _guard = FILE_LOCK.lock().await;
    tokio::task::spawn_blocking(move || get_or_create(&path, &key, &legacy_keys))
        .await
        .map_err(|e| AppError::Config(format!("Local password task failed: {}", e)))?
}

fn is_valid_password(s: &str) -> bool {
    s.len() >= 16 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn generate_password() -> Result<String, AppError> {
    let mut out = String::with_capacity(PASSWORD_LEN);
    let mut buf = [0u8; 64];
    while out.len() < PASSWORD_LEN {
        getrandom::fill(&mut buf)
            .map_err(|e| AppError::General(format!("No secure randomness available: {}", e)))?;
        // 248 = 4 * 62: rejecting the rest keeps every character equally likely.
        for b in buf.iter().copied().filter(|&b| b < 248) {
            out.push(ALPHABET[(b % 62) as usize] as char);
            if out.len() == PASSWORD_LEN {
                break;
            }
        }
    }
    Ok(out)
}

fn get_or_create(path: &Path, key: &str, legacy_keys: &[String]) -> Result<String, AppError> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Config(format!("Invalid path: {}", path.display())))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::Config(format!("Failed to create {}: {}", dir.display(), e)))?;
    let lock_path = dir.join(LOCK_FILE_NAME);
    let mut lock_options = std::fs::OpenOptions::new();
    lock_options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        lock_options.mode(0o600);
    }
    let lock = lock_options
        .open(&lock_path)
        .map_err(|e| AppError::Config(format!("Failed to open {}: {}", lock_path.display(), e)))?;
    lock.lock()
        .map_err(|e| AppError::Config(format!("Failed to lock {}: {}", lock_path.display(), e)))?;
    let result = get_or_create_locked(path, key, legacy_keys);
    let _ = lock.unlock();
    result
}

fn get_or_create_locked(
    path: &Path,
    key: &str,
    legacy_keys: &[String],
) -> Result<String, AppError> {
    let mut passwords: BTreeMap<String, String> = match std::fs::read(path) {
        Ok(data) => match std::str::from_utf8(&data)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(text).map_err(|e| e.to_string()))
        {
            Ok(passwords) => passwords,
            Err(e) => {
                let backup = path.with_file_name(format!(
                    "{}.{}.bak",
                    FILE_NAME,
                    chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
                ));
                std::fs::rename(path, &backup).map_err(|re| {
                    AppError::Config(format!(
                        "{} is unreadable ({}) and could not be moved aside: {}",
                        path.display(),
                        e,
                        re
                    ))
                })?;
                log::warn!(
                    "{} was unreadable ({}); moved it to {} and started a new one",
                    path.display(),
                    e,
                    backup.display()
                );
                BTreeMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => {
            return Err(AppError::Config(format!(
                "Failed to read {}: {}",
                path.display(),
                e
            )));
        }
    };

    if let Some(existing) = passwords.get(key).filter(|p| is_valid_password(p)) {
        restrict_permissions(path);
        return Ok(existing.clone());
    }

    let password = match legacy_keys
        .iter()
        .find_map(|k| passwords.get(k).filter(|p| is_valid_password(p)))
    {
        Some(saved) => saved.clone(),
        None => generate_password()?,
    };
    passwords.insert(key.to_string(), password.clone());
    write_private(path, &serde_json::to_string_pretty(&passwords)?)?;
    Ok(password)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Write via a 0600 temp file in the same directory, then rename into place.
fn write_private(path: &Path, contents: &str) -> Result<(), AppError> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Config(format!("Invalid path: {}", path.display())))?;
    let tmp = dir.join(format!(".{}.{}.tmp", FILE_NAME, uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(contents.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::Config(format!(
            "Failed to write {}: {}",
            path.display(),
            e
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir()
            .join(format!("connection-app-test-{}", uuid::Uuid::new_v4()))
            .join(FILE_NAME)
    }

    #[test]
    fn password_is_stable_per_key_and_distinct_across_keys() {
        let path = temp_path();
        let a1 = get_or_create(&path, "covered:covered-dev", &[]).unwrap();
        let a2 = get_or_create(&path, "covered:covered-dev", &[]).unwrap();
        let b = get_or_create(&path, "covered:covered-staging", &[]).unwrap();
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_eq!(a1.len(), PASSWORD_LEN);
        assert!(is_valid_password(&a1));
        // Both entries survive the second write
        assert_eq!(
            get_or_create(&path, "covered:covered-dev", &[]).unwrap(),
            a1
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private_and_permissions_are_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path();
        get_or_create(&path, "p:x", &[]).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        get_or_create(&path, "p:x", &[]).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn concurrent_writers_never_lose_entries() {
        let path = temp_path();
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let key = format!("project-{}:profile", i);
                    (key.clone(), get_or_create(&path, &key, &[]).unwrap())
                })
            })
            .collect();
        let created: Vec<(String, String)> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();
        let stored: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (key, password) in created {
            assert_eq!(stored.get(&key), Some(&password), "entry {} was lost", key);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn unreadable_file_is_kept_as_a_backup() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let pw = get_or_create(&path, "p:x", &[]).unwrap();
        assert!(is_valid_password(&pw));

        let backups: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(backups.len(), 1, "{:?}", backups);
        assert!(
            backups[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("iam-proxy-passwords.json.")
        );
        assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "{ not json");
        let stored: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.get("p:x"), Some(&pw));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn non_utf8_file_is_kept_as_a_backup_too() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\xff\xfe{}").unwrap();
        let pw = get_or_create(&path, "p:x", &[]).unwrap();
        assert!(is_valid_password(&pw));
        let backups = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .to_string_lossy()
                    .ends_with(".bak")
            })
            .count();
        assert_eq!(backups, 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn invalid_stored_value_is_replaced() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"p:x": "short"}"#).unwrap();
        let pw = get_or_create(&path, "p:x", &[]).unwrap();
        assert!(is_valid_password(&pw));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn generated_passwords_use_the_alphabet() {
        let pw = generate_password().unwrap();
        assert_eq!(pw.len(), PASSWORD_LEN);
        assert!(pw.bytes().all(|b| ALPHABET.contains(&b)));
        assert_ne!(pw, generate_password().unwrap());
    }

    fn write_passwords(path: &Path, entries: &[(&str, &str)]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let map: BTreeMap<&str, &str> = entries.iter().copied().collect();
        std::fs::write(path, serde_json::to_string(&map).unwrap()).unwrap();
    }

    fn read_passwords(path: &Path) -> BTreeMap<String, String> {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    const SAVED: &str = "abcdefghijklmnopqrstuvwx";

    #[tokio::test]
    async fn migrated_environment_keeps_the_saved_client_password() {
        let path = temp_path();
        write_passwords(&path, &[("covered:covered-dev", SAVED)]);
        let legacy = vec!["covered-dev".to_string()];
        let pw = local_proxy_password_at(&path, "covered", "dev", &legacy)
            .await
            .unwrap();
        assert_eq!(pw, SAVED);
        let stored = read_passwords(&path);
        assert_eq!(stored.get("covered:dev").map(String::as_str), Some(SAVED));
        assert_eq!(
            stored.get("covered:covered-dev").map(String::as_str),
            Some(SAVED)
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn legacy_key_unchanged() {
        let path = temp_path();
        write_passwords(&path, &[("covered:covered-dev", SAVED)]);
        let legacy = vec!["covered-dev".to_string()];
        let pw = local_proxy_password_at(&path, "covered", "covered-dev", &legacy)
            .await
            .unwrap();
        assert_eq!(pw, SAVED);
        assert_eq!(read_passwords(&path).len(), 1);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn legacy_profiles_are_tried_in_order_and_new_key_wins() {
        let path = temp_path();
        let other = "ZYXWVUTSRQPONMLKJIHGFEDC";
        write_passwords(&path, &[("p:a", SAVED), ("p:b", other)]);
        let legacy = vec!["missing".to_string(), "b".to_string(), "a".to_string()];
        assert_eq!(
            local_proxy_password_at(&path, "p", "env", &legacy)
                .await
                .unwrap(),
            other
        );
        // Once copied, the environment key is authoritative.
        let legacy = vec!["a".to_string()];
        assert_eq!(
            local_proxy_password_at(&path, "p", "env", &legacy)
                .await
                .unwrap(),
            other
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn no_legacy_entry_creates_a_new_password() {
        let path = temp_path();
        let legacy = vec!["nope".to_string()];
        let pw = local_proxy_password_at(&path, "p", "env", &legacy)
            .await
            .unwrap();
        assert!(is_valid_password(&pw));
        let stored = read_passwords(&path);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored.get("p:env"), Some(&pw));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
