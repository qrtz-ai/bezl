import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { MeetingListItem } from '@qrtz/core';
import {
  STANDUP_FIXTURE,
  fixtureActiveSpeakers,
  fixtureAsrEvents,
  TranscriptAssembler,
  type FetchLike,
} from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};

function seed(h: ReturnType<typeof createWorkerHandlers>) {
  h['memo.create']({
    meeting: {
      id: 'm1',
      title: STANDUP_FIXTURE.meeting.title,
      platform: 'zoom',
      startedAt: 1_760_000_000_000,
      status: 'recording',
      participants: [...STANDUP_FIXTURE.participants],
    },
  });
  const a = new TranscriptAssembler();
  for (const e of fixtureAsrEvents()) {
    a.push(e);
    if (e.kind === 'final')
      h['memo.appendLive']({
        meetingId: 'm1',
        segment: a.segments().find((s) => s.id === e.utteranceId)!,
      });
  }
}

describe('worker handlers (DB + pipeline integration)', () => {
  it('records live, finalizes the live transcript with AX speaker fusion + extractive summary, lists, searches and exports', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    expect(h['db.health']()).toEqual({
      ok: true,
      schemaVersion: 3,
      fts5: true,
    });
    seed(h);
    expect(h['memo.get']({ id: 'm1' }).segments).toHaveLength(8);
    const summary = await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1_760_000_036_500,
      durationMs: 36_500,
      audioPath: '/tmp/m1.wav',
      active: fixtureActiveSpeakers(),
      llm: { provider: 'none' },
    });
    expect(summary.provider).toBe('extractive');
    expect(summary.actionItems.length).toBeGreaterThanOrEqual(3);
    const got = h['memo.get']({ id: 'm1' });
    expect(got.meeting).toMatchObject({
      status: 'done',
      durationMs: 36_500,
      audioPath: '/tmp/m1.wav',
    });
    expect(
      got.segments.every((s) => s.source === 'live' && s.speakerName),
    ).toBe(true);
    expect(h['memo.list']({}).map((r) => r.meeting.id)).toEqual(['m1']);
    expect(
      h['memo.list']({ query: 'onboarding copy' })[0]?.hit?.snippet,
    ).toMatch(/\[onboarding\]/);
    expect(h['memo.list']({ query: 'zebra' })).toEqual([]);
    const ex = h['memo.export']({ id: 'm1' });
    expect(ex.filename).toMatch(/weekly-product-sync\.md$/);
    expect(ex.markdown).toContain('## Action items');
  });

  it('names live segments from AX and falls back to extractive when the LLM fails', async () => {
    const logs: string[] = [];
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
      log: (m) => logs.push(m),
    });
    seed(h);
    const s = await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1,
      durationMs: 36_500,
      active: fixtureActiveSpeakers(),
      llm: { provider: 'openai', apiKey: 'sk-test' },
    });
    expect(s.provider).toBe('extractive');
    expect(logs).toContain('llm failed; using extractive fallback');
    expect(h['memo.get']({ id: 'm1' }).segments[1]?.speakerName).toBe(
      'Grace Hopper',
    );
  });

  it('rejects unknown meetings with E_NOT_FOUND', () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    expect(() => h['memo.get']({ id: 'nope' })).toThrow(/not found/);
  });

  it('M5.1: recovers meetings stuck in recording/processing (repairs the WAV, summarizes the live transcript)', async () => {
    const fs = await import('node:fs');
    const os = await import('node:os');
    const path = await import('node:path');
    const wav = path.join(
      fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-rec-')),
      'r.wav',
    );
    // 0-length header (helper killed mid-recording) + 2 s of 16 kHz 16-bit silence
    const h = Buffer.alloc(44);
    h.write('RIFF', 0, 'ascii');
    h.write('WAVEfmt ', 8, 'ascii');
    h.writeUInt32LE(16, 16);
    h.writeUInt16LE(1, 20);
    h.writeUInt16LE(1, 22);
    h.writeUInt32LE(16_000, 24);
    h.writeUInt32LE(32_000, 28);
    h.writeUInt16LE(2, 32);
    h.writeUInt16LE(16, 34);
    h.write('data', 36, 'ascii');
    fs.writeFileSync(wav, Buffer.concat([h, Buffer.alloc(64_000)]));

    const h2 = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    seed(h2);
    const m = h2['memo.get']({ id: 'm1' }).meeting;
    h2['memo.create']({ meeting: { ...m, audioPath: wav } });
    const r = await h2['memo.recover']({ llm: { provider: 'none' } });
    expect(r).toEqual({ recovered: ['m1'], failed: [] });
    const got = h2['memo.get']({ id: 'm1' });
    expect(got.meeting.status).toBe('done');
    expect(got.meeting.durationMs).toBe(2000);
    expect(got.summary?.provider).toBe('extractive');
    expect(fs.readFileSync(wav).readUInt32LE(40)).toBe(64_000);
    // idempotent: nothing left to recover
    expect(await h2['memo.recover']({ llm: { provider: 'none' } })).toEqual({
      recovered: [],
      failed: [],
    });
  });

  it('M6.4: prunes audio of finished meetings past retention, keeps transcript', async () => {
    const fs = await import('node:fs');
    const os = await import('node:os');
    const path = await import('node:path');
    const wav = path.join(
      fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-ret-')),
      'a.wav',
    );
    fs.writeFileSync(wav, 'x');
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    seed(h);
    await h['memo.finalize']({
      meetingId: 'm1',
      endedAt: 1_760_000_036_500,
      durationMs: 36_500,
      audioPath: wav,
      active: [],
      llm: { provider: 'none' },
    });
    expect(h['memo.pruneAudio']({ days: 0 }).pruned).toEqual([]);
    expect(
      h['memo.pruneAudio']({ days: 30, now: 1_760_000_036_500 + 86_400_000 })
        .pruned,
    ).toEqual([]);
    expect(
      h['memo.pruneAudio']({
        days: 30,
        now: 1_760_000_036_500 + 31 * 86_400_000,
      }).pruned,
    ).toEqual(['m1']);
    expect(fs.existsSync(wav)).toBe(false);
    const got = h['memo.get']({ id: 'm1' });
    expect(got.meeting.audioPath).toBeUndefined();
    expect(got.segments).toHaveLength(8);
  });

  it('perf: a 2-hour meeting (1500 segments) seeds, summarizes, loads and searches within budget', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: noFetch,
    });
    let t = performance.now();
    const r = await h['dev.seedLarge']({
      id: 'big',
      minutes: 120,
      startedAt: 1_760_000_000_000,
    });
    const seedMs = performance.now() - t;
    expect(r.segments).toBe(1500);
    t = performance.now();
    const got = h['memo.get']({ id: 'big' });
    const getMs = performance.now() - t;
    expect(got.segments).toHaveLength(1500);
    expect(got.summary?.actionItems.length).toBeGreaterThan(0);
    t = performance.now();
    const hits = h['memo.list']({ query: 'migration plan' });
    const searchMs = performance.now() - t;
    const parsed = z.array(MeetingListItem).safeParse(hits);
    expect(parsed.error?.issues ?? []).toEqual([]);
    expect(hits.length).toBeGreaterThan(0);
    t = performance.now();
    const md = h['memo.export']({ id: 'big' }).markdown;
    const exportMs = performance.now() - t;
    expect(md.length).toBeGreaterThan(100_000);
    // Generous CI budgets; typical numbers are ~10x lower.
    expect(seedMs).toBeLessThan(5000);
    expect(getMs).toBeLessThan(500);
    expect(searchMs).toBeLessThan(200);
    expect(exportMs).toBeLessThan(1000);
  });
});
