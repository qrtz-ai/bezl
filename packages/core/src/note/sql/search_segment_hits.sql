-- Transcript hits for full-text search: the best-ranked line of each meeting, best meetings first, at most ?2.
-- ?1 is a toFtsQuery / to_fts_query MATCH expression. snippet() cannot run under a window function, so ranks are
-- picked in a materialized CTE and the snippet is taken in a plain MATCH query over just the winners.
-- Shared byte-for-byte by packages/core/src/note/repo.ts and crates/qrtz-storage/src/notes.rs (cycle 3, N10).
WITH hits AS MATERIALIZED (
  SELECT segments_fts.rowid AS r, bm25(segments_fts) AS rank FROM segments_fts WHERE segments_fts MATCH ?1
), best AS (
  SELECT r, rank FROM (
    SELECT h.r AS r, h.rank AS rank, ROW_NUMBER() OVER (PARTITION BY s.meeting_id ORDER BY h.rank, h.r) AS rn
    FROM hits h JOIN segments s ON s.rowid = h.r
  ) WHERE rn = 1 ORDER BY rank, r LIMIT ?2
)
SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
       snippet(segments_fts, 0, '[', ']', '…', 12) AS snippet, best.rank AS rank
FROM segments_fts JOIN best ON best.r = segments_fts.rowid
  JOIN segments s ON s.rowid = segments_fts.rowid JOIN meetings m ON m.id = s.meeting_id
WHERE segments_fts MATCH ?1 ORDER BY best.rank, best.r
