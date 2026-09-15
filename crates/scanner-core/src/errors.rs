//! Типы ошибок ядра.

use thiserror::Error;

/// Единая ошибка приложения.
#[derive(Debug, Error)]
pub enum ScannerError {
    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("SANE error: {0}")]
    Sane(String),

    #[error("failed to spawn scanimage: {0}")]
    Spawn(String),

    #[error("process timed out")]
    Timeout,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("image error: {0}")]
    Image(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("database error: {0}")]
    Database(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Other(String),
}

impl ScannerError {
    /// Стабильный код ошибки для протокола worker <-> GUI и журнала.
    pub fn code(&self) -> &'static str {
        match self {
            ScannerError::DeviceNotFound(_) => "device_not_found",
            ScannerError::Sane(_) => "sane",
            ScannerError::Spawn(_) => "spawn",
            ScannerError::Timeout => "timeout",
            ScannerError::Io(_) => "io",
            ScannerError::Image(_) => "image",
            ScannerError::Cancelled => "cancelled",
            ScannerError::Database(_) => "database",
            ScannerError::Unsupported(_) => "unsupported",
            ScannerError::Other(_) => "other",
        }
    }
}

pub type Result<T> = std::result::Result<T, ScannerError>;
