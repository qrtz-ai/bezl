-- Title / summary hits for a search with a Han / kana / Thai / Lao / Khmer / Myanmar term (N48, ADR 0039).
-- Same parameters as search_segment_substr.sql, over meetings_fts (title, summary). The snippet is built in code.
-- Shared byte-for-byte by packages/core/src/note/repo.ts and crates/qrtz-storage/src/notes.rs.
SELECT m.id AS meetingId, m.title AS title, m.started_at AS startedAt, f.title AS ftsTitle, f.summary AS summary
FROM meetings_fts f JOIN meetings m ON m.rowid = f.rowid
WHERE (?1 IS NULL OR f.rowid IN (SELECT rowid FROM meetings_fts WHERE meetings_fts MATCH ?1))
  AND NOT EXISTS (
    SELECT 1 FROM json_each(?2) j WHERE instr(lower(f.title || char(10) || f.summary), j.value) = 0
  )
ORDER BY m.started_at DESC, m.id LIMIT ?3
