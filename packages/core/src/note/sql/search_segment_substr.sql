-- Transcript hits for a search with a Han / kana / Thai / Lao / Khmer / Myanmar term (N48, ADR 0039). unicode61 keeps
-- a run of those scripts as one token, so a word inside a run is found by substring instead of by the index.
-- ?1: the MATCH expression for the other terms (searchPlan().fts / search_plan().fts), or NULL when there are none.
-- ?2: JSON array of substring terms (lowercase, NFKC); every one must occur in lower(text) (ASCII lower, as SQLite).
-- ?3: limit. The earliest matching line of each meeting, newest meetings first. The snippet is built in code.
-- Shared byte-for-byte by packages/core/src/note/repo.ts and crates/qrtz-storage/src/notes.rs.
SELECT meetingId, title, startedAt, startMs, text FROM (
  SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs, s.text AS text,
         ROW_NUMBER() OVER (PARTITION BY s.meeting_id ORDER BY s.start_ms, s.rowid) AS rn
  FROM segments s JOIN meetings m ON m.id = s.meeting_id
  WHERE (?1 IS NULL OR s.rowid IN (SELECT rowid FROM segments_fts WHERE segments_fts MATCH ?1))
    AND NOT EXISTS (SELECT 1 FROM json_each(?2) j WHERE instr(lower(s.text), j.value) = 0)
) WHERE rn = 1 ORDER BY startedAt DESC, meetingId LIMIT ?3
