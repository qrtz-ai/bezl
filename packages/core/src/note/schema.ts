import type { Migration } from '../db/migrate';
// Shared with the Rust core (crates/qrtz-storage, via include_str!). Never edit a shipped .sql file.
import memo from '../db/sql/0002_memo.sql?raw';
import segmentVectors from '../db/sql/0003_segment_vectors.sql?raw';
import notesTemplates from '../db/sql/0004_notes_templates.sql?raw';
import recordingLifecycle from '../db/sql/0005_recording_lifecycle.sql?raw';
import calendar from '../db/sql/0006_calendar.sql?raw';

/** Note tables. Appended to the app migration list (never edit once shipped). */
export const noteMigrations: Migration[] = [
  // Meetings, segments (+ external-content FTS5 and its sync triggers), summaries, meetings_fts.
  { version: 2, name: 'memo', up: memo }, // shipped migration name (pre-rename); never edit
  // On-device embeddings for semantic chat retrieval.
  { version: 3, name: 'segment_vectors', up: segmentVectors },
  // Live notes (Granola-style) and per-meeting summary templates.
  { version: 4, name: 'notes_templates', up: notesTemplates },
  // Durable recording lifecycle: the live session snapshot and per-meeting post-processing pipelines.
  { version: 5, name: 'recording_lifecycle', up: recordingLifecycle },
  // ADR 0217: nano's Apple Calendar cache and meetings.calendar_event_id.
  { version: 6, name: 'calendar', up: calendar },
];
