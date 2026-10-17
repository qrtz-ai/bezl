
CREATE TABLE segment_vectors (
  segment_rowid INTEGER PRIMARY KEY,
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  vec BLOB NOT NULL
);
CREATE INDEX segment_vectors_meeting ON segment_vectors(meeting_id);
CREATE TRIGGER segments_vec_ad AFTER DELETE ON segments BEGIN
  DELETE FROM segment_vectors WHERE segment_rowid = old.rowid;
END;
