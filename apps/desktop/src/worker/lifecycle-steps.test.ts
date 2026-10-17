import { describe, expect, it } from 'vitest';
import { newPipeline, type FetchLike } from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const meeting = (id: string, status: 'recording' | 'processing' | 'done') => ({
  id,
  title: 'T',
  platform: 'zoom' as const,
  startedAt: 1,
  status,
  participants: [],
});
const seg = {
  id: 'u0',
  startMs: 0,
  endMs: 900,
  text: 'We ship on Friday.',
  source: 'live' as const,
};
const failing =
  (status: number): FetchLike =>
  async () =>
    new Response('{"error":{"message":"x"}}', { status });
const llm = { provider: 'openrouter' as const, apiKey: 'sk' };

describe('worker lifecycle steps', () => {
  it('stepSummary is strict: surfaces retryable vs terminal provider errors and leaves status alone', async () => {
    for (const [status, retryable] of [
      [503, true],
      [429, true],
      [401, false],
    ] as const) {
      const h = createWorkerHandlers(openDatabase(':memory:'), {
        fetch: failing(status),
      });
      h['memo.create']({ meeting: meeting('m', 'processing') });
      h['memo.appendLive']({ meetingId: 'm', segment: seg });
      const err = await h['memo.stepSummary']({ meetingId: 'm', llm }).catch(
        (e) => e,
      );
      expect(err.retryable ?? false).toBe(retryable);
      expect(h['memo.get']({ id: 'm' }).meeting.status).toBe('processing');
      expect(h['memo.get']({ id: 'm' }).summary).toBeUndefined();
    }
  });

  it('stepSummary fallback saves the extractive summary with the LLM error and marks done', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: failing(503),
    });
    h['memo.create']({ meeting: meeting('m', 'processing') });
    h['memo.appendLive']({ meetingId: 'm', segment: seg });
    await h['memo.stepSummary']({
      meetingId: 'm',
      llm,
      fallback: { error: 'LLM HTTP 503' },
    });
    const m = h['memo.get']({ id: 'm' });
    expect(m.meeting.status).toBe('done');
    expect(m.summary?.llmError).toBe('LLM HTTP 503');
  });

  it('stepFinalize is idempotent and pipeline rows round-trip; orphans exclude meetings with a pipeline', () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: failing(500),
    });
    h['memo.create']({ meeting: meeting('a', 'recording') });
    h['memo.create']({ meeting: meeting('b', 'processing') });
    h['memo.create']({ meeting: meeting('c', 'done') });
    h['memo.appendLive']({ meetingId: 'a', segment: seg });
    const ctx = {
      startedAt: 1,
      endedAt: 2,
      durationMs: 0,
      audioParts: [],
      active: [{ atMs: 0, name: 'Ada' }],
      helperCrashed: true,
    };
    const r1 = h['memo.stepFinalize']({ meetingId: 'a', ctx });
    const r2 = h['memo.stepFinalize']({ meetingId: 'a', ctx });
    expect(r1).toEqual(r2);
    expect(h['memo.get']({ id: 'a' }).segments[0]?.speakerName).toBe('Ada');
    const p = newPipeline('a', 'crash-recovered', ctx, 5);
    h['pipeline.save']({ state: p, settled: false });
    expect(h['pipeline.get']({ id: 'a' })).toEqual(p);
    expect(h['pipeline.listUnsettled']().map((x) => x.meetingId)).toEqual([
      'a',
    ]);
    expect(h['pipeline.orphans']().map((m) => m.id)).toEqual(['b']);
    h['pipeline.save']({ state: p, settled: true });
    expect(h['pipeline.listUnsettled']()).toEqual([]);
    // Deleting the meeting cascades the pipeline row; saving for a deleted meeting is a no-op.
    h['memo.delete']({ id: 'a' });
    expect(h['pipeline.get']({ id: 'a' })).toBeNull();
    expect(h['pipeline.save']({ state: p, settled: false })).toEqual({
      ok: false,
    });
  });

  it('session snapshot round-trips and ignores corrupt rows', () => {
    const db = openDatabase(':memory:');
    const h = createWorkerHandlers(db, { fetch: failing(500) });
    expect(h['lifecycle.loadSession']()).toBeNull();
    const s = {
      phase: 'recording' as const,
      meetingId: 'm',
      startedAt: 1,
      degraded: ['mic-only' as const],
      audioParts: ['/a'],
      partOffsetMs: 0,
    };
    h['lifecycle.saveSession']({ state: s });
    expect(h['lifecycle.loadSession']()).toEqual(s);
    db.prepare(`UPDATE lifecycle_state SET json='{"phase":"bogus"}'`).run();
    expect(h['lifecycle.loadSession']()).toBeNull();
  });
});
