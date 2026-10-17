import { describe, expect, it } from 'vitest';
import {
  LifecycleMachine,
  RETRY_POLICIES,
  applyTranscriptDelta,
  initialRecordingState,
  lifecycleReducer,
  newPipeline,
  nextRunnableStep,
  pipelineSettled,
  pipelineStep,
  pipelineUsable,
  resumeAfterRestart,
  retryDelay,
  type LifecycleEvent,
  type RecordingState,
} from './lifecycle';

const meeting = {
  key: 'k',
  title: 'Standup',
  platform: 'zoom' as const,
  app: 'Zoom',
};
const run = (
  events: LifecycleEvent[],
  s: RecordingState = initialRecordingState(),
) => {
  const m = new LifecycleMachine(s);
  const accepted = events.map((e) => m.send(e));
  return { s: m.state, accepted };
};
const ctx = {
  startedAt: 0,
  endedAt: 1,
  durationMs: 1,
  audioParts: [],
  active: [],
  helperCrashed: false,
};
const err = (retryable = true) => ({ code: 'E', message: 'boom', retryable });

describe('recording lifecycle reducer', () => {
  it('happy path: detected → starting → recording → stopping → processing → done', () => {
    const { s, accepted } = run([
      { type: 'detected', meeting },
      { type: 'start-requested', meetingId: 'm', at: 10, audioPath: '/a.wav' },
      { type: 'start-acked' },
      { type: 'stop-requested', reason: 'manual', at: 20 },
      { type: 'stopped', at: 21 },
      { type: 'processing-done' },
    ]);
    expect(accepted.every(Boolean)).toBe(true);
    expect(s).toMatchObject({
      phase: 'done',
      meetingId: 'm',
      stopReason: 'manual',
      audioParts: ['/a.wav'],
    });
  });

  it('failed start returns to the resting phase with a structured error', () => {
    const { s } = run([
      { type: 'detected', meeting },
      { type: 'start-requested', meetingId: 'm', at: 1, audioPath: '/a' },
      { type: 'start-failed', error: err() },
    ]);
    expect(s.phase).toBe('detected');
    expect(s.error?.message).toBe('boom');
    expect(s.meetingId).toBeUndefined();
  });

  it('helper loss → reconnecting → restored into a new part with an offset and a gap flag', () => {
    const { s } = run([
      { type: 'start-requested', meetingId: 'm', at: 1000, audioPath: '/p0' },
      { type: 'start-acked' },
      { type: 'helper-lost', at: 2000, deadlineAt: 32000 },
      { type: 'helper-lost', at: 2500, deadlineAt: 32000 },
      { type: 'helper-restored', at: 4000, audioPath: '/p1' },
    ]);
    expect(s).toMatchObject({
      phase: 'recording',
      audioParts: ['/p0', '/p1'],
      partOffsetMs: 3000,
      degraded: ['transcript-gap'],
    });
    expect(s.reconnect).toBeUndefined();
  });

  it('stop is legal while reconnecting (salvage) and rejected when idle', () => {
    const r = run([
      { type: 'start-requested', meetingId: 'm', at: 0, audioPath: '/p0' },
      { type: 'start-acked' },
      { type: 'helper-lost', at: 1, deadlineAt: 2 },
      { type: 'stop-requested', reason: 'helper-lost', at: 3 },
    ]);
    expect(r.s).toMatchObject({ phase: 'stopping', stopReason: 'helper-lost' });
    expect(
      lifecycleReducer(initialRecordingState(), {
        type: 'stop-requested',
        reason: 'manual',
        at: 0,
      }),
    ).toBeNull();
  });

  it('degradations are deduplicated and only apply while capturing', () => {
    const { s, accepted } = run([
      { type: 'degraded', flag: 'mic-only' },
      { type: 'start-requested', meetingId: 'm', at: 0, audioPath: '/p' },
      { type: 'degraded', flag: 'mic-only' },
      { type: 'degraded', flag: 'mic-only' },
      { type: 'degraded', flag: 'ax-unavailable' },
    ]);
    expect(accepted).toEqual([false, true, true, false, true]);
    expect(s.degraded).toEqual(['mic-only', 'ax-unavailable']);
  });

  it('reset cannot abandon a live capture; a new recording may start while the last one post-processes', () => {
    const rec = run([
      { type: 'start-requested', meetingId: 'm', at: 0, audioPath: '/p' },
    ]).s;
    expect(lifecycleReducer(rec, { type: 'reset' })).toBeNull();
    const processing = run([
      { type: 'start-requested', meetingId: 'm', at: 0, audioPath: '/p' },
      { type: 'stop-requested', reason: 'manual', at: 1 },
      { type: 'stopped', at: 1 },
    ]).s;
    expect(
      lifecycleReducer(processing, {
        type: 'start-requested',
        meetingId: 'n',
        at: 2,
        audioPath: '/q',
      })?.meetingId,
    ).toBe('n');
  });

  it('rejected start records the error without changing phase; clear-error removes it', () => {
    const { s } = run([
      { type: 'detected', meeting },
      { type: 'rejected', error: err() },
    ]);
    expect(s).toMatchObject({ phase: 'detected', error: { code: 'E' } });
    expect(lifecycleReducer(s, { type: 'clear-error' })?.error).toBeUndefined();
  });
});

describe('post-processing pipeline', () => {
  it('retries with backoff up to maxAttempts, then fails', () => {
    let p = newPipeline('m', 'manual', ctx, 0);
    for (let i = 1; i <= RETRY_POLICIES.summary.maxAttempts; i++) {
      p = pipelineStep(p, 'finalize', { kind: 'start', now: 0 });
      p = pipelineStep(p, 'finalize', { kind: 'success', now: 0 });
      p = pipelineStep(p, 'summary', { kind: 'start', now: i });
      p = pipelineStep(p, 'summary', {
        kind: 'failure',
        now: i,
        error: err(),
        delayMs: 100,
      });
      expect(p.steps.summary.attempts).toBe(i);
    }
    expect(p.steps.summary.status).toBe('failed');
    expect(nextRunnableStep(p)).toBe('index'); // summary failure is soft
    expect(pipelineUsable(p)).toBe(true);
  });

  it('non-retryable errors fail immediately; retrying carries nextRetryAt', () => {
    let p = newPipeline('m', 'manual', ctx, 0);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 0 });
    const r = pipelineStep(p, 'finalize', {
      kind: 'failure',
      now: 5,
      error: err(true),
      delayMs: 100,
    });
    expect(r.steps.finalize).toMatchObject({
      status: 'retrying',
      nextRetryAt: 105,
    });
    const f = pipelineStep(p, 'finalize', {
      kind: 'failure',
      now: 5,
      error: err(false),
      delayMs: 100,
    });
    expect(f.steps.finalize.status).toBe('failed');
    expect(nextRunnableStep(f)).toBeNull(); // finalize failure blocks everything after it
    expect(pipelineSettled(f)).toBe(true);
  });

  it('manual retry grants a fresh budget and re-arms later steps', () => {
    let p = newPipeline('m', 'manual', ctx, 0);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 0 });
    p = pipelineStep(p, 'finalize', {
      kind: 'failure',
      now: 0,
      error: err(false),
      delayMs: 0,
    });
    p = pipelineStep(p, 'finalize', { kind: 'manual-retry', now: 1 });
    expect(p.steps.finalize).toMatchObject({ status: 'pending', attempts: 0 });
    expect(nextRunnableStep(p)).toBe('finalize');
  });

  it('retryDelay is exponential, capped, jittered ±20%, and scalable', () => {
    const pol = { maxAttempts: 5, baseMs: 1000, maxMs: 5000 };
    expect(retryDelay(pol, 1, () => 0.5)).toBe(1000);
    expect(retryDelay(pol, 3, () => 0.5)).toBe(4000);
    expect(retryDelay(pol, 9, () => 0.5)).toBe(5000);
    expect(retryDelay(pol, 1, () => 0)).toBe(800);
    expect(retryDelay(pol, 1, () => 1)).toBe(1200);
    expect(retryDelay(pol, 1, () => 0.5, 0.01)).toBe(10);
  });
});

describe('live transcript deltas', () => {
  const seg = (id: string) => ({ id, text: id, startMs: 0, endMs: 1 });
  it('appends finals, replaces the partial, and signals gaps', () => {
    const t0 = { meetingId: 'm', rev: 0, segments: [], partial: null };
    const t1 = applyTranscriptDelta(t0, {
      meetingId: 'm',
      baseRev: 0,
      rev: 1,
      append: [],
      partial: { id: 'u0', text: 'hel', startMs: 0 },
    });
    const t2 = applyTranscriptDelta(t1, {
      meetingId: 'm',
      baseRev: 1,
      rev: 2,
      append: [],
      partial: { id: 'u0', text: 'hello wor', startMs: 0 },
    });
    const t3 = applyTranscriptDelta(t2, {
      meetingId: 'm',
      baseRev: 2,
      rev: 3,
      append: [seg('u0')],
      partial: null,
    });
    expect(t2?.partial?.text).toBe('hello wor');
    expect(t3).toMatchObject({ rev: 3, segments: [seg('u0')], partial: null });
    expect(
      applyTranscriptDelta(t3, {
        meetingId: 'm',
        baseRev: 5,
        rev: 6,
        append: [],
        partial: null,
      }),
    ).toBeNull();
    expect(
      applyTranscriptDelta(t3, {
        meetingId: 'x',
        baseRev: 3,
        rev: 4,
        append: [],
        partial: null,
      }),
    ).toBeNull();
    // duplicate append is idempotent
    expect(
      applyTranscriptDelta(t3, {
        meetingId: 'm',
        baseRev: 3,
        rev: 4,
        append: [seg('u0')],
        partial: null,
      })?.segments,
    ).toHaveLength(1);
  });
});

describe('hardening: clock skew, mid-recording degrade, quit mid-pipeline', () => {
  it('helper restore with a backward clock jump clamps partOffsetMs to 0', () => {
    const { s } = run([
      { type: 'start-requested', meetingId: 'm', at: 10_000, audioPath: '/p0' },
      { type: 'start-acked' },
      { type: 'helper-lost', at: 12_000, deadlineAt: 42_000 },
      // Wall clock jumped back before the restart ack (NTP / sleep / VM).
      { type: 'helper-restored', at: 9_000, audioPath: '/p1' },
    ]);
    expect(s.partOffsetMs).toBe(0);
    expect(s.degraded).toContain('transcript-gap');
    expect(s.audioParts).toEqual(['/p0', '/p1']);
  });

  it('permission / audio loss mid-recording is accepted as degradation (not a stop)', () => {
    const { s, accepted } = run([
      { type: 'start-requested', meetingId: 'm', at: 0, audioPath: '/p' },
      { type: 'start-acked' },
      { type: 'degraded', flag: 'audio-unavailable' },
      { type: 'degraded', flag: 'asr-unavailable' },
    ]);
    expect(accepted.slice(2)).toEqual([true, true]);
    expect(s.phase).toBe('recording');
    expect(s.degraded).toEqual(['audio-unavailable', 'asr-unavailable']);
  });

  it('resumeAfterRestart turns running steps into retrying and keeps attempts', () => {
    let p = newPipeline('m', 'crash-recovered', ctx, 0);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 5 });
    expect(p.steps.finalize).toMatchObject({ status: 'running', attempts: 1 });
    const r = resumeAfterRestart(p);
    expect(r.steps.finalize).toMatchObject({
      status: 'retrying',
      attempts: 1,
    });
    expect(nextRunnableStep(r)).toBe('finalize');
    // Already-done steps stay done; pending stay pending.
    expect(r.steps.summary.status).toBe('pending');
  });

  it('quit mid-pipeline: running summary becomes retrying; finalize stays done', () => {
    let p = newPipeline('m', 'manual', ctx, 0);
    p = pipelineStep(p, 'finalize', { kind: 'start', now: 1 });
    p = pipelineStep(p, 'finalize', { kind: 'success', now: 2 });
    p = pipelineStep(p, 'summary', { kind: 'start', now: 3 });
    const r = resumeAfterRestart(p);
    expect(r.steps.finalize.status).toBe('done');
    expect(r.steps.summary).toMatchObject({ status: 'retrying', attempts: 1 });
    expect(nextRunnableStep(r)).toBe('summary');
    expect(pipelineUsable(r)).toBe(false);
  });
});
