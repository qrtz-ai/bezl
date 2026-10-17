/**
 * Hardening cycle 3 stress scenarios (keep running on every `npm test`). See docs/hardening/LOG.md.
 * LLM transport after a 200 (N8), OpenRouter error bodies, malformed model replies; search across one huge
 * meeting (N10); nested atomic blocks (N11); disk full / busy classified retryable (N13).
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import { AppError } from '../errors';
import { migrate, migrations } from '../db/migrate';
import { createProvider, summarizeMeeting, type FetchLike } from './llm';
import { NoteRepo } from './repo';

const segs = [
  {
    id: 's0',
    startMs: 0,
    endMs: 900,
    text: 'We ship on Friday.',
    source: 'live' as const,
  },
];
const orWith = (fetch: FetchLike) =>
  createProvider('openrouter', { fetch, apiKey: 'sk-test' })!;
/** A 200 whose body read fails with `e` (what undici does when the socket resets mid-body). */
const okThenBody =
  (body: () => Promise<unknown>): FetchLike =>
  async () => ({
    ok: true,
    status: 200,
    json: body,
    text: async () => '',
  });
const strict = (fetch: FetchLike, signal?: AbortSignal) =>
  summarizeMeeting('T', segs, orWith(fetch), undefined, {
    strict: true,
    ...(signal ? { signal } : {}),
  }).then(
    () => {
      throw new Error('expected the strict summary to throw');
    },
    (e: unknown) => e as AppError,
  );

describe('hardening 3: LLM failures after the status line (N8)', () => {
  it('a connection reset while reading a 200 body is a retryable E_UNAVAILABLE, not E_INTERNAL', async () => {
    const err = await strict(
      okThenBody(() => Promise.reject(new TypeError('terminated'))),
    );
    expect(err).toBeInstanceOf(AppError);
    expect(err.code).toBe('E_UNAVAILABLE');
    expect(err.retryable).toBe(true);
  });

  it('a 200 with an HTML body (proxy, captive portal) is a retryable provider error', async () => {
    const err = await strict(
      okThenBody(() =>
        Promise.reject(new SyntaxError('Unexpected token < in JSON')),
      ),
    );
    expect(err).toBeInstanceOf(AppError);
    expect(err.code).toBe('E_PROVIDER');
    expect(err.retryable).toBe(true);
  });

  it('the deadline firing during the body read is E_TIMEOUT', async () => {
    const ac = new AbortController();
    const err = await strict(
      okThenBody(() => {
        ac.abort();
        return Promise.reject(
          Object.assign(new Error('aborted'), { name: 'AbortError' }),
        );
      }),
      ac.signal,
    );
    expect(err.code).toBe('E_TIMEOUT');
    expect(err.retryable).toBe(true);
  });

  it('OpenRouter upstream errors inside a 200 surface their message; 5xx retryable, 4xx terminal', async () => {
    const body = (error: unknown) => okThenBody(async () => ({ error }));
    const e502 = await strict(
      body({ code: 502, message: 'upstream overloaded' }),
    );
    expect(e502.code).toBe('E_PROVIDER');
    expect(e502.message).toContain('upstream overloaded');
    expect(e502.retryable).toBe(true);
    const e400 = await strict(body({ code: 400, message: 'context too long' }));
    expect(e400.retryable).toBe(false);
  });

  it('a malformed model reply (prose, wrong shape) is retryable so the pipeline re-asks before falling back', async () => {
    for (const content of [
      'Sorry, I cannot help with that.',
      JSON.stringify({ summary: 5, actionItems: 'none' }),
    ]) {
      const err = await strict(
        okThenBody(async () => ({ choices: [{ message: { content } }] })),
      );
      expect(err).toBeInstanceOf(AppError);
      expect(err.code).toBe('E_PROVIDER');
      expect(err.retryable).toBe(true);
    }
  });

  it('non-strict callers still get the extractive fallback with the LLM error attached', async () => {
    const s = await summarizeMeeting(
      'T',
      segs,
      orWith(okThenBody(() => Promise.reject(new TypeError('terminated')))),
    );
    expect(s.provider).toBe('extractive');
    expect(s.llmError).toContain('terminated');
  });
});

function repoWith(): { db: DatabaseSync; repo: NoteRepo } {
  const db = new DatabaseSync(':memory:');
  migrate(db, migrations);
  return { db, repo: new NoteRepo(db) };
}
const meetingRow = (id: string, title: string, startedAt: number) => ({
  id,
  title,
  platform: 'zoom' as const,
  startedAt,
  status: 'done' as const,
  participants: [],
});

describe('hardening 3: search is not starved by one long meeting (N10)', () => {
  it('a 2h all-hands that says "budget" 600 times does not hide the five other meetings that mention it', () => {
    const { repo } = repoWith();
    repo.upsertMeeting(meetingRow('allhands', 'All hands', 1));
    repo.replaceSegments(
      'allhands',
      'live',
      Array.from({ length: 600 }, (_, i) => ({
        id: `a${i}`,
        startMs: i * 10_000,
        endMs: i * 10_000 + 9_000,
        text: `budget budget ${i}`,
        source: 'live' as const,
      })),
    );
    for (let k = 0; k < 5; k++) {
      repo.upsertMeeting(meetingRow(`m${k}`, `Sync ${k}`, 10 + k));
      repo.replaceSegments(`m${k}`, 'live', [
        {
          id: `s${k}`,
          startMs: 0,
          endMs: 5_000,
          text: `We went over the quarterly plan and the marketing budget, among a few other topics ${k}.`,
          source: 'live',
        },
      ]);
    }
    const hits = repo.search('budget', 50);
    expect(hits.map((h) => h.meetingId).sort()).toEqual([
      'allhands',
      'm0',
      'm1',
      'm2',
      'm3',
      'm4',
    ]);
    // One hit per meeting, each with a snippet and the timestamp of its best line.
    for (const h of hits) {
      expect(h.snippet).toMatch(/\[budget\]/i);
      expect(typeof h.startMs).toBe('number');
    }
    // `limit` is honoured in SQL, best meetings first (the all-hands lines are the densest match).
    expect(repo.search('budget', 2)).toHaveLength(2);
    expect(repo.search('budget', 2)[0]!.meetingId).toBe('allhands');
  });
});

describe('hardening 3: repo.tx nests (N11)', () => {
  it('an inner failure rolls back only the inner block; an outer failure rolls back both', () => {
    const { repo } = repoWith();
    repo.tx(() => {
      repo.upsertMeeting(meetingRow('a', 'A', 1));
      expect(() =>
        repo.tx(() => {
          repo.upsertMeeting(meetingRow('b', 'B', 2));
          throw new Error('inner');
        }),
      ).toThrow('inner');
    });
    expect(repo.getMeeting('a')).toBeDefined();
    expect(repo.getMeeting('b')).toBeUndefined();
    expect(() =>
      repo.tx(() => {
        repo.upsertMeeting(meetingRow('c', 'C', 3));
        repo.replaceSegments('c', 'live', [
          { id: 'x', startMs: 0, endMs: 1, text: 'hello', source: 'live' },
        ]);
        throw new Error('outer');
      }),
    ).toThrow('outer');
    expect(repo.getMeeting('c')).toBeUndefined();
    expect(repo.getSegments('c')).toEqual([]);
  });

  it('disk full inside a block surfaces the disk-full error, not "no such savepoint"', () => {
    const { db, repo } = repoWith();
    repo.upsertMeeting(meetingRow('m', 'M', 1));
    const pages = (
      db.prepare('PRAGMA page_count').get() as { page_count: number }
    ).page_count;
    db.exec(`PRAGMA max_page_count = ${pages + 2}`);
    const big = Array.from({ length: 2_000 }, (_, i) => ({
      id: `s${i}`,
      startMs: i,
      endMs: i + 1,
      text: `segment ${i} `.repeat(20),
      source: 'live' as const,
    }));
    expect(() => repo.tx(() => repo.replaceSegments('m', 'live', big))).toThrow(
      /full/,
    );
    expect(repo.getSegments('m')).toEqual([]);
    db.exec('PRAGMA max_page_count = 1073741823');
    repo.replaceSegments('m', 'live', big);
    expect(repo.getSegments('m')).toHaveLength(2_000);
  });
});

describe('hardening 3: storage errors a later attempt can outlive are retryable (N13)', () => {
  const thrown = (fn: () => unknown): unknown => {
    try {
      fn();
    } catch (e) {
      return e;
    }
    throw new Error('expected a throw');
  };

  it('disk full (SQLITE_FULL) → E_UNAVAILABLE, retryable', () => {
    const { db, repo } = repoWith();
    repo.upsertMeeting(meetingRow('m', 'M', 1));
    const pages = (
      db.prepare('PRAGMA page_count').get() as { page_count: number }
    ).page_count;
    db.exec(`PRAGMA max_page_count = ${pages}`);
    const e = AppError.from(
      thrown(() =>
        repo.replaceSegments(
          'm',
          'live',
          Array.from({ length: 500 }, (_, i) => ({
            id: `s${i}`,
            startMs: i,
            endMs: i + 1,
            text: 'words '.repeat(40),
            source: 'live' as const,
          })),
        ),
      ),
    );
    expect(e.message).toMatch(/full/);
    expect(e.code).toBe('E_UNAVAILABLE');
    expect(e.retryable).toBe(true);
  });

  it('a locked database (SQLITE_BUSY from a second connection) → E_UNAVAILABLE, retryable', () => {
    const file = path.join(
      fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-busy-')),
      'x.db',
    );
    const a = new DatabaseSync(file);
    migrate(a, migrations);
    const b = new DatabaseSync(file);
    b.exec('PRAGMA busy_timeout = 0');
    a.exec('BEGIN IMMEDIATE');
    const e = AppError.from(
      thrown(() => new NoteRepo(b).upsertMeeting(meetingRow('m', 'M', 1))),
    );
    a.exec('ROLLBACK');
    expect(e.code).toBe('E_UNAVAILABLE');
    expect(e.retryable).toBe(true);
  });

  it('a constraint or SQL error stays E_INTERNAL, not retryable', () => {
    const { db } = repoWith();
    const e = AppError.from(
      thrown(() => db.exec('SELECT * FROM no_such_table')),
    );
    expect(e.code).toBe('E_INTERNAL');
    expect(e.retryable).toBe(false);
  });
});
