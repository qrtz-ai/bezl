//! qrtz SQLite storage, ported from the Electron worker's `node:sqlite` layer
//! (`packages/core/src/db/migrate.ts`, `packages/core/src/note/repo.ts`, `apps/desktop/src/worker/{db,handlers}.ts`).
//!
//! **Compatibility contract:** a database created by the Electron build opens here and vice versa. Both sides run
//! the same migration files (`packages/core/src/db/sql`, listed in `spec/schema/migrations.json`) with the same
//! `PRAGMA user_version` numbering, and both read/write the same row and JSON shapes (qrtz-model). The cross-open
//! tests (`tests/parity.rs` here, `apps/desktop/src/worker/db-parity.test.ts` in TS) prove it on shared fixtures.
//!
//! **Never run a plain `VACUUM`.** `meetings_fts` is keyed by the implicit `meetings.rowid`, which `VACUUM` may
//! renumber (meetings has a TEXT primary key). Only `VACUUM INTO` (snapshots) is used; a test enforces it until a
//! migration re-keys `meetings_fts`.

mod db;
mod error;
mod fts;
mod handle;
mod lifecycle;
mod migrate;
mod notes;
mod pending_deletes;
pub mod recordings;
mod row;

pub use db::{Db, DbHealth, ImportHooks, ImportReport};
pub use error::{Result, StorageError};
pub use fts::unsegmented as is_unsegmented;
pub use fts::{
    bigrams, chat_plan, search_plan, substring_snippet, to_chat_fts_query, to_fts_query, ChatPlan,
    SearchPlan, CHAT_MAX_BIGRAMS,
};
pub use handle::StorageHandle;
pub use lifecycle::LifecycleRepo;
pub use migrate::{migrate, schema_version, Migration, MigrationReport, MIGRATIONS};
pub use notes::{ListQuery, NoteRepo, SegmentHit, VectorRow, VectorSource, VectorTodo};
pub use pending_deletes::{PendingDeletes, PENDING_DELETES_FILE};
pub use qrtz_model as model;
