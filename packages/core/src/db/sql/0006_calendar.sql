-- ADR 0217: nano's Apple Calendar cache (EventKit, read-only, on this Mac). Only what nano shows and matches on:
-- no notes, locations, attendee names or addresses. The window is one day back to 14 days ahead, replaced whole on
-- every read; it is re-readable, so backups may skip it.
CREATE TABLE calendar_calendars (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL
);
CREATE TABLE calendar_events (
  id TEXT PRIMARY KEY,
  calendar_id TEXT NOT NULL,
  title TEXT NOT NULL,
  start_at INTEGER NOT NULL,
  end_at INTEGER NOT NULL,
  all_day INTEGER NOT NULL DEFAULT 0,
  attendee_count INTEGER NOT NULL DEFAULT 0,
  meeting_url TEXT
);
CREATE INDEX calendar_events_start ON calendar_events(start_at);
-- The event a recording was titled from. Not a foreign key: the cache is replaced on every read and the note keeps
-- its link (and its title) after the event leaves the window.
ALTER TABLE meetings ADD COLUMN calendar_event_id TEXT;
