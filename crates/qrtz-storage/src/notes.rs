use indexmap::IndexMap;
use qrtz_model::{
    EpochMs, Meeting, MeetingDetail, MeetingListItem, MeetingNote, MeetingPlatform, MeetingStatus,
    MeetingSummary, Ms, SearchHit, Segment, SegmentSource,
};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::fts::{js_trim, to_chat_fts_query, to_fts_query};
use crate::row::{blob, i64_, opt_f64, opt_i64, opt_string, string};
use crate::{Result, StorageError};

/// `note:list` request (`note.list` worker handler).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Page size (default 100). Up to limit+1 rows come back; the extra one signals more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
}

/// A transcript line hit for chat retrieval (`searchSegments`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentHit {
    pub meeting_id: String,
    pub title: String,
    pub started_at: EpochMs,
    pub start_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
}

/// A segment that still needs a vector (`missingVectors`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorTodo {
    pub rowid: i64,
    pub meeting_id: String,
    pub text: String,
}

/// One vector to store (`saveVectors`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorRow {
    pub rowid: i64,
    pub meeting_id: String,
    pub vec: Vec<u8>,
}

/// An indexed line with its vector (`vectorSources`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorSource {
    pub meeting_id: String,
    pub title: String,
    pub started_at: EpochMs,
    pub start_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub text: String,
    pub vec: Vec<u8>,
}

/// Meetings, segments, notes, summaries, vectors and FTS search: a 1:1 port of `NoteRepo` (repo.ts) plus the
/// `note.list` / `note.get` compositions of the worker.
pub struct NoteRepo<'a> {
    pub(crate) conn: &'a Connection,
}

fn to_meeting(r: &Row) -> Result<Meeting> {
    let platform = string(r, "platform")?;
    let status = string(r, "status")?;
    let participants = opt_string(r, "participants")?.unwrap_or_else(|| "[]".into());
    Ok(Meeting {
        id: string(r, "id")?,
        title: string(r, "title")?,
        platform: MeetingPlatform::parse(&platform)
            .ok_or_else(|| StorageError::Corrupt(format!("meeting platform {platform:?}")))?,
        started_at: i64_(r, "started_at")?,
        ended_at: opt_i64(r, "ended_at")?,
        status: MeetingStatus::parse(&status)
            .ok_or_else(|| StorageError::Corrupt(format!("meeting status {status:?}")))?,
        participants: serde_json::from_str(&participants)?,
        audio_path: opt_string(r, "audio_path")?,
        duration_ms: opt_i64(r, "duration_ms")?,
        // Column exists from migration 4 on (always present after migrate).
        template: r
            .as_ref()
            .column_index("template")
            .ok()
            .map(|_| opt_string(r, "template"))
            .transpose()?
            .flatten(),
    })
}

fn to_segment(r: &Row) -> Result<Segment> {
    let source = string(r, "source")?;
    Ok(Segment {
        id: string(r, "id")?,
        start_ms: i64_(r, "start_ms")?,
        end_ms: i64_(r, "end_ms")?,
        text: string(r, "text")?,
        source: SegmentSource::parse(&source)
            .ok_or_else(|| StorageError::Corrupt(format!("segment source {source:?}")))?,
        speaker: opt_string(r, "speaker")?,
        speaker_name: opt_string(r, "speaker_name")?,
        confidence: opt_f64(r, "confidence")?,
    })
}

fn collect<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    f: impl Fn(&Row) -> Result<T>,
) -> Result<Vec<T>> {
    let mut st = conn.prepare_cached(sql)?;
    let mut rows = st.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(f(r)?);
    }
    Ok(out)
}

impl<'a> NoteRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        NoteRepo { conn }
    }

    /// `tx`: BEGIN/COMMIT, ROLLBACK on error (SQLite has no nested BEGIN; callers must not nest).
    fn tx<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN")?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Insert or update (template is set separately, like TS).
    pub fn upsert_meeting(&self, m: &Meeting) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO meetings (id,title,platform,started_at,ended_at,status,participants,audio_path,duration_ms)
         VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, platform=excluded.platform, ended_at=excluded.ended_at,
           status=excluded.status, participants=excluded.participants, audio_path=excluded.audio_path, duration_ms=excluded.duration_ms",
            )?
            .execute(params![
                m.id,
                m.title,
                m.platform.as_str(),
                m.started_at,
                m.ended_at,
                m.status.as_str(),
                serde_json::to_string(&m.participants)?,
                m.audio_path,
                m.duration_ms,
            ])?;
        Ok(())
    }

    pub fn set_template(&self, id: &str, template: &str) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE meetings SET template=? WHERE id=?")?
            .execute(params![template, id])?;
        Ok(())
    }

    pub fn get_notes(&self, meeting_id: &str) -> Result<Vec<MeetingNote>> {
        collect(
            self.conn,
            "SELECT * FROM notes WHERE meeting_id=? ORDER BY at_ms, created_at",
            [meeting_id],
            |r| {
                Ok(MeetingNote {
                    id: string(r, "id")?,
                    at_ms: i64_(r, "at_ms")?,
                    text: string(r, "text")?,
                    created_at: i64_(r, "created_at")?,
                })
            },
        )
    }

    pub fn add_note(&self, meeting_id: &str, n: &MeetingNote) -> Result<()> {
        self.conn
            .prepare_cached("INSERT INTO notes (id,meeting_id,at_ms,text,created_at) VALUES (?,?,?,?,?)")?
            .execute(params![n.id, meeting_id, n.at_ms, n.text, n.created_at])?;
        Ok(())
    }

    pub fn update_note(&self, id: &str, text: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("UPDATE notes SET text=? WHERE id=?")?
            .execute(params![text, id])?
            > 0)
    }

    pub fn delete_note(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("DELETE FROM notes WHERE id=?")?
            .execute([id])?
            > 0)
    }

    pub fn set_status(&self, id: &str, status: MeetingStatus) -> Result<()> {
        self.conn
            .prepare_cached("UPDATE meetings SET status=? WHERE id=?")?
            .execute(params![status.as_str(), id])?;
        Ok(())
    }

    pub fn get_meeting(&self, id: &str) -> Result<Option<Meeting>> {
        let mut st = self.conn.prepare_cached("SELECT * FROM meetings WHERE id=?")?;
        let mut rows = st.query([id])?;
        rows.next()?.map(to_meeting).transpose()
    }

    /// `need()`: the meeting or `NotFound` ("meeting <id> not found", E_NOT_FOUND).
    pub fn need(&self, id: &str) -> Result<Meeting> {
        self.get_meeting(id)?.ok_or_else(|| StorageError::NotFound {
            what: "meeting",
            id: id.to_string(),
        })
    }

    /// Newest first.
    pub fn list_meetings(&self, limit: u32, offset: u32) -> Result<Vec<Meeting>> {
        collect(
            self.conn,
            "SELECT * FROM meetings ORDER BY started_at DESC LIMIT ? OFFSET ?",
            params![limit, offset],
            to_meeting,
        )
    }

    /// Transcript lines matching any query term (FTS, best first), for chat retrieval across meetings.
    pub fn search_segments(&self, input: &str, limit: u32) -> Result<Vec<SegmentHit>> {
        let Some(q) = to_chat_fts_query(input) else {
            return Ok(vec![]);
        };
        collect(
            self.conn,
            "SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
                  COALESCE(s.speaker_name, s.speaker) AS speaker, s.text AS text
           FROM segments_fts JOIN segments s ON s.rowid = segments_fts.rowid JOIN meetings m ON m.id = s.meeting_id
           WHERE segments_fts MATCH ? ORDER BY bm25(segments_fts) LIMIT ?",
            params![q, limit],
            |r| {
                Ok(SegmentHit {
                    meeting_id: string(r, "meetingId")?,
                    title: string(r, "title")?,
                    started_at: i64_(r, "startedAt")?,
                    start_ms: i64_(r, "startMs")?,
                    speaker: opt_string(r, "speaker")?,
                    text: string(r, "text")?,
                })
            },
        )
    }

    /// Segments with no vector for `model` yet (optionally one meeting), newest rows first.
    pub fn missing_vectors(
        &self,
        model: &str,
        limit: u32,
        meeting_id: Option<&str>,
    ) -> Result<Vec<VectorTodo>> {
        let map = |r: &Row| {
            Ok(VectorTodo {
                rowid: i64_(r, "rowid")?,
                meeting_id: string(r, "meetingId")?,
                text: string(r, "text")?,
            })
        };
        const BASE: &str =
            "SELECT s.rowid AS rowid, s.meeting_id AS meetingId, s.text AS text FROM segments s
           LEFT JOIN segment_vectors v ON v.segment_rowid = s.rowid AND v.model = ?
           WHERE v.segment_rowid IS NULL";
        match meeting_id {
            Some(id) => collect(
                self.conn,
                &format!("{BASE} AND s.meeting_id = ? ORDER BY s.rowid DESC LIMIT ?"),
                params![model, id, limit],
                map,
            ),
            None => collect(
                self.conn,
                &format!("{BASE} ORDER BY s.rowid DESC LIMIT ?"),
                params![model, limit],
                map,
            ),
        }
    }

    pub fn save_vectors(&self, model: &str, rows: &[VectorRow]) -> Result<()> {
        self.tx(|| {
            let mut st = self.conn.prepare_cached(
                "INSERT OR REPLACE INTO segment_vectors (segment_rowid, meeting_id, model, vec) VALUES (?,?,?,?)",
            )?;
            for r in rows {
                st.execute(params![r.rowid, r.meeting_id, model, r.vec])?;
            }
            Ok(())
        })
    }

    /// Indexed transcript lines with their vectors (one meeting, or all).
    pub fn vector_sources(&self, model: &str, meeting_id: Option<&str>) -> Result<Vec<VectorSource>> {
        let map = |r: &Row| {
            Ok(VectorSource {
                meeting_id: string(r, "meetingId")?,
                title: string(r, "title")?,
                started_at: i64_(r, "startedAt")?,
                start_ms: i64_(r, "startMs")?,
                speaker: opt_string(r, "speaker")?,
                text: string(r, "text")?,
                vec: blob(r, "vec")?,
            })
        };
        const BASE: &str =
            "SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
                  COALESCE(s.speaker_name, s.speaker) AS speaker, s.text AS text, v.vec AS vec
           FROM segment_vectors v JOIN segments s ON s.rowid = v.segment_rowid JOIN meetings m ON m.id = s.meeting_id
           WHERE v.model = ?";
        match meeting_id {
            Some(id) => collect(
                self.conn,
                &format!("{BASE} AND v.meeting_id = ?"),
                params![model, id],
                map,
            ),
            None => collect(self.conn, BASE, [model], map),
        }
    }

    /// Rename a speaker in one meeting (merge = rename onto an existing name). Returns lines changed.
    pub fn rename_speaker(&self, meeting_id: &str, from: &str, to: &str) -> Result<u32> {
        self.tx(|| {
            let changed = self
                .conn
                .prepare_cached(
                    "UPDATE segments SET speaker_name=? WHERE meeting_id=? AND COALESCE(speaker_name, speaker)=?",
                )?
                .execute(params![to, meeting_id, from])?;
            let ps: Option<String> = self
                .conn
                .query_row("SELECT participants FROM meetings WHERE id=?", [meeting_id], |r| r.get(0))
                .optional()?;
            if let Some(ps) = ps {
                let list: Vec<String> = serde_json::from_str(if ps.is_empty() { "[]" } else { &ps })?;
                let mut seen = indexmap::IndexSet::new();
                for p in list {
                    seen.insert(if p == from { to.to_string() } else { p });
                }
                let out: Vec<String> = seen.into_iter().collect();
                self.conn
                    .prepare_cached("UPDATE meetings SET participants=? WHERE id=?")?
                    .execute(params![serde_json::to_string(&out)?, meeting_id])?;
            }
            Ok(changed as u32)
        })
    }

    /// Meetings in the given states regardless of age (crash recovery must not miss old ones), oldest first.
    pub fn list_by_status(&self, statuses: &[MeetingStatus]) -> Result<Vec<Meeting>> {
        if statuses.is_empty() {
            return Ok(vec![]);
        }
        let marks = vec!["?"; statuses.len()].join(",");
        collect(
            self.conn,
            &format!("SELECT * FROM meetings WHERE status IN ({marks}) ORDER BY started_at"),
            params_from_iter(statuses.iter().map(|s| s.as_str())),
            to_meeting,
        )
    }

    /// Deletes the meeting, its segments and its `meetings_fts` row; FK cascades take summaries, notes, vectors and
    /// pipeline_state. Audio files are left on disk (TS parity; see docs/rust-core-api.md §6.7).
    pub fn delete_meeting(&self, id: &str) -> Result<()> {
        self.tx(|| {
            let rowid: Option<i64> = self
                .conn
                .query_row("SELECT rowid AS r FROM meetings WHERE id=?", [id], |r| r.get(0))
                .optional()?;
            if let Some(r) = rowid {
                self.conn
                    .prepare_cached("DELETE FROM meetings_fts WHERE rowid=?")?
                    .execute([r])?;
            }
            self.conn
                .prepare_cached("DELETE FROM segments WHERE meeting_id=?")?
                .execute([id])?;
            self.conn
                .prepare_cached("DELETE FROM meetings WHERE id=?")?
                .execute([id])?;
            Ok(())
        })
    }

    /// Replaces all segments of one source for a meeting (idempotent re-processing).
    pub fn replace_segments(&self, meeting_id: &str, source: SegmentSource, segs: &[Segment]) -> Result<()> {
        self.tx(|| {
            self.conn
                .prepare_cached("DELETE FROM segments WHERE meeting_id=? AND source=?")?
                .execute(params![meeting_id, source.as_str()])?;
            let mut ins = self.conn.prepare_cached(
                "INSERT INTO segments (id,meeting_id,source,start_ms,end_ms,speaker,speaker_name,text,confidence) VALUES (?,?,?,?,?,?,?,?,?)",
            )?;
            for s in segs {
                ins.execute(params![
                    s.id,
                    meeting_id,
                    source.as_str(),
                    s.start_ms,
                    s.end_ms,
                    s.speaker,
                    s.speaker_name,
                    s.text,
                    s.confidence
                ])?;
            }
            Ok(())
        })
    }

    /// Idempotent upsert of one live line on (meeting, source, id).
    pub fn append_segment(&self, meeting_id: &str, s: &Segment) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO segments (id,meeting_id,source,start_ms,end_ms,speaker,speaker_name,text,confidence) VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(meeting_id, source, id) DO UPDATE SET text=excluded.text, end_ms=excluded.end_ms, speaker_name=excluded.speaker_name",
            )?
            .execute(params![
                s.id,
                meeting_id,
                s.source.as_str(),
                s.start_ms,
                s.end_ms,
                s.speaker,
                s.speaker_name,
                s.text,
                s.confidence
            ])?;
        Ok(())
    }

    /// Post-processed transcript if present, else live.
    pub fn get_segments(&self, meeting_id: &str) -> Result<Vec<Segment>> {
        let post = collect(
            self.conn,
            "SELECT * FROM segments WHERE meeting_id=? AND source='post' ORDER BY start_ms",
            [meeting_id],
            to_segment,
        )?;
        if !post.is_empty() {
            return Ok(post);
        }
        collect(
            self.conn,
            "SELECT * FROM segments WHERE meeting_id=? AND source='live' ORDER BY start_ms",
            [meeting_id],
            to_segment,
        )
    }

    /// Upserts the summary and rebuilds the meeting's `meetings_fts` row (title + summary + action items).
    pub fn save_summary(&self, meeting_id: &str, s: &MeetingSummary, now: EpochMs) -> Result<()> {
        self.tx(|| {
            self.conn
                .prepare_cached(
                    "INSERT INTO summaries (meeting_id,provider,json,created_at) VALUES (?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET provider=excluded.provider, json=excluded.json, created_at=excluded.created_at",
                )?
                .execute(params![meeting_id, s.provider, serde_json::to_string(s)?, now])?;
            let title = self.get_meeting(meeting_id)?.map(|m| m.title).unwrap_or_default();
            let rowid: i64 = self
                .conn
                .query_row("SELECT rowid AS r FROM meetings WHERE id=?", [meeting_id], |r| r.get(0))
                .optional()?
                .ok_or_else(|| StorageError::NotFound { what: "meeting", id: meeting_id.to_string() })?;
            self.conn.prepare_cached("DELETE FROM meetings_fts WHERE rowid=?")?.execute([rowid])?;
            let text = std::iter::once(s.summary.as_str())
                .chain(s.action_items.iter().map(|a| a.text.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            self.conn
                .prepare_cached("INSERT INTO meetings_fts(rowid,title,summary) VALUES (?,?,?)")?
                .execute(params![rowid, title, text])?;
            Ok(())
        })
    }

    pub fn get_summary(&self, meeting_id: &str) -> Result<Option<MeetingSummary>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT json FROM summaries WHERE meeting_id=?",
                [meeting_id],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|j| {
            serde_json::from_str(&j).map_err(|e| StorageError::Corrupt(format!("summary {meeting_id}: {e}")))
        })
        .transpose()
    }

    /// Full-text search across transcripts, titles and summaries. One best hit per meeting, best first.
    /// Tie-breaking matches TS exactly: title hits, then summary hits, then transcript hits, keeping the first
    /// strictly-better rank per meeting in insertion order, then a stable sort by rank.
    pub fn search(&self, input: &str, limit: u32) -> Result<Vec<SearchHit>> {
        let Some(q) = to_fts_query(input) else {
            return Ok(vec![]);
        };
        let hit = |r: &Row, with_start: bool| -> Result<(SearchHit, f64)> {
            Ok((
                SearchHit {
                    meeting_id: string(r, "meetingId")?,
                    title: string(r, "title")?,
                    started_at: i64_(r, "startedAt")?,
                    snippet: string(r, "snippet")?,
                    start_ms: if with_start { opt_i64(r, "startMs")? } else { None },
                },
                opt_f64(r, "rank")?.unwrap_or(0.0),
            ))
        };
        let seg_hits = collect(
            self.conn,
            "SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
                snippet(segments_fts, 0, '[', ']', '…', 12) AS snippet, bm25(segments_fts) AS rank
         FROM segments_fts JOIN segments s ON s.rowid = segments_fts.rowid JOIN meetings m ON m.id = s.meeting_id
         WHERE segments_fts MATCH ? ORDER BY rank LIMIT 500",
            [&q],
            |r| hit(r, true),
        )?;
        let meta_hits = collect(
            self.conn,
            "SELECT m.id AS meetingId, m.title AS title, m.started_at AS startedAt,
                snippet(meetings_fts, -1, '[', ']', '…', 12) AS snippet, bm25(meetings_fts) AS rank
         FROM meetings_fts JOIN meetings m ON m.rowid = meetings_fts.rowid WHERE meetings_fts MATCH ? ORDER BY rank LIMIT 100",
            [&q],
            |r| hit(r, false),
        )?;
        // Also match titles of meetings without a summary yet.
        let needle = js_trim(input).to_lowercase();
        let title_hits = if needle.is_empty() {
            vec![]
        } else {
            collect(self.conn, "SELECT id, title, started_at FROM meetings", [], |r| {
                Ok((string(r, "id")?, string(r, "title")?, i64_(r, "started_at")?))
            })?
            .into_iter()
            .filter(|(_, title, _)| title.to_lowercase().contains(&needle))
            .map(|(id, title, started_at)| {
                (
                    SearchHit {
                        meeting_id: id,
                        snippet: title.clone(),
                        title,
                        started_at,
                        start_ms: None,
                    },
                    -1e9,
                )
            })
            .collect()
        };
        let mut best: IndexMap<String, (SearchHit, f64)> = IndexMap::new();
        for (h, rank) in title_hits.into_iter().chain(meta_hits).chain(seg_hits) {
            match best.get(&h.meeting_id) {
                Some((_, cur)) if rank >= *cur => {} // ranks are finite (bm25 / -1e9)
                _ => {
                    // Map.set on an existing key keeps its original position, like IndexMap::insert.
                    best.insert(h.meeting_id.clone(), (h, rank));
                }
            }
        }
        let mut out: Vec<(SearchHit, f64)> = best.into_values().collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out.into_iter().take(limit as usize).map(|(h, _)| h).collect())
    }

    /// `note.list`: newest-first page, or search results when `query` is non-blank. Up to `limit + 1` rows.
    pub fn list(&self, q: &ListQuery) -> Result<Vec<MeetingListItem>> {
        let limit = q.limit.unwrap_or(100);
        let offset = q.offset.unwrap_or(0);
        let query = q.query.as_deref().map(js_trim).unwrap_or("");
        if query.is_empty() {
            return Ok(self
                .list_meetings(limit + 1, offset)?
                .into_iter()
                .map(|meeting| MeetingListItem { meeting, hit: None })
                .collect());
        }
        let mut out = Vec::new();
        for hit in self
            .search(query, offset + limit + 1)?
            .into_iter()
            .skip(offset as usize)
        {
            if let Some(meeting) = self.get_meeting(&hit.meeting_id)? {
                out.push(MeetingListItem {
                    meeting,
                    hit: Some(hit),
                });
            }
        }
        Ok(out)
    }

    /// `note.get`: meeting, transcript (post if any, else live), summary and notes. `NotFound` if missing.
    pub fn detail(&self, id: &str) -> Result<MeetingDetail> {
        let meeting = self.need(id)?;
        Ok(MeetingDetail {
            meeting,
            segments: self.get_segments(id)?,
            summary: self.get_summary(id)?,
            notes: self.get_notes(id)?,
        })
    }
}
