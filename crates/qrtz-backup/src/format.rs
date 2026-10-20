//! The backup file format, shared with the Electron build (`apps/desktop/src/main/backup.ts`):
//!
//! ```text
//! db/qrtz.db          consistent VACUUM INTO snapshot (deflated)
//! audio/<name>.wav    recordings (stored: WAV barely compresses)
//! manifest.json       {format, version, createdAt, appVersion, schemaVersion, meetings, files[{path, sha256, bytes}]}
//! ```
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const BACKUP_FORMAT: &str = "qrtz-backup";
pub const BACKUP_VERSION: u32 = 1;
pub const DB_ENTRY: &str = "db/qrtz.db";
pub const MANIFEST_ENTRY: &str = "manifest.json";
/// `files` holds at most this many entries (zod `.max(100_000)`).
pub const MAX_MANIFEST_FILES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// `BackupManifest`. Field order is the TS `JSON.stringify` order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub created_at: serde_json::Number,
    pub app_version: String,
    pub schema_version: i64,
    pub meetings: i64,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    /// The zod schema's checks: literal format/version, integers, sha256 hex, at most 100 000 files.
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "constant regex literal; compiled by this crate's unit tests"
    )]
    pub fn validate(&self) -> Result<(), String> {
        static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[0-9a-f]{64}$").unwrap());
        if self.format != BACKUP_FORMAT || self.version != BACKUP_VERSION {
            return Err("unknown format".into());
        }
        if self.files.len() > MAX_MANIFEST_FILES {
            return Err("bad fields".into());
        }
        if self.files.iter().any(|f| !HEX.is_match(&f.sha256)) {
            return Err("bad sha256".into());
        }
        Ok(())
    }

    /// Serialized like TS `JSON.stringify(manifest, null, 2)`.
    #[allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "serde_json on a plain derived type with string keys cannot fail"
    )]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest serializes")
    }
}

#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "constant regex literal; compiled by this crate's unit tests"
)]
static SAFE_AUDIO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^audio/[A-Za-z0-9._-]{1,128}\.wav$").unwrap());

/// Only `db/qrtz.db` and `audio/<safe>.wav` are ever written or read: no traversal, no absolute paths, no
/// backslashes, no directories.
pub fn is_safe_entry(p: &str) -> bool {
    p == DB_ENTRY || is_safe_audio(p)
}

pub fn is_safe_audio(p: &str) -> bool {
    SAFE_AUDIO.is_match(p)
}
