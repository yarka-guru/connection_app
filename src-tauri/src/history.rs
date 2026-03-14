use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

const MAX_HISTORY_SIZE: usize = 10_000;
const TRUNCATE_TO: usize = 5_000;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HistoryEntry {
    pub timestamp: String,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "connectionId")]
    pub connection_id: String,
    #[serde(rename = "projectKey")]
    pub project_key: String,
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

fn history_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".connection-app")
        .join("history.jsonl")
}

/// Append a history entry as a JSON line to the history file.
/// Creates the directory and file if they don't exist.
/// If the file exceeds MAX_HISTORY_SIZE lines, truncates to keep the last TRUNCATE_TO entries.
pub async fn log_event(entry: HistoryEntry) {
    if let Err(e) = log_event_inner(entry).await {
        log::warn!("Failed to write history entry: {}", e);
    }
}

async fn log_event_inner(entry: HistoryEntry) -> Result<(), Box<dyn std::error::Error>> {
    let path = history_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Serialize the entry
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');

    // Append to file
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await?;

    // Check line count and truncate if needed
    let contents = tokio::fs::read_to_string(&path).await?;
    let line_count = contents.lines().count();

    if line_count > MAX_HISTORY_SIZE {
        let lines: Vec<&str> = contents.lines().collect();
        let keep = &lines[lines.len() - TRUNCATE_TO..];
        let truncated = keep.join("\n") + "\n";
        tokio::fs::write(&path, truncated).await?;
        log::info!(
            "History file truncated from {} to {} entries",
            line_count,
            TRUNCATE_TO
        );
    }

    Ok(())
}

/// Read the last `limit` entries from the history file.
pub async fn read_history(limit: usize) -> Vec<HistoryEntry> {
    match read_history_inner(limit).await {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("Failed to read history: {}", e);
            Vec::new()
        }
    }
}

async fn read_history_inner(
    limit: usize,
) -> Result<Vec<HistoryEntry>, Box<dyn std::error::Error>> {
    let path = history_path();

    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = tokio::fs::read_to_string(&path).await?;
    let entries: Vec<HistoryEntry> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str(line) {
            Ok(entry) => Some(entry),
            Err(e) => {
                log::warn!("Skipping malformed history line: {}", e);
                None
            }
        })
        .collect();

    // Return the last `limit` entries (most recent)
    let start = entries.len().saturating_sub(limit);
    Ok(entries[start..].to_vec())
}

/// Delete the history file.
pub async fn clear_history() {
    let path = history_path();
    if path.exists()
        && let Err(e) = tokio::fs::remove_file(&path).await
    {
        log::warn!("Failed to clear history file: {}", e);
    }
}
