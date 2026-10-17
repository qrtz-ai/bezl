
CREATE TABLE notes (
  id TEXT PRIMARY KEY,
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  at_ms INTEGER NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX notes_meeting ON notes(meeting_id, at_ms);
ALTER TABLE meetings ADD COLUMN template TEXT NOT NULL DEFAULT 'general';
