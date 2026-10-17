use qrtz_model::{EpochMs, Meeting, MeetingStatus, PipelineState, RecordingState};
use rusqlite::{params, Connection, OptionalExtension};

use crate::notes::NoteRepo;
use crate::{Result, StorageError};

/// Durable recording lifecycle (migration 5): the session snapshot (`lifecycle_state`, key `session`) and one
/// `pipeline_state` row per meeting. Port of the worker's `lifecycle.*` / `pipeline.*` handlers.
///
/// Corruption rules match TS: valid JSON that doesn't fit the schema reads as absent (zod `safeParse`), while
/// unparseable JSON is an error (`JSON.parse` throws).
pub struct LifecycleRepo<'a> {
    pub(crate) conn: &'a Connection,
}

fn parse_lenient<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<Option<T>> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| StorageError::Corrupt(format!("{what}: {e}")))?;
    Ok(serde_json::from_value(v).ok())
}

impl<'a> LifecycleRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        LifecycleRepo { conn }
    }

    /// `lifecycle.saveSession` (TS stamps `Date.now()`; here the caller passes `now`).
    pub fn save_session(&self, s: &RecordingState, now: EpochMs) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO lifecycle_state (key,json,updated_at) VALUES ('session',?,?)
         ON CONFLICT(key) DO UPDATE SET json=excluded.json, updated_at=excluded.updated_at",
            )?
            .execute(params![serde_json::to_string(s)?, now])?;
        Ok(())
    }

    /// `lifecycle.loadSession`.
    pub fn load_session(&self) -> Result<Option<RecordingState>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT json FROM lifecycle_state WHERE key='session'", [], |r| {
                r.get(0)
            })
            .optional()?;
        match json {
            Some(j) => parse_lenient(&j, "lifecycle_state.session"),
            None => Ok(None),
        }
    }

    /// `pipeline.save`: `false` when the meeting was deleted meanwhile (F14), nothing written.
    pub fn save_pipeline(&self, p: &PipelineState, settled: bool) -> Result<bool> {
        if NoteRepo::new(self.conn).get_meeting(&p.meeting_id)?.is_none() {
            return Ok(false);
        }
        self.conn
            .prepare_cached(
                "INSERT INTO pipeline_state (meeting_id,json,settled,updated_at) VALUES (?,?,?,?)
         ON CONFLICT(meeting_id) DO UPDATE SET json=excluded.json, settled=excluded.settled, updated_at=excluded.updated_at",
            )?
            .execute(params![p.meeting_id, serde_json::to_string(p)?, settled as i64, p.updated_at])?;
        Ok(true)
    }

    /// `pipeline.get`.
    pub fn get_pipeline(&self, meeting_id: &str) -> Result<Option<PipelineState>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM pipeline_state WHERE meeting_id=?",
                [meeting_id],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(j) => parse_lenient(&j, "pipeline_state"),
            None => Ok(None),
        }
    }

    /// `pipeline.listUnsettled`: oldest update first; rows that don't fit the schema are skipped.
    pub fn list_unsettled(&self) -> Result<Vec<PipelineState>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT json FROM pipeline_state WHERE settled=0 ORDER BY updated_at")?;
        let rows: Vec<String> = st
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for j in rows {
            if let Some(p) = parse_lenient(&j, "pipeline_state")? {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// `pipeline.orphans`: meetings stuck in recording/processing with no pipeline row (pre-lifecycle crashes).
    pub fn orphans(&self) -> Result<Vec<Meeting>> {
        let mut out = Vec::new();
        for m in
            NoteRepo::new(self.conn).list_by_status(&[MeetingStatus::Recording, MeetingStatus::Processing])?
        {
            let has: Option<i64> = self
                .conn
                .query_row("SELECT 1 FROM pipeline_state WHERE meeting_id=?", [&m.id], |r| {
                    r.get(0)
                })
                .optional()?;
            if has.is_none() {
                out.push(m);
            }
        }
        Ok(out)
    }
}
