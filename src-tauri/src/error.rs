use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    General(String),

    #[error("AWS error: {0}")]
    Aws(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("SSO error: {0}")]
    Sso(String),

    #[error("Tunnel error: {0}")]
    Tunnel(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

// Tauri commands require Serialize on error types
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::General(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::General(s.to_string())
    }
}
