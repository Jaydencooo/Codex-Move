use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unified application error code for structured frontend handling and i18n
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    CodexHomeNotFound,
    CodexProcessRunning,
    UnsupportedDatabaseSchema,
    CorruptedSessionJsonl,
    CorruptedConfigToml,
    PathTraversalDetected,
    InvalidRelativePath,
    ExternalSymlinkBlocked,
    PathConflictDetected,
    UnsupportedGitWorktree,
    DecompressionLimitExceeded,
    DiskSpaceInsufficient,
    PairingExpired,
    PairingRateLimited,
    PairingAuthenticationFailed,
    TlsFingerprintMismatch,
    CapabilitiesIncompatible,
    TransferSnapshotMismatch,
    TransferInterrupted,
    BackupVerificationFailed,
    MigrationAbortedBySourceChange,
    RollbackFailedPartial,
    DatabaseError,
    IoError,
    SerializationError,
    NetworkError,
    CancelledByUser,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("[{code:?}] {message}")]
    Structured {
        code: ErrorCode,
        message: String,
        detail: Option<String>,
        recoverable: bool,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML edit error: {0}")]
    Toml(#[from] toml_edit::TomlError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
    pub recoverable: bool,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            detail: None,
            recoverable: false,
        }
    }

    pub fn with_detail(
        code: ErrorCode,
        message: impl Into<String>,
        detail: impl Into<String>,
        recoverable: bool,
    ) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            detail: Some(crate::logging::redact_sensitive_text(&detail.into())),
            recoverable,
        }
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Structured { code, .. } => *code,
            Self::Io(_) => ErrorCode::IoError,
            Self::Sqlite(_) => ErrorCode::DatabaseError,
            Self::Json(_) | Self::Toml(_) => ErrorCode::SerializationError,
        }
    }

    pub fn to_payload(&self) -> ErrorPayload {
        match self {
            Self::Structured {
                code,
                message,
                detail,
                recoverable,
            } => ErrorPayload {
                code: *code,
                message: crate::logging::redact_sensitive_text(message),
                detail: detail
                    .as_ref()
                    .map(|d| crate::logging::redact_sensitive_text(d)),
                recoverable: *recoverable,
            },
            Self::Io(err) => ErrorPayload {
                code: ErrorCode::IoError,
                message: "File system operation failed.".to_string(),
                detail: Some(crate::logging::redact_sensitive_text(&err.to_string())),
                recoverable: false,
            },
            Self::Sqlite(err) => ErrorPayload {
                code: ErrorCode::DatabaseError,
                message: "Database operation failed.".to_string(),
                detail: Some(crate::logging::redact_sensitive_text(&err.to_string())),
                recoverable: false,
            },
            Self::Json(err) => ErrorPayload {
                code: ErrorCode::SerializationError,
                message: "JSON parsing or serialization failed.".to_string(),
                detail: Some(crate::logging::redact_sensitive_text(&err.to_string())),
                recoverable: false,
            },
            Self::Toml(err) => ErrorPayload {
                code: ErrorCode::CorruptedConfigToml,
                message: "TOML configuration parsing failed.".to_string(),
                detail: Some(crate::logging::redact_sensitive_text(&err.to_string())),
                recoverable: false,
            },
        }
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_payload().serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
