use rusqlite::Connection;

use crate::{Result, StorageError};

/// One shipped migration. `sql` is the shared file, byte-for-byte what TS runs.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

macro_rules! shared_sql {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/core/src/db/sql/",
            $file
        ))
    };
}

/// App migrations: the same numbers, names and SQL as `migrations` in packages/core/src/db/migrate.ts
/// (v2 keeps its shipped name "memo"). Append only; numbers go to whoever commits first; a Rust-only
/// migration is not allowed during the dual run. `tests/migrate.rs` checks this against
/// `spec/schema/migrations.json`.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "kv",
        sql: shared_sql!("0001_kv.sql"),
    },
    Migration {
        version: 2,
        name: "memo",
        sql: shared_sql!("0002_memo.sql"),
    },
    Migration {
        version: 3,
        name: "segment_vectors",
        sql: shared_sql!("0003_segment_vectors.sql"),
    },
    Migration {
        version: 4,
        name: "notes_templates",
        sql: shared_sql!("0004_notes_templates.sql"),
    },
    Migration {
        version: 5,
        name: "recording_lifecycle",
        sql: shared_sql!("0005_recording_lifecycle.sql"),
    },
    Migration {
        version: 6,
        name: "calendar",
        sql: shared_sql!("0006_calendar.sql"),
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from: u32,
    pub to: u32,
    /// `"<version>_<name>"` of each migration applied now.
    pub applied: Vec<String>,
}

/// `PRAGMA user_version`.
pub fn schema_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as u32)
}

/// Port of `migrate()`: applies pending migrations in order, each in its own `BEGIN IMMEDIATE` transaction that
/// also sets `user_version`. Idempotent. Refuses a database newer than the code, and gaps in the list.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<MigrationReport> {
    let mut sorted: Vec<Migration> = migrations.to_vec();
    sorted.sort_by_key(|m| m.version);
    for (i, m) in sorted.iter().enumerate() {
        if m.version as usize != i + 1 {
            return Err(StorageError::NonContiguous {
                got: m.version,
                at: i as u32 + 1,
            });
        }
    }
    let from = schema_version(conn)?;
    let latest = sorted.last().map_or(0, |m| m.version);
    if from > latest {
        return Err(StorageError::SchemaTooNew {
            db: from,
            app: latest,
        });
    }
    let mut applied = Vec::new();
    for m in sorted.iter().filter(|m| m.version > from) {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let run = conn
            .execute_batch(m.sql)
            .and_then(|_| conn.execute_batch(&format!("PRAGMA user_version = {}", m.version)))
            .and_then(|_| conn.execute_batch("COMMIT"));
        if let Err(source) = run {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(StorageError::MigrationFailed {
                version: m.version,
                name: m.name,
                source,
            });
        }
        applied.push(format!("{}_{}", m.version, m.name));
    }
    Ok(MigrationReport {
        from,
        to: schema_version(conn)?,
        applied,
    })
}
