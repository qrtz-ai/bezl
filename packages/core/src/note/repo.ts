import type { SqlDb } from '../db/migrate';
import type { Meeting, Segment } from './types';
import { MeetingSummary } from './summary';
import type { MeetingNote } from './notes';
import SEARCH_SEGMENT_HITS_SQL from './sql/search_segment_hits.sql?raw';

export interface MeetingRow extends Meeting {
  durationMs?: number;
}

export interface SearchHit {
  meetingId: string;
  title: string;
  startedAt: number;
  snippet: string;
  startMs?: number;
}

type Row = Record<string, unknown>;
const toMeeting = (r: Row): MeetingRow => ({
  id: String(r.id),
  title: String(r.title),
  platform: r.platform as Meeting['platform'],
  startedAt: Number(r.started_at),
  ...(r.ended_at == null ? {} : { endedAt: Number(r.ended_at) }),
  status: r.status as Meeting['status'],
  participants: JSON.parse(String(r.participants ?? '[]')) as string[],
  ...(r.audio_path == null ? {} : { audioPath: String(r.audio_path) }),
  ...(r.duration_ms == null ? {} : { durationMs: Number(r.duration_ms) }),
  ...(r.template == null ? {} : { template: String(r.template) }),
});
const toSegment = (r: Row): Segment => ({
  id: String(r.id),
  startMs: Number(r.start_ms),
  endMs: Number(r.end_ms),
  text: String(r.text),
  source: r.source as Segment['source'],
  ...(r.speaker == null ? {} : { speaker: String(r.speaker) }),
  ...(r.speaker_name == null ? {} : { speakerName: String(r.speaker_name) }),
  ...(r.confidence == null ? {} : { confidence: Number(r.confidence) }),
});

/**
 * Turns free text into a safe FTS5 query: each word becomes a quoted prefix term (AND-ed).
 * Strips FTS syntax so user input can never produce a query error or injection.
 */
export function toFtsQuery(input: string): string | null {
  const words = input
    .toLowerCase()
    .normalize('NFKC')
    .split(/[^\p{L}\p{N}']+/u)
    .map((w) => w.replace(/'/g, ''))
    .filter(Boolean)
    .slice(0, 8);
  if (!words.length) return null;
  return words.map((w) => `"${w}"*`).join(' ');
}

let txSeq = 0;

export class NoteRepo {
  constructor(private readonly db: SqlDb) {}

  /**
   * Atomic block. A SAVEPOINT, so blocks nest: outside a transaction it behaves like BEGIN/COMMIT, inside one it
   * is a sub-transaction (backup import wraps each meeting around replaceSegments/saveSummary, N11).
   */
  tx<T>(fn: () => T): T {
    const sp = `qrtz_tx_${++txSeq}`;
    this.db.exec(`SAVEPOINT ${sp}`);
    try {
      const r = fn();
      this.db.exec(`RELEASE ${sp}`);
      return r;
    } catch (e) {
      // SQLite may already have rolled the whole transaction back (SQLITE_FULL/IOERR/NOMEM), taking the savepoint
      // with it; then ROLLBACK TO fails with "no such savepoint" and must not mask the original error.
      try {
        this.db.exec(`ROLLBACK TO ${sp}`);
        this.db.exec(`RELEASE ${sp}`);
      } catch {
        /* already rolled back */
      }
      throw e;
    }
  }

  upsertMeeting(m: MeetingRow): void {
    this.db
      .prepare(
        `INSERT INTO meetings (id,title,platform,started_at,ended_at,status,participants,audio_path,duration_ms)
         VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, platform=excluded.platform, ended_at=excluded.ended_at,
           status=excluded.status, participants=excluded.participants, audio_path=excluded.audio_path, duration_ms=excluded.duration_ms`,
      )
      .run(
        m.id,
        m.title,
        m.platform,
        m.startedAt,
        m.endedAt ?? null,
        m.status,
        JSON.stringify(m.participants ?? []),
        m.audioPath ?? null,
        m.durationMs ?? null,
      );
  }

  setTemplate(id: string, template: string): void {
    this.db
      .prepare('UPDATE meetings SET template=? WHERE id=?')
      .run(template, id);
  }

  getNotes(meetingId: string): MeetingNote[] {
    return (
      this.db
        .prepare(
          'SELECT * FROM notes WHERE meeting_id=? ORDER BY at_ms, created_at',
        )
        .all(meetingId) as Row[]
    ).map((r) => ({
      id: String(r.id),
      atMs: Number(r.at_ms),
      text: String(r.text),
      createdAt: Number(r.created_at),
    }));
  }

  addNote(meetingId: string, n: MeetingNote): void {
    this.db
      .prepare(
        'INSERT INTO notes (id,meeting_id,at_ms,text,created_at) VALUES (?,?,?,?,?)',
      )
      .run(n.id, meetingId, Math.round(n.atMs), n.text, n.createdAt);
  }

  updateNote(id: string, text: string): boolean {
    const r = this.db
      .prepare('UPDATE notes SET text=? WHERE id=?')
      .run(text, id) as { changes: number | bigint };
    return Number(r.changes) > 0;
  }

  deleteNote(id: string): boolean {
    const r = this.db.prepare('DELETE FROM notes WHERE id=?').run(id) as {
      changes: number | bigint;
    };
    return Number(r.changes) > 0;
  }

  setStatus(id: string, status: Meeting['status']): void {
    this.db.prepare('UPDATE meetings SET status=? WHERE id=?').run(status, id);
  }

  getMeeting(id: string): MeetingRow | undefined {
    const r = this.db.prepare('SELECT * FROM meetings WHERE id=?').get(id) as
      | Row
      | undefined;
    return r ? toMeeting(r) : undefined;
  }

  listMeetings(limit = 100, offset = 0): MeetingRow[] {
    return (
      this.db
        .prepare(
          'SELECT * FROM meetings ORDER BY started_at DESC LIMIT ? OFFSET ?',
        )
        .all(limit, offset) as Row[]
    ).map(toMeeting);
  }

  /** Transcript lines matching any query term (FTS, best first), for chat retrieval across meetings. */
  searchSegments(input: string, limit = 60) {
    const words = (
      input
        .toLowerCase()
        .normalize('NFKC')
        .match(/[\p{L}\p{N}]{2,}/gu) ?? []
    ).slice(0, 12);
    if (!words.length) return [];
    const q = words.map((w) => `"${w}"*`).join(' OR ');
    return (
      this.db
        .prepare(
          `SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
                  COALESCE(s.speaker_name, s.speaker) AS speaker, s.text AS text
           FROM segments_fts JOIN segments s ON s.rowid = segments_fts.rowid JOIN meetings m ON m.id = s.meeting_id
           WHERE segments_fts MATCH ? ORDER BY bm25(segments_fts) LIMIT ?`,
        )
        .all(q, limit) as Row[]
    ).map((r) => ({
      meetingId: String(r.meetingId),
      title: String(r.title),
      startedAt: Number(r.startedAt),
      startMs: Number(r.startMs),
      ...(r.speaker == null ? {} : { speaker: String(r.speaker) }),
      text: String(r.text),
    }));
  }

  /** Segments with no vector for `model` yet (optionally one meeting), oldest meetings last. */
  missingVectors(model: string, limit: number, meetingId?: string) {
    return (
      this.db
        .prepare(
          `SELECT s.rowid AS rowid, s.meeting_id AS meetingId, s.text AS text FROM segments s
           LEFT JOIN segment_vectors v ON v.segment_rowid = s.rowid AND v.model = ?
           WHERE v.segment_rowid IS NULL ${meetingId ? 'AND s.meeting_id = ?' : ''}
           ORDER BY s.rowid DESC LIMIT ?`,
        )
        .all(
          ...(meetingId ? [model, meetingId, limit] : [model, limit]),
        ) as Row[]
    ).map((r) => ({
      rowid: Number(r.rowid),
      meetingId: String(r.meetingId),
      text: String(r.text),
    }));
  }

  saveVectors(
    model: string,
    rows: { rowid: number; meetingId: string; vec: Uint8Array }[],
  ) {
    this.tx(() => {
      const st = this.db.prepare(
        'INSERT OR REPLACE INTO segment_vectors (segment_rowid, meeting_id, model, vec) VALUES (?,?,?,?)',
      );
      for (const r of rows) st.run(r.rowid, r.meetingId, model, r.vec);
    });
  }

  /** Indexed transcript lines with their vectors (one meeting, or all). */
  vectorSources(model: string, meetingId?: string) {
    return (
      this.db
        .prepare(
          `SELECT s.meeting_id AS meetingId, m.title AS title, m.started_at AS startedAt, s.start_ms AS startMs,
                  COALESCE(s.speaker_name, s.speaker) AS speaker, s.text AS text, v.vec AS vec
           FROM segment_vectors v JOIN segments s ON s.rowid = v.segment_rowid JOIN meetings m ON m.id = s.meeting_id
           WHERE v.model = ? ${meetingId ? 'AND v.meeting_id = ?' : ''}`,
        )
        .all(...(meetingId ? [model, meetingId] : [model])) as Row[]
    ).map((r) => ({
      meetingId: String(r.meetingId),
      title: String(r.title),
      startedAt: Number(r.startedAt),
      startMs: Number(r.startMs),
      ...(r.speaker == null ? {} : { speaker: String(r.speaker) }),
      text: String(r.text),
      vec: r.vec as Uint8Array,
    }));
  }

  /** Rename a speaker in one meeting (merge = rename onto an existing name). Returns lines changed. */
  renameSpeaker(meetingId: string, from: string, to: string): number {
    return this.tx(() => {
      const r = this.db
        .prepare(
          'UPDATE segments SET speaker_name=? WHERE meeting_id=? AND COALESCE(speaker_name, speaker)=?',
        )
        .run(to, meetingId, from);
      const m = this.db
        .prepare('SELECT participants FROM meetings WHERE id=?')
        .get(meetingId) as Row | undefined;
      if (m) {
        const ps = (JSON.parse(String(m.participants ?? '[]')) as string[]).map(
          (p) => (p === from ? to : p),
        );
        this.db
          .prepare('UPDATE meetings SET participants=? WHERE id=?')
          .run(JSON.stringify([...new Set(ps)]), meetingId);
      }
      return Number((r as { changes: number | bigint }).changes);
    });
  }

  /** Meetings in the given states regardless of age (crash recovery must not miss old ones). */
  listByStatus(statuses: string[]): MeetingRow[] {
    if (!statuses.length) return [];
    return (
      this.db
        .prepare(
          `SELECT * FROM meetings WHERE status IN (${statuses.map(() => '?').join(',')}) ORDER BY started_at`,
        )
        .all(...statuses) as Row[]
    ).map(toMeeting);
  }

  /**
   * Every file on disk that belongs to a meeting: its recording plus the WAV parts its post-processing record
   * still lists (a delete before finalize stitched them). Deleting the meeting must remove these too (N15).
   */
  meetingFiles(id: string): string[] {
    const files = new Set<string>();
    const m = this.db
      .prepare('SELECT audio_path AS a FROM meetings WHERE id=?')
      .get(id) as { a: string | null } | undefined;
    if (m?.a) files.add(m.a);
    const p = this.db
      .prepare('SELECT json FROM pipeline_state WHERE meeting_id=?')
      .get(id) as { json: string } | undefined;
    if (p) {
      try {
        const parts = (JSON.parse(p.json) as { ctx?: { audioParts?: unknown } })
          .ctx?.audioParts;
        if (Array.isArray(parts))
          for (const f of parts) if (typeof f === 'string' && f) files.add(f);
      } catch {
        // An unreadable record lists nothing; the recording itself is still returned.
      }
    }
    return [...files];
  }

  deleteMeeting(id: string): void {
    this.tx(() => {
      const r = this.db
        .prepare('SELECT rowid AS r FROM meetings WHERE id=?')
        .get(id) as { r: number } | undefined;
      if (r) this.db.prepare('DELETE FROM meetings_fts WHERE rowid=?').run(r.r);
      this.db.prepare('DELETE FROM segments WHERE meeting_id=?').run(id);
      this.db.prepare('DELETE FROM meetings WHERE id=?').run(id);
    });
  }

  /** Replaces all segments of one source for a meeting (idempotent re-processing). */
  replaceSegments(
    meetingId: string,
    source: Segment['source'],
    segs: Segment[],
  ): void {
    this.tx(() => {
      this.db
        .prepare('DELETE FROM segments WHERE meeting_id=? AND source=?')
        .run(meetingId, source);
      const ins = this.db.prepare(
        'INSERT INTO segments (id,meeting_id,source,start_ms,end_ms,speaker,speaker_name,text,confidence) VALUES (?,?,?,?,?,?,?,?,?)',
      );
      for (const s of segs)
        ins.run(
          s.id,
          meetingId,
          source,
          Math.round(s.startMs),
          Math.round(s.endMs),
          s.speaker ?? null,
          s.speakerName ?? null,
          s.text,
          s.confidence ?? null,
        );
    });
  }

  appendSegment(meetingId: string, s: Segment): void {
    this.db
      .prepare(
        `INSERT INTO segments (id,meeting_id,source,start_ms,end_ms,speaker,speaker_name,text,confidence) VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(meeting_id, source, id) DO UPDATE SET text=excluded.text, end_ms=excluded.end_ms, speaker_name=excluded.speaker_name`,
      )
      .run(
        s.id,
        meetingId,
        s.source,
        Math.round(s.startMs),
        Math.round(s.endMs),
        s.speaker ?? null,
        s.speakerName ?? null,
        s.text,
        s.confidence ?? null,
      );
  }

  /** Post-processed transcript if present, else live. */
  getSegments(meetingId: string): Segment[] {
    const post = this.db
      .prepare(
        "SELECT * FROM segments WHERE meeting_id=? AND source='post' ORDER BY start_ms",
      )
      .all(meetingId) as Row[];
    const rows = post.length
      ? post
      : (this.db
          .prepare(
            "SELECT * FROM segments WHERE meeting_id=? AND source='live' ORDER BY start_ms",
          )
          .all(meetingId) as Row[]);
    return rows.map(toSegment);
  }

  saveSummary(meetingId: string, s: MeetingSummary, now = Date.now()): void {
    this.tx(() => {
      this.db
        .prepare(
          'INSERT INTO summaries (meeting_id,provider,json,created_at) VALUES (?,?,?,?) ON CONFLICT(meeting_id) DO UPDATE SET provider=excluded.provider, json=excluded.json, created_at=excluded.created_at',
        )
        .run(meetingId, s.provider, JSON.stringify(s), now);
      const m = this.getMeeting(meetingId);
      const rowid = this.meetingRowid(meetingId);
      this.db.prepare('DELETE FROM meetings_fts WHERE rowid=?').run(rowid);
      this.db
        .prepare('INSERT INTO meetings_fts(rowid,title,summary) VALUES (?,?,?)')
        .run(
          rowid,
          m?.title ?? '',
          [s.summary, ...s.actionItems.map((a) => a.text)].join('\n'),
        );
    });
  }

  private meetingRowid(id: string): number {
    return Number(
      (
        this.db
          .prepare('SELECT rowid AS r FROM meetings WHERE id=?')
          .get(id) as { r: number }
      ).r,
    );
  }

  getSummary(meetingId: string): MeetingSummary | undefined {
    const r = this.db
      .prepare('SELECT json FROM summaries WHERE meeting_id=?')
      .get(meetingId) as { json: string } | undefined;
    return r ? MeetingSummary.parse(JSON.parse(r.json)) : undefined;
  }

  /** Full-text search across transcripts, titles and summaries. One best hit per meeting. */
  search(input: string, limit = 50): SearchHit[] {
    const q = toFtsQuery(input);
    if (!q) return [];
    // Best line per meeting in SQL, then the `limit` best meetings (sql/search_segment_hits.sql). It used to take the
    // 500 best *lines* and dedupe after: one long meeting full of a common word filled all 500 and hid every other
    // meeting that mentions it (cycle 3, N10).
    const segHits = this.db
      .prepare(SEARCH_SEGMENT_HITS_SQL)
      .all(q, limit) as (SearchHit & { rank: number })[];
    const metaHits = this.db
      .prepare(
        `SELECT m.id AS meetingId, m.title AS title, m.started_at AS startedAt,
                snippet(meetings_fts, -1, '[', ']', '…', 12) AS snippet, bm25(meetings_fts) AS rank
         FROM meetings_fts JOIN meetings m ON m.rowid = meetings_fts.rowid WHERE meetings_fts MATCH ? ORDER BY rank LIMIT ?`,
      )
      .all(q, limit) as (SearchHit & { rank: number })[];
    // Also match titles of meetings without a summary yet.
    const titleHits = (
      this.db
        .prepare('SELECT id, title, started_at FROM meetings')
        .all() as Row[]
    )
      .filter(
        (r) =>
          input.trim() &&
          String(r.title).toLowerCase().includes(input.trim().toLowerCase()),
      )
      .map((r) => ({
        meetingId: String(r.id),
        title: String(r.title),
        startedAt: Number(r.started_at),
        snippet: String(r.title),
        rank: -1e9,
      }));
    const best = new Map<string, SearchHit & { rank: number }>();
    for (const h of [...titleHits, ...metaHits, ...segHits]) {
      const cur = best.get(h.meetingId);
      if (!cur || h.rank < cur.rank) best.set(h.meetingId, h);
    }
    return [...best.values()]
      .sort((a, b) => a.rank - b.rank)
      .slice(0, limit)
      .map(({ rank: _r, ...h }) => ({
        ...h,
        startMs: h.startMs == null ? undefined : Number(h.startMs),
      }));
  }
}
