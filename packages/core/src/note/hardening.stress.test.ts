/**
 * Stress scenarios that keep running every `bun run test` (hardening cycle 1).
 * Prefer unit/integration here; e2e harness ownership stays with Beve Bobs.
 */
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import { AppError } from '../errors';
import { migrate, migrations } from '../db/migrate';
import { EngineArbiter } from '../talk/asr';
import { syntheticSegments } from './large';
import { createProvider, summarizeMeeting, type FetchLike } from './llm';
import { NoteRepo } from './repo';
import {
  RETRY_POLICIES,
  newPipeline,
  nextRunnableStep,
  pipelineSettled,
  pipelineStep,
  pipelineUsable,
  resumeAfterRestart,
  retryDelay,
} from './lifecycle';

const ctx = {
  startedAt: 0,
  endedAt: 1,
  durationMs: 1,
  audioParts: ['/a.wav'],
  active: [],
  helperCrashed: true,
};

describe('hardening stress: DB versions', () => {
  it('migrates a DB left at an older schema version (v2 → tip)', () => {
    const db = new DatabaseSync(':memory:');
    const early = migrations.filter((m) => m.version <= 2);
    expect(migrate(db, early)).toMatchObject({ from: 0, to: 2 });
    const again = migrate(db, migrations);
    expect(again.from).toBe(2);
    expect(again.to).toBe(5);
    expect(again.applied).toEqual([
      '3_segment_vectors',
      '4_notes_templates',
      '5_recording_lifecycle',
    ]);
  });

  it('refuses a DB from a newer app version (forward-incompatible)', () => {
    const db = new DatabaseSync(':memory:');
    migrate(db, migrations);
    db.exec('PRAGMA user_version = 99');
    expect(() => migrate(db, migrations)).toThrow(/newer than this app/);
  });
});

describe('hardening stress: huge transcripts', () => {
  it('indexes and searches a ~2h synthetic transcript without throwing', () => {
    const db = new DatabaseSync(':memory:');
    migrate(db, migrations);
    const repo = new NoteRepo(db);
    const segs = syntheticSegments(120); // ~2h at 4.8s/slot
    expect(segs.length).toBeGreaterThan(1400);
    repo.upsertMeeting({
      id: 'huge',
      title: 'All-hands marathon',
      platform: 'zoom',
      startedAt: 1,
      status: 'done',
      participants: ['Ada'],
    });
    repo.replaceSegments('huge', 'live', segs);
    const hits = repo.search('latency pipeline', 10);
    expect(hits.length).toBeGreaterThan(0);
    expect(hits[0]!.meetingId).toBe('huge');
    const listed = repo.listMeetings(5, 0);
    expect(listed.some((m) => m.id === 'huge')).toBe(true);
  });
});

describe('hardening stress: OpenRouter / network / disk-ish finalize', () => {
  it('network loss during summary is retryable (E_UNAVAILABLE), not E_INTERNAL', async () => {
    const f: FetchLike = async () => {
      throw new TypeError('fetch failed');
    };
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    const err = await p
      .complete([{ role: 'user', content: 'hi' }])
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AppError);
    const a = err as AppError;
    expect(a.code).toBe('E_UNAVAILABLE');
    expect(a.retryable).toBe(true);
    expect(a.message).toMatch(/network/i);
  });

  it('abort mid-request is retryable E_TIMEOUT', async () => {
    const f: FetchLike = async (_u, init) => {
      const e = new Error('aborted');
      e.name = 'AbortError';
      // honour the signal the caller attached
      if (init?.signal?.aborted) throw e;
      throw e;
    };
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    const ac = new AbortController();
    ac.abort();
    const err = await p
      .complete([{ role: 'user', content: 'hi' }], { signal: ac.signal })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AppError);
    expect((err as AppError).code).toBe('E_TIMEOUT');
    expect((err as AppError).retryable).toBe(true);
  });

  it('slow OpenRouter: non-strict summarize falls back extractive; strict still throws retryable', async () => {
    const f: FetchLike = async () => {
      throw new TypeError('Connect Timeout Error');
    };
    const p = createProvider('openrouter', { fetch: f, apiKey: 'k' })!;
    const segs = syntheticSegments(1).slice(0, 3);
    const soft = await summarizeMeeting('t', segs, p);
    expect(soft.provider).toBe('extractive');
    expect(soft.llmError).toMatch(/network|Timeout/i);

    await expect(
      summarizeMeeting('t', segs, p, undefined, { strict: true }),
    ).rejects.toMatchObject({ code: 'E_UNAVAILABLE', retryable: true });
  });

  it('finalize exhaustion (disk-full stand-in) settles the pipeline as failed + unusable', () => {
    let p = newPipeline('m', 'manual', ctx, 0);
    const disk = {
      code: 'ENOSPC',
      message: 'no space left on device',
      retryable: true,
    };
    for (let i = 0; i < RETRY_POLICIES.finalize.maxAttempts; i++) {
      p = pipelineStep(p, 'finalize', { kind: 'start', now: i });
      p = pipelineStep(p, 'finalize', {
        kind: 'failure',
        now: i,
        error: disk,
        delayMs: retryDelay(RETRY_POLICIES.finalize, i + 1, () => 0.5, 0.01),
      });
    }
    expect(p.steps.finalize.status).toBe('failed');
    expect(p.steps.finalize.error?.code).toBe('ENOSPC');
    expect(nextRunnableStep(p)).toBeNull();
    expect(pipelineSettled(p)).toBe(true);
    expect(pipelineUsable(p)).toBe(false);
  });

  it('app quit mid-summary then relaunch: resumeAfterRestart then summary retries', () => {
    let p = newPipeline('m', 'crash-recovered', ctx, 10);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 11 });
    p = pipelineStep(p, 'finalize', { kind: 'success', now: 12 });
    p = pipelineStep(p, 'summary', { kind: 'start', now: 13 });
    // Crash here (status still `running` on disk).
    p = resumeAfterRestart(p);
    expect(p.steps.summary.status).toBe('retrying');
    p = pipelineStep(p, 'summary', { kind: 'start', now: 20 });
    p = pipelineStep(p, 'summary', {
      kind: 'failure',
      now: 21,
      error: { code: 'E_UNAVAILABLE', message: 'offline', retryable: true },
      delayMs: 1,
      fallback: true,
      terminal: true,
    });
    // Terminal + fallback: failed but soft — index still runs; meeting usable.
    expect(p.steps.summary.status).toBe('failed');
    expect(p.steps.summary.fallback).toBe(true);
    expect(nextRunnableStep(p)).toBe('index');
    expect(pipelineUsable(p)).toBe(true);
  });
});

describe('hardening stress: talk + note concurrency (pure)', () => {
  it('EngineArbiter failover is independent of a note pipeline advancing', async () => {
    const cloud = {
      engine: 'elevenlabs' as const,
      push: () => {},
      finish: () =>
        new Promise<string>((_r, rej) =>
          setTimeout(() => rej(new Error('socket dead')), 5),
        ),
      close: () => {},
    };
    let now = 0;
    const arb = new EngineArbiter({
      cloud,
      stallMs: 50,
      finalMs: 30,
      now: () => now,
    });
    arb.localFinal('hello from device');
    now = 100;
    arb.check();
    expect(arb.failover).toBe(true);
    const r = await arb.finish('hello from device');
    expect(r.engine).toBe('on-device');
    expect(r.failover).toBe(true);

    // Meanwhile a note pipeline for another meeting can settle independently.
    let p = newPipeline('other', 'manual', ctx, 0);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 1 });
    p = pipelineStep(p, 'finalize', { kind: 'success', now: 2 });
    p = pipelineStep(p, 'summary', { kind: 'skip', now: 3 });
    p = pipelineStep(p, 'index', { kind: 'skip', now: 4 });
    expect(pipelineSettled(p)).toBe(true);
  });
});
