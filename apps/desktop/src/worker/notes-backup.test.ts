import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  extractiveSummary,
  buildSummaryPrompt,
  type FetchLike,
} from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};
const llm = { provider: 'none' as const };

function meeting(
  h: ReturnType<typeof createWorkerHandlers>,
  id: string,
  template?: string,
) {
  h['memo.create']({
    meeting: {
      id,
      title: 'Acme pricing call',
      platform: 'zoom',
      startedAt: 1_760_000_000_000,
      status: 'recording',
      participants: ['Ada', 'Bob'],
      ...(template ? { template } : {}),
    },
  });
  const lines = [
    'Thanks for joining, we wanted to review the rollout plan today.',
    'Our budget for the pilot is fifty thousand dollars this quarter.',
    'The weather has been really nice lately around the office.',
  ];
  lines.forEach((text, i) =>
    h['memo.appendLive']({
      meetingId: id,
      segment: {
        id: `u${i}`,
        startMs: i * 5000,
        endMs: i * 5000 + 4000,
        text,
        source: 'live',
      },
    }),
  );
}

describe('notes + templates', () => {
  it('stores timestamped notes and merges them into the extractive summary', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    meeting(h, 'm1');
    h['memo.addNote']({
      meetingId: 'm1',
      text: 'Pilot budget 50k',
      atMs: 6200,
    });
    const todo = h['memo.addNote']({
      meetingId: 'm1',
      text: 'TODO send the security questionnaire',
      atMs: 9000,
    });
    const s = await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1,
      durationMs: 15000,
      active: [],
      llm,
    });
    expect(s.summary).toMatch(
      /^Your notes: Pilot budget 50k; TODO send the security questionnaire\./,
    );
    expect(s.actionItems.map((a) => a.text)).toContain(
      'send the security questionnaire',
    );
    const d = h['memo.get']({ id: 'm1' });
    expect(d.notes.map((n) => [n.atMs, n.text])).toEqual([
      [6200, 'Pilot budget 50k'],
      [9000, 'TODO send the security questionnaire'],
    ]);
    expect(h['memo.export']({ id: 'm1' }).markdown).toContain(
      '- [0:06] Pilot budget 50k',
    );
    expect(
      h['memo.updateNote']({ id: todo.id, text: 'Send questionnaire' }).ok,
    ).toBe(true);
    expect(h['memo.deleteNote']({ id: todo.id }).ok).toBe(true);
    expect(h['memo.listNotes']({ meetingId: 'm1' })).toHaveLength(1);
  });

  it('stores the template per meeting and re-runs with it', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    meeting(h, 'm2', 'standup');
    const first = await h['memo.finalize']({
      meetingId: 'm2',
      endedAt: 1,
      durationMs: 15000,
      active: [],
      llm,
    });
    expect(first.template).toBe('standup');
    const again = await h['memo.resummarize']({
      id: 'm2',
      template: 'sales',
      llm,
    });
    expect(again.template).toBe('sales');
    expect(h['memo.get']({ id: 'm2' }).meeting.template).toBe('sales');
  });

  it('puts notes and template guidance into the LLM prompt', () => {
    const msgs = buildSummaryPrompt('t', [], {
      template: 'interview',
      notes: [
        {
          id: 'n',
          atMs: 61_000,
          text: 'strong on systems design',
          createdAt: 0,
        },
      ],
    });
    expect(msgs[0]!.content).toMatch(/job interview/);
    expect(msgs[1]!.content).toContain(
      'My notes:\n[1:01] strong on systems design',
    );
    expect(extractiveSummary([], { notes: [] }).summary).toBe(
      'No speech was transcribed.',
    );
  });
});

describe('backup import', () => {
  it('snapshots, verifies and merges meetings without duplicating', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-imp-'));
    const a = createWorkerHandlers(openDatabase(path.join(dir, 'a.db')), {
      fetch: noFetch,
    });
    meeting(a, 'm1');
    a['memo.addNote']({ meetingId: 'm1', text: 'note', atMs: 1 });
    await a['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1,
      durationMs: 15000,
      active: [],
      llm,
      audioPath: '/old/m1.wav',
    });
    const snap = path.join(dir, 'snap.db');
    expect(a['data.snapshot']({ path: snap }).meetings).toBe(1);

    const b = createWorkerHandlers(openDatabase(path.join(dir, 'b.db')), {
      fetch: noFetch,
    });
    const r = b['data.importDb']({
      path: snap,
      audioMap: { 'm1.wav': '/new/m1.wav' },
    });
    expect(r).toEqual({ imported: 1, skipped: 0 });
    const d = b['memo.get']({ id: 'm1' });
    expect(d.segments).toHaveLength(3);
    expect(d.notes).toHaveLength(1);
    expect(d.summary?.provider).toBe('extractive');
    expect(d.meeting.audioPath).toBe('/new/m1.wav');
    expect(b['memo.list']({ query: 'budget' }).length).toBe(1);
    expect(b['data.importDb']({ path: snap, audioMap: {} })).toEqual({
      imported: 0,
      skipped: 1,
    });

    fs.writeFileSync(path.join(dir, 'junk.db'), 'not a database at all');
    expect(() =>
      b['data.importDb']({ path: path.join(dir, 'junk.db'), audioMap: {} }),
    ).toThrow();
  });
});
