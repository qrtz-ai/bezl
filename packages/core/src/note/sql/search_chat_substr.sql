-- Chat retrieval across meetings for a question with a Han / kana / Thai / Lao / Khmer / Myanmar word (N49, ADR 0047).
-- unicode61 keeps a run of those scripts as one token, so the question's run is cut into character bigrams
-- (chatPlan / chat_plan) and each bigram is matched as a substring; the other words still go through the index.
-- ?1: the OR-ed MATCH for the other words, or NULL when there are none.
-- ?2: JSON array of bigrams (lowercase, NFKC), matched in lower(text) (ASCII lower, as SQLite). The array is read
--     once into `t` (not per line), and only a bigram with an ASCII letter pays for lower().
-- ?3: limit. A line scores one per bigram it contains, plus one when it matches ?1; best first, then newest meeting,
--     then earliest line. Shared byte-for-byte by packages/core/src/note/repo.ts and crates/qrtz-storage/src/notes.rs.
WITH t(term, ascii) AS MATERIALIZED (SELECT value, value GLOB '*[a-z]*' FROM json_each(?2))
SELECT meetingId, title, startedAt, startMs, speaker, text FROM (
  SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
         COALESCE(s.speaker_name, s.speaker) AS speaker, s.text AS text, s.rowid AS rid,
         (SELECT count(*) FROM t WHERE instr(CASE WHEN t.ascii THEN lower(s.text) ELSE s.text END, t.term) > 0)
           + CASE WHEN ?1 IS NULL THEN 0
                  ELSE s.rowid IN (SELECT rowid FROM segments_fts WHERE segments_fts MATCH ?1) END AS hits
  FROM segments s JOIN meetings m ON m.id = s.meeting_id
) WHERE hits > 0 ORDER BY hits DESC, startedAt DESC, meetingId, startMs, rid LIMIT ?3
