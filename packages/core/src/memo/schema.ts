import type { Migration } from '../db/migrate';

/** Memo tables. Appended to the app migration list (never edit once shipped). */
export const memoMigrations: Migration[] = [
  {
    version: 2,
    name: 'memo',
    up: `
CREATE TABLE meetings (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  platform TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  status TEXT NOT NULL,
  participants TEXT NOT NULL DEFAULT '[]',
  audio_path TEXT,
  duration_ms INTEGER
);
CREATE INDEX meetings_started ON meetings(started_at DESC);
CREATE TABLE segments (
  rowid INTEGER PRIMARY KEY,
  id TEXT NOT NULL,
  meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  start_ms INTEGER NOT NULL,
  end_ms INTEGER NOT NULL,
  speaker TEXT,
  speaker_name TEXT,
  text TEXT NOT NULL,
  confidence REAL,
  UNIQUE(meeting_id, source, id)
);
CREATE INDEX segments_meeting ON segments(meeting_id, source, start_ms);
CREATE VIRTUAL TABLE segments_fts USING fts5(text, speaker_name, content='segments', content_rowid='rowid', tokenize='porter unicode61');
CREATE TRIGGER segments_ai AFTER INSERT ON segments BEGIN
  INSERT INTO segments_fts(rowid, text, speaker_name) VALUES (new.rowid, new.text, new.speaker_name);
END;
CREATE TRIGGER segments_ad AFTER DELETE ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text, speaker_name) VALUES ('delete', old.rowid, old.text, old.speaker_name);
END;
CREATE TRIGGER segments_au AFTER UPDATE ON segments BEGIN
  INSERT INTO segments_fts(segments_fts, rowid, text, speaker_name) VALUES ('delete', old.rowid, old.text, old.speaker_name);
  INSERT INTO segments_fts(rowid, text, speaker_name) VALUES (new.rowid, new.text, new.speaker_name);
END;
CREATE TABLE summaries (
  meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE VIRTUAL TABLE meetings_fts USING fts5(title, summary, tokenize='porter unicode61');
`,
  },
];
