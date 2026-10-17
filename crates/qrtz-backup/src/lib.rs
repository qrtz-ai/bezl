//! qrtz meeting export and backup, ported from the Electron build:
//!
//! - [`markdown`]: `note:export` (`packages/core/src/note/markdown.ts`), byte-identical on the shared vectors in
//!   `spec/vectors/export/markdown.json`.
//! - [`backup`]: `data:export-backup` / `data:import-backup` (`apps/desktop/src/main/backup.ts` + the worker's
//!   `data.snapshot` / `data.importDb`), same zip format ([`format`]), so a backup made by either build imports
//!   into the other (`spec/fixtures/backup`).
//!
//! Blocking, synchronous APIs over a `&Db`: run them inside `StorageHandle::call` (the single writer thread),
//! like the TS worker. See docs/rust-core-api.md §9 and ADR 0042.
pub mod backup;
pub mod format;
pub mod markdown;

pub use backup::{
    export_backup, import_backup, no_progress, write_backup, ExportBackup, ExportReport, ImportBackup,
    ImportLimits, ImportReport, Phase, Progress, WriteBackup, WriteReport,
};
pub use format::{Manifest, ManifestFile, BACKUP_FORMAT, DB_ENTRY};
pub use markdown::{export_filename, export_meeting, meeting_to_markdown, slugify, MeetingExport};
