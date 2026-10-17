/**
 * Hardening cycle 3 (N11): a backup import that fails mid-meeting (disk full) leaves no half meeting behind, so
 * the re-import (which skips meeting ids already present) repairs it instead of skipping it forever.
 */
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import type { FetchLike } from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};

describe('hardening 3: backup import is atomic per meeting (N11)', () => {
  it('disk full mid-transcript → no meeting row; after space frees up the re-import brings the whole meeting', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-imp3-'));
    const a = createWorkerHandlers(openDatabase(path.join(dir, 'a.db')), {
      fetch: noFetch,
    });
    a['note.create']({
      meeting: {
        id: 'long',
        title: 'Quarterly planning',
        platform: 'zoom',
        startedAt: 1_760_000_000_000,
        status: 'recording',
        participants: [],
      },
    });
    for (let i = 0; i < 1_500; i++)
      a['note.appendLive']({
        meetingId: 'long',
        segment: {
          id: `s${i}`,
          startMs: i * 4_000,
          endMs: i * 4_000 + 3_500,
          text: `line ${i}: we talked through the roadmap and the hiring plan in some detail`,
          source: 'live',
        },
      });
    const snap = path.join(dir, 'snap.db');
    expect(a['data.snapshot']({ path: snap }).meetings).toBe(1);

    const target = openDatabase(':memory:');
    const b = createWorkerHandlers(target, { fetch: noFetch });
    const pages = (
      target.prepare('PRAGMA page_count').get() as { page_count: number }
    ).page_count;
    // Room for the meeting row, not for 1,500 transcript lines and their FTS index.
    target.exec(`PRAGMA max_page_count = ${pages + 4}`);
    expect(() => b['data.importDb']({ path: snap, audioMap: {} })).toThrow(
      /full/,
    );
    expect(b['note.list']({}).map((r) => r.meeting.id)).toEqual([]);

    target.exec('PRAGMA max_page_count = 1073741823');
    expect(b['data.importDb']({ path: snap, audioMap: {} })).toEqual({
      imported: 1,
      skipped: 0,
    });
    expect(b['note.get']({ id: 'long' }).segments).toHaveLength(1_500);
  });
});
