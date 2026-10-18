use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModuleError {
    #[error("manifest is not valid json: {0}")]
    Json(String),
    #[error("{field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("{0}: a version range is not allowed; pin an exact version (ADR 0146)")]
    VersionRange(String),
    #[error(
        "{id}: kind {kind:?} is code loaded into the module; shared code must be an xpc-service (ADR 0146)"
    )]
    InProcessCode { id: String, kind: String },
    #[error("{id}@{version}: not in the lockfile")]
    Missing { id: String, version: String },
    #[error("{id}@{version}: {field} differs from the lockfile")]
    Mismatch {
        id: String,
        version: String,
        field: &'static str,
    },
    #[error("{id}@{version}: already installed with different content; an installed version never changes")]
    ContentChanged { id: String, version: String },
    #[error("{0}: packages can't run anything at install time; qrtz has no install scripts (ADR 0157)")]
    InstallScript(String),
    #[error("lockfile: {0}")]
    Lock(String),
    #[error("io: {0}")]
    Io(String),
}

impl ModuleError {
    pub fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl From<std::io::Error> for ModuleError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
