import { describe, expect, it } from 'vitest';
import { newPipeline, type Embedder, type FetchLike } from '@qrtz/core';
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
      h['note.create']({ meeting: meeting('m', 'processing') });
      h['note.appendLive']({ meetingId: 'm', segment: seg });
      const err = await h['note.stepSummary']({ meetingId: 'm', llm }).catch(
        (e) => e,
      );
      expect(err.retryable ?? false).toBe(retryable);
      expect(h['note.get']({ id: 'm' }).meeting.status).toBe('processing');
      expect(h['note.get']({ id: 'm' }).summary).toBeUndefined();
    }
  });

  it('stepSummary fallback saves the extractive summary with the LLM error and marks done', async () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: failing(503),
    });
    h['note.create']({ meeting: meeting('m', 'processing') });
    h['note.appendLive']({ meetingId: 'm', segment: seg });
    await h['note.stepSummary']({
      meetingId: 'm',
      llm,
      fallback: { error: 'LLM HTTP 503' },
    });
    const m = h['note.get']({ id: 'm' });
    expect(m.meeting.status).toBe('done');
    expect(m.summary?.llmError).toBe('LLM HTTP 503');
  });

  it('stepSummary lists the recording silent stretches from its pipeline row as capture gaps (N29)', async () => {
    const bodies: string[] = [];
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: async (_url, init) => {
        bodies.push(String(init.body));
        return new Response('{"error":{"message":"x"}}', { status: 503 });
      },
    });
    h['note.create']({ meeting: meeting('m', 'processing') });
    h['note.appendLive']({ meetingId: 'm', segment: seg });
    const ctx = {
      startedAt: 1,
      endedAt: 2,
      durationMs: 600_000,
      audioParts: [],
      active: [],
      helperCrashed: false,
    };
    h['pipeline.save']({
      state: {
        ...newPipeline('m', 'manual', ctx, 1),
        capture: {
          silentSpans: [
            { source: 'mic', startMs: 60_000, endMs: 240_000 },
            { source: 'system', startMs: 120_000 },
          ],
        },
      },
      settled: false,
    });
    await h['note.stepSummary']({ meetingId: 'm', llm }).catch(() => {});
    expect(bodies).toHaveLength(1);
    const prompt = JSON.stringify(JSON.parse(bodies[0]!).messages);
    expect(prompt).toContain('Capture gaps:');
    expect(prompt).toContain("1:00–2:00: no audio from this user's microphone");
    expect(prompt).toContain('2:00–4:00: no audio at all');
    expect(prompt).toContain(
      '4:00–10:00: no audio from the other participants',
    );
  });

  it('stepFinalize is idempotent and pipeline rows round-trip; orphans exclude meetings with a pipeline', () => {
    const h = createWorkerHandlers(openDatabase(':memory:'), {
      fetch: failing(500),
    });
    h['note.create']({ meeting: meeting('a', 'recording') });
    h['note.create']({ meeting: meeting('b', 'processing') });
    h['note.create']({ meeting: meeting('c', 'done') });
    h['note.appendLive']({ meetingId: 'a', segment: seg });
    const ctx = {
      startedAt: 1,
      endedAt: 2,
      durationMs: 0,
      audioParts: [],
      active: [{ atMs: 0, name: 'Ada' }],
      helperCrashed: true,
    };
    const r1 = h['note.stepFinalize']({ meetingId: 'a', ctx });
    const r2 = h['note.stepFinalize']({ meetingId: 'a', ctx });
    expect(r1).toEqual(r2);
    expect(h['note.get']({ id: 'a' }).segments[0]?.speakerName).toBe('Ada');
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
    h['note.delete']({ id: 'a' });
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

/** An OpenRouter-shaped reply whose summary text is `text`. */
const reply = (text: string) =>
  new Response(
    JSON.stringify({
      id: 'x',
      choices: [
        {
          index: 0,
          message: {
            role: 'assistant',
            content: JSON.stringify({ summary: text, actionItems: [], qa: [] }),
          },
        },
      ],
    }),
    { status: 200 },
  );
const sleepMs = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Hardening cycle 2 (ADR 0005): a summary attempt the host gave up on must be over in the worker too.
describe('stepSummary deadline (slow OpenRouter)', () => {
  const setup = (fetch: FetchLike) => {
    const h = createWorkerHandlers(openDatabase(':memory:'), { fetch });
    h['note.create']({ meeting: meeting('m', 'processing') });
    h['note.appendLive']({ meetingId: 'm', segment: seg });
    return h;
  };

  it('a provider that hangs is aborted at deadlineMs: retryable E_TIMEOUT, nothing saved', async () => {
    let sawSignal = false;
    const hang: FetchLike = (_u, init) =>
      new Promise((_r, reject) => {
        sawSignal = !!init.signal;
        init.signal?.addEventListener('abort', () =>
          reject(Object.assign(new Error('aborted'), { name: 'AbortError' })),
        );
      });
    const h = setup(hang);
    const err = await h['note.stepSummary']({
      meetingId: 'm',
      llm,
      deadlineMs: 30,
    }).catch((e) => e);
    expect(sawSignal).toBe(true);
    expect(err.code).toBe('E_TIMEOUT');
    expect(err.retryable).toBe(true);
    expect(h['note.get']({ id: 'm' }).summary).toBeUndefined();
    expect(h['note.get']({ id: 'm' }).meeting.status).toBe('processing');
  });

  it('a reply that ignores the abort and lands after the deadline is dropped, not saved', async () => {
    const late: FetchLike = async () => {
      await sleepMs(60);
      return reply('stale');
    };
    const h = setup(late);
    const err = await h['note.stepSummary']({
      meetingId: 'm',
      llm,
      deadlineMs: 20,
    }).catch((e) => e);
    expect(err.code).toBe('E_TIMEOUT');
    expect(h['note.get']({ id: 'm' }).summary).toBeUndefined();
  });

  it('a timed-out attempt can never overwrite the retry that replaced it, or the fallback', async () => {
    let n = 0;
    const fetch: FetchLike = async () => {
      const i = ++n;
      if (i === 1) {
        await sleepMs(80); // attempt 1: the host already timed it out
        return reply('stale attempt 1');
      }
      return reply('fresh attempt 2');
    };
    const h = setup(fetch);
    const a1 = h['note.stepSummary']({ meetingId: 'm', llm, deadlineMs: 20 });
    await sleepMs(30);
    await h['note.stepSummary']({ meetingId: 'm', llm, deadlineMs: 1000 });
    await a1.catch(() => {});
    await sleepMs(70);
    expect(h['note.get']({ id: 'm' }).summary?.summary).toBe('fresh attempt 2');

    // Exhausted → fallback: a late LLM reply must not replace the extractive summary the pipeline recorded.
    let m = 0;
    const h2 = setup(async () => {
      m++;
      await sleepMs(60);
      return reply('late llm');
    });
    const late = h2['note.stepSummary']({
      meetingId: 'm',
      llm,
      deadlineMs: 15,
    });
    await h2['note.stepSummary']({
      meetingId: 'm',
      llm,
      fallback: { error: 'LLM timed out' },
    });
    await late.catch(() => {});
    await sleepMs(70);
    expect(m).toBe(1);
    expect(h2['note.get']({ id: 'm' }).summary?.llmError).toBe('LLM timed out');
  });

  it('the default deadline sits inside the host RPC timeout', async () => {
    const { SUMMARY_DEADLINE_MS } = await import('./handlers');
    expect(SUMMARY_DEADLINE_MS).toBeLessThan(120_000);
  });
});

// Hardening cycle 3 (N3, ADR 0007): the index step gets the same worker-side deadline as the summary.
describe('stepIndex deadline (hung on-device embedder)', () => {
  const lines = (n: number) =>
    Array.from({ length: n }, (_, i) => ({
      id: `u${i}`,
      startMs: i * 1000,
      endMs: i * 1000 + 900,
      text: `line ${i} about the launch`,
      source: 'live' as const,
    }));
  const vec = () => {
    const v = new Float32Array(4);
    v[0] = 1;
    return v;
  };
  const setup = (embedder: () => Promise<Embedder | null>, n = 100) => {
    const db = openDatabase(':memory:');
    const h = createWorkerHandlers(db, { fetch: failing(500), embedder });
    h['note.create']({ meeting: meeting('m', 'processing') });
    for (const s of lines(n))
      h['note.appendLive']({ meetingId: 'm', segment: s });
    const vectors = () =>
      Number(
        (
          db.prepare('SELECT count(*) AS n FROM segment_vectors').get() as {
            n: number;
          }
        ).n,
      );
    return { h, vectors };
  };

  it('an embed that never resolves ends the attempt at deadlineMs: retryable E_TIMEOUT, nothing saved', async () => {
    const hung: Embedder = { id: 'e', embed: () => new Promise(() => {}) };
    const { h, vectors } = setup(async () => hung);
    const err = await h['note.stepIndex']({
      meetingId: 'm',
      deadlineMs: 30,
    }).catch((e) => e);
    expect(err.code).toBe('E_TIMEOUT');
    expect(err.retryable).toBe(true);
    expect(vectors()).toBe(0);
  });

  it('a model load that hangs (first-run download) is bounded too', async () => {
    const { h } = setup(() => new Promise(() => {}));
    const err = await h['note.stepIndex']({
      meetingId: 'm',
      deadlineMs: 30,
    }).catch((e) => e);
    expect(err.code).toBe('E_TIMEOUT');
  });

  it('a batch that lands after the deadline is dropped; earlier batches stay and the retry resumes', async () => {
    let calls = 0;
    const slowSecond: Embedder = {
      id: 'e',
      embed: async (texts) => {
        if (++calls === 2) await sleepMs(80); // lands after the 30 ms deadline
        return texts.map(vec);
      },
    };
    const { h, vectors } = setup(async () => slowSecond);
    const err = await h['note.stepIndex']({
      meetingId: 'm',
      deadlineMs: 30,
    }).catch((e) => e);
    expect(err.code).toBe('E_TIMEOUT');
    await sleepMs(100); // the abandoned batch resolves now
    expect(vectors()).toBe(64); // batch 1 only; batch 2 was never written
    const r = await h['note.stepIndex']({ meetingId: 'm', deadlineMs: 1000 });
    expect(r).toEqual({ indexed: 36, skipped: false });
    expect(vectors()).toBe(100);
  });

  it('the default deadline sits inside the host RPC timeout', async () => {
    const { INDEX_DEADLINE_MS } = await import('./handlers');
    expect(INDEX_DEADLINE_MS).toBeLessThan(300_000);
  });
});
