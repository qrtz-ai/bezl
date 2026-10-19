/**
 * Stress scenarios that keep running every `bun run test` (hardening cycle 1).
 * Prefer unit/integration here; e2e harness ownership stays with Beve Bobs.
 */
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import { AppError } from '../errors';
import { migrate, migrations } from '../db/migrate';
import { syntheticSegments } from './large';
import { summarizeMeeting, type Summarizer } from './llm';
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
    expect(again.to).toBe(6);
    expect(again.applied).toEqual([
      '3_segment_vectors',
      '4_notes_templates',
      '5_recording_lifecycle',
      '6_calendar',
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

describe('hardening stress: on-device summary / disk-ish finalize', () => {
  const segs = syntheticSegments(1).slice(0, 3);
  it('a summarizer that fails after the deadline is a retryable E_TIMEOUT, never terminal', async () => {
    const ac = new AbortController();
    const late: Summarizer = {
      id: 'on-device',
      complete: async () => {
        ac.abort();
        throw new AppError('E_VALIDATION', 'late terminal-looking error');
      },
    };
    const err = await summarizeMeeting('t', segs, late, {
      signal: ac.signal,
      strict: true,
    }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(AppError);
    expect((err as AppError).code).toBe('E_TIMEOUT');
    expect((err as AppError).retryable).toBe(true);
  });

  it('no model: the extractive summary, nothing throws and nothing is sent anywhere', async () => {
    expect((await summarizeMeeting('t', segs, null)).provider).toBe(
      'extractive',
    );
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
