use thiserror::Error;

pub type Result<T, E = StorageError> = std::result::Result<T, E>;

/// Storage failures. `code()`/`retryable()` map onto the IPC `AppError` codes (packages/core/src/errors.ts).
/// Messages keep the TS wording where callers/tests match on it.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("{what} {id} not found")]
    NotFound { what: &'static str, id: String },
    #[error("Database schema v{db} is newer than this app (v{app})")]
    SchemaTooNew { db: u32, app: u32 },
    #[error("Migrations must be contiguous from 1; got {got} at #{at}")]
    NonContiguous { got: u32, at: u32 },
    #[error("Migration {version}_{name} failed: {source}")]
    MigrationFailed {
        version: u32,
        name: &'static str,
        source: rusqlite::Error,
    },
    #[error("{0}")]
    Validation(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("storage thread stopped")]
    Closed,
    #[error("storage task panicked: {0}")]
    Panicked(String),
}

impl StorageError {
    fn busy(&self) -> bool {
        matches!(
            self,
            StorageError::Sqlite(rusqlite::Error::SqliteFailure(e, _))
                if matches!(e.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
        )
    }

    /// The `AppError` code (`E_NOT_FOUND`, `E_VALIDATION`, `E_UNAVAILABLE`, `E_INTERNAL`).
    pub fn code(&self) -> &'static str {
        match self {
            StorageError::NotFound { .. } => "E_NOT_FOUND",
            StorageError::SchemaTooNew { .. }
            | StorageError::NonContiguous { .. }
            | StorageError::Validation(_)
            | StorageError::Corrupt(_) => "E_VALIDATION",
            StorageError::Closed => "E_UNAVAILABLE",
            e if e.busy() => "E_UNAVAILABLE",
            _ => "E_INTERNAL",
        }
    }

    /// Whether retrying can help (busy/locked DB, a restarting storage thread, a panicked task).
    pub fn retryable(&self) -> bool {
        matches!(self, StorageError::Closed | StorageError::Panicked(_)) || self.busy()
    }
}
