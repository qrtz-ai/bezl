import { z } from 'zod';
import { MeetingPlatform } from './types';

/**
 * The single authoritative recording lifecycle (see docs/recording-lifecycle.md).
 *
 * Pure: reducers here own every legal transition. The runtime owner (main's NoteService) feeds events in,
 * persists every accepted state through the worker (SQLite `lifecycle_state` / `pipeline_state`), and
 * broadcasts the derived SessionView / PipelineView over typed IPC. The renderer never infers state.
 */

export const RecordingPhase = z.enum([
  'idle',
  'detected',
  'starting',
  'recording',
  'reconnecting',
  'stopping',
  'processing',
  'done',
  'error',
]);
export type RecordingPhase = z.infer<typeof RecordingPhase>;

export const StopReason = z.enum([
  'manual',
  'meeting-ended',
  'helper-lost',
  'crash-recovered',
]);
export type StopReason = z.infer<typeof StopReason>;

export const Degradation = z.enum([
  /** No speech recogniser could start: audio only, no live transcript. */
  'asr-unavailable',
  /** No audio source started: nothing is being captured. */
  'audio-unavailable',
  /** Screen & System Audio Recording denied: only the microphone is captured (remote voices are faint/missing). */
  'mic-only',
  /** Microphone revoked mid-recording: only system audio (the other side of the call) is captured. */
  'no-mic',
  /** Accessibility denied: no meeting detection, participants or speaker names. */
  'ax-unavailable',
  /** The helper restarted mid-meeting: a short stretch of audio/transcript is missing. */
  'transcript-gap',
]);
export type Degradation = z.infer<typeof Degradation>;

export const LifecycleError = z.object({
  code: z.string(),
  message: z.string(),
  retryable: z.boolean(),
});
export type LifecycleError = z.infer<typeof LifecycleError>;

export const DetectedMeetingSchema = z.object({
  key: z.string(),
  title: z.string(),
  platform: MeetingPlatform,
  app: z.string(),
});

export const RecordingState = z.object({
  phase: RecordingPhase,
  detected: DetectedMeetingSchema.optional(),
  meetingId: z.string().optional(),
  startedAt: z.number().optional(),
  stoppedAt: z.number().optional(),
  stopReason: StopReason.optional(),
  degraded: z.array(Degradation).default([]),
  /** Helper restart in progress while recording. */
  reconnect: z
    .object({ attempt: z.number(), since: z.number(), deadlineAt: z.number() })
    .optional(),
  error: LifecycleError.optional(),
  /** WAV parts in order: one per helper (re)start during this recording. */
  audioParts: z.array(z.string()).default([]),
  /** Offset added to helper timestamps for the current part (ms since startedAt). */
  partOffsetMs: z.number().default(0),
});
export type RecordingState = z.infer<typeof RecordingState>;

export type LifecycleEvent =
  | { type: 'detected'; meeting: z.infer<typeof DetectedMeetingSchema> }
  | { type: 'meeting-ended'; key: string }
  | {
      type: 'start-requested';
      meetingId: string;
      at: number;
      audioPath: string;
    }
  | { type: 'start-acked' }
  | { type: 'start-failed'; error: LifecycleError }
  | { type: 'degraded'; flag: Degradation }
  | { type: 'helper-lost'; at: number; deadlineAt: number }
  | { type: 'helper-restored'; at: number; audioPath: string }
  | { type: 'stop-requested'; reason: StopReason; at: number }
  | { type: 'stopped'; at: number }
  | { type: 'processing-done' }
  | { type: 'processing-failed'; error: LifecycleError }
  | { type: 'rejected'; error: LifecycleError }
  | { type: 'clear-error' }
  | { type: 'reset' };

export const initialRecordingState = (): RecordingState => ({
  phase: 'idle',
  degraded: [],
  audioParts: [],
  partOffsetMs: 0,
});

/** Phases in which a capture is live (the helper owns an open WAV). */
export const CAPTURING: readonly RecordingPhase[] = [
  'starting',
  'recording',
  'reconnecting',
];
export const isCapturing = (p: RecordingPhase) => CAPTURING.includes(p);
/** Phases in which a new recording may begin (post-processing runs per meeting, off the session). */
export const canStart = (p: RecordingPhase) =>
  p === 'idle' ||
  p === 'detected' ||
  p === 'processing' ||
  p === 'done' ||
  p === 'error';

/** Returns the next state, or null when the event is not legal in this state (callers log and ignore). */
export function lifecycleReducer(
  s: RecordingState,
  e: LifecycleEvent,
): RecordingState | null {
  switch (e.type) {
    case 'detected':
      // Allowed while the last meeting post-processes (back-to-back meetings); its pipeline runs on regardless.
      if (!canStart(s.phase)) return null;
      return {
        ...initialRecordingState(),
        phase: 'detected',
        detected: e.meeting,
      };
    case 'meeting-ended':
      if (s.phase !== 'detected' || s.detected?.key !== e.key) return null;
      return initialRecordingState();
    case 'start-requested':
      if (!canStart(s.phase)) return null;
      return {
        ...initialRecordingState(),
        phase: 'starting',
        ...(s.detected ? { detected: s.detected } : {}),
        meetingId: e.meetingId,
        startedAt: e.at,
        audioParts: [e.audioPath],
      };
    case 'start-acked':
      if (s.phase !== 'starting') return null;
      return { ...s, phase: 'recording' };
    case 'start-failed': {
      if (s.phase !== 'starting') return null;
      return {
        ...initialRecordingState(),
        phase: s.detected ? 'detected' : 'idle',
        ...(s.detected ? { detected: s.detected } : {}),
        error: e.error,
      };
    }
    case 'degraded':
      if (!isCapturing(s.phase) || s.degraded.includes(e.flag)) return null;
      return { ...s, degraded: [...s.degraded, e.flag] };
    case 'helper-lost':
      if (s.phase !== 'recording' && s.phase !== 'starting') {
        if (s.phase !== 'reconnecting') return null;
        return {
          ...s,
          reconnect: { ...s.reconnect!, attempt: s.reconnect!.attempt + 1 },
        };
      }
      return {
        ...s,
        phase: 'reconnecting',
        reconnect: { attempt: 1, since: e.at, deadlineAt: e.deadlineAt },
      };
    case 'helper-restored': {
      if (s.phase !== 'reconnecting') return null;
      const { reconnect: _r, ...rest } = s;
      return {
        ...rest,
        phase: 'recording',
        audioParts: [...s.audioParts, e.audioPath],
        // Clamp: a backward clock jump must not invent negative meeting-time offsets.
        partOffsetMs: Math.max(0, e.at - (s.startedAt ?? e.at)),
        degraded: s.degraded.includes('transcript-gap')
          ? s.degraded
          : [...s.degraded, 'transcript-gap'],
      };
    }
    case 'stop-requested': {
      if (!isCapturing(s.phase)) return null;
      const { reconnect: _r, ...rest } = s;
      return {
        ...rest,
        phase: 'stopping',
        stopReason: e.reason,
        stoppedAt: e.at,
      };
    }
    case 'stopped':
      if (s.phase !== 'stopping') return null;
      return { ...s, phase: 'processing' };
    case 'processing-done': {
      // From `error`: a manual retry of the failed step succeeded; the error is resolved.
      if (s.phase !== 'processing' && s.phase !== 'error') return null;
      const { error: _e, ...rest } = s;
      return { ...rest, phase: 'done' };
    }
    case 'processing-failed':
      if (s.phase !== 'processing') return null;
      return { ...s, phase: 'error', error: e.error };
    case 'rejected':
      // A start refused before anything was created (e.g. permissions): record why, keep the phase.
      if (!canStart(s.phase)) return null;
      return { ...s, error: e.error };
    case 'clear-error': {
      if (!s.error) return null;
      const { error: _e, ...rest } = s;
      return rest;
    }
    case 'reset':
      if (isCapturing(s.phase) || s.phase === 'stopping') return null;
      return initialRecordingState();
  }
}

export class LifecycleMachine {
  private s: RecordingState;
  private listeners = new Set<(s: RecordingState, e: LifecycleEvent) => void>();
  constructor(initial: RecordingState = initialRecordingState()) {
    this.s = initial;
  }
  get state(): RecordingState {
    return this.s;
  }
  send(e: LifecycleEvent): boolean {
    const next = lifecycleReducer(this.s, e);
    if (!next) return false;
    this.s = next;
    for (const l of this.listeners) l(next, e);
    return true;
  }
  onChange(cb: (s: RecordingState, e: LifecycleEvent) => void) {
    this.listeners.add(cb);
    return () => this.listeners.delete(cb);
  }
}

// ---------------------------------------------------------------- post-processing pipeline

export const StepId = z.enum(['finalize', 'summary', 'index']);
export type StepId = z.infer<typeof StepId>;
export const STEP_ORDER: readonly StepId[] = ['finalize', 'summary', 'index'];

export const StepStatus = z.enum([
  'pending',
  'running',
  'retrying',
  'done',
  'failed',
  'skipped',
]);
export type StepStatus = z.infer<typeof StepStatus>;

export const StepState = z.object({
  status: StepStatus,
  attempts: z.number(),
  maxAttempts: z.number(),
  nextRetryAt: z.number().optional(),
  error: LifecycleError.optional(),
  /** Summary only: an on-device extractive summary was saved in place of the LLM one. */
  fallback: z.boolean().optional(),
  startedAt: z.number().optional(),
  finishedAt: z.number().optional(),
});
export type StepState = z.infer<typeof StepState>;

/** Inputs finalize needs, persisted so a restart can resume post-processing exactly. */
export const PipelineContext = z.object({
  startedAt: z.number(),
  endedAt: z.number(),
  durationMs: z.number(),
  audioParts: z.array(z.string()),
  active: z.array(z.object({ atMs: z.number(), name: z.string() })),
  helperCrashed: z.boolean(),
});
export type PipelineContext = z.infer<typeof PipelineContext>;

export const PipelineState = z.object({
  meetingId: z.string(),
  stopReason: StopReason,
  steps: z.object({
    finalize: StepState,
    summary: StepState,
    index: StepState,
  }),
  ctx: PipelineContext,
  updatedAt: z.number(),
});
export type PipelineState = z.infer<typeof PipelineState>;
/** What the renderer gets (ctx stays in main/worker). */
export const PipelineView = PipelineState.omit({ ctx: true });
export type PipelineView = z.infer<typeof PipelineView>;

export interface RetryPolicy {
  maxAttempts: number;
  baseMs: number;
  maxMs: number;
}
export const RETRY_POLICIES = {
  /** Helper restarts while recording: supervisor backoff (500 ms × 2^n, cap 30 s); give up after this window. */
  helperReconnectWindowMs: 30_000,
  /** recording.started ack. */
  startAckMs: 5_000,
  /** recording.stopped ack (helper flushes recogniser + WAV). */
  stopAckMs: 10_000,
  /** N2: recording.aborted ack after a missed stop ack (a hard deadline; then the helper is restarted). */
  abortAckMs: 2_000,
  appendLive: { maxAttempts: 5, baseMs: 250, maxMs: 4_000 },
  finalize: { maxAttempts: 3, baseMs: 1_000, maxMs: 16_000 },
  summary: { maxAttempts: 3, baseMs: 2_000, maxMs: 30_000 },
  index: { maxAttempts: 3, baseMs: 5_000, maxMs: 60_000 },
} as const satisfies Record<string, RetryPolicy | number>;

/** Exponential backoff with ±20% jitter; `scale` shrinks every delay in tests (QRTZ_RETRY_SCALE). */
export function retryDelay(
  p: RetryPolicy,
  attempt: number,
  random: () => number = Math.random,
  scale = 1,
): number {
  const exp = Math.min(p.maxMs, p.baseMs * 2 ** Math.max(0, attempt - 1));
  return Math.round(exp * (0.8 + random() * 0.4) * scale);
}

/**
 * The longest delay `retryDelay` can draw for this attempt (jitter at +20%). A persisted `nextRetryAt` is wall
 * clock; the runtime never waits longer than this for it, so a clock that jumped back cannot stall a retry.
 */
export const maxRetryDelay = (p: RetryPolicy, attempt: number, scale = 1) =>
  retryDelay(p, attempt, () => 1, scale);

const step = (maxAttempts: number): StepState => ({
  status: 'pending',
  attempts: 0,
  maxAttempts,
});

export function newPipeline(
  meetingId: string,
  stopReason: StopReason,
  ctx: PipelineContext,
  now: number,
): PipelineState {
  return {
    meetingId,
    stopReason,
    ctx,
    updatedAt: now,
    steps: {
      finalize: step(RETRY_POLICIES.finalize.maxAttempts),
      summary: step(RETRY_POLICIES.summary.maxAttempts),
      index: step(RETRY_POLICIES.index.maxAttempts),
    },
  };
}

type StepPatch =
  | { kind: 'start'; now: number }
  | { kind: 'success'; now: number; fallback?: boolean }
  | { kind: 'skip'; now: number }
  | {
      kind: 'failure';
      now: number;
      error: LifecycleError;
      delayMs: number;
      /** Final failure regardless of attempts left (e.g. 401). */
      terminal?: boolean;
      fallback?: boolean;
    }
  | { kind: 'manual-retry'; now: number };

/** Pure step transition. Failure with attempts left → `retrying` + nextRetryAt; otherwise `failed`. */
export function pipelineStep(
  p: PipelineState,
  id: StepId,
  patch: StepPatch,
): PipelineState {
  const cur = p.steps[id];
  let next: StepState;
  switch (patch.kind) {
    case 'start':
      next = {
        ...cur,
        status: 'running',
        attempts: cur.attempts + 1,
        startedAt: patch.now,
      };
      delete next.nextRetryAt;
      break;
    case 'success':
      next = {
        status: 'done',
        attempts: cur.attempts,
        maxAttempts: cur.maxAttempts,
        finishedAt: patch.now,
        ...(cur.startedAt ? { startedAt: cur.startedAt } : {}),
      };
      break;
    case 'skip':
      next = {
        status: 'skipped',
        attempts: cur.attempts,
        maxAttempts: cur.maxAttempts,
        finishedAt: patch.now,
      };
      break;
    case 'failure': {
      const left =
        !patch.terminal &&
        patch.error.retryable &&
        cur.attempts < cur.maxAttempts;
      next = left
        ? {
            ...cur,
            status: 'retrying',
            error: patch.error,
            nextRetryAt: patch.now + patch.delayMs,
          }
        : {
            ...cur,
            status: 'failed',
            error: patch.error,
            finishedAt: patch.now,
            ...(patch.fallback ? { fallback: true } : {}),
          };
      if (!left) delete next.nextRetryAt;
      break;
    }
    case 'manual-retry':
      // A user retry grants a fresh budget for this step and re-arms every later step.
      next = step(cur.maxAttempts);
      break;
  }
  const steps = { ...p.steps, [id]: next };
  if (patch.kind === 'manual-retry') {
    for (const later of STEP_ORDER.slice(STEP_ORDER.indexOf(id) + 1))
      if (steps[later].status !== 'done' || id === 'finalize')
        steps[later] = step(steps[later].maxAttempts);
  }
  return { ...p, steps, updatedAt: patch.now };
}

/** Next step to run, or null when the pipeline is settled (done/failed/skipped everywhere it can be). */
export function nextRunnableStep(p: PipelineState): StepId | null {
  for (const id of STEP_ORDER) {
    const s = p.steps[id].status;
    if (s === 'done' || s === 'skipped') continue;
    if (s === 'failed') {
      // Summary failure is soft (fallback summary saved): indexing still runs. Finalize failure blocks the rest.
      if (id === 'summary') continue;
      return null;
    }
    return id;
  }
  return null;
}

export const pipelineSettled = (p: PipelineState) =>
  STEP_ORDER.every((id) =>
    ['done', 'failed', 'skipped'].includes(p.steps[id].status),
  ) || p.steps.finalize.status === 'failed';

/** The meeting is usable (transcript final + some summary) once finalize is done and summary settled. */
export const pipelineUsable = (p: PipelineState) =>
  p.steps.finalize.status === 'done' &&
  ['done', 'failed', 'skipped'].includes(p.steps.summary.status);

export const toPipelineView = (p: PipelineState): PipelineView => {
  const { ctx: _c, ...v } = p;
  return v;
};

/** Crash recovery R2: a step caught mid-run becomes `retrying` (idempotent re-run); attempts are kept. */
export function resumeAfterRestart(p: PipelineState): PipelineState {
  const steps = { ...p.steps };
  for (const id of STEP_ORDER) {
    if (steps[id].status === 'running')
      steps[id] = { ...steps[id], status: 'retrying' };
  }
  return { ...p, steps };
}

// ---------------------------------------------------------------- live transcript stream

export const LiveSegment = z.object({
  id: z.string(),
  text: z.string(),
  startMs: z.number(),
  endMs: z.number(),
  speaker: z.string().optional(),
});
export type LiveSegment = z.infer<typeof LiveSegment>;

/** Full live transcript of the meeting being recorded (snapshot; `rev` increments on every change). */
export const LiveTranscript = z.object({
  meetingId: z.string(),
  rev: z.number(),
  segments: z.array(LiveSegment),
  /** The volatile hypothesis being spoken now; replaced in place until it is finalized. */
  partial: z
    .object({ id: z.string(), text: z.string(), startMs: z.number() })
    .nullable(),
});
export type LiveTranscript = z.infer<typeof LiveTranscript>;

/** Incremental update: finals appended since `baseRev`, plus the current partial. Apply only if rev === baseRev+1. */
export const LiveTranscriptDelta = z.object({
  meetingId: z.string(),
  baseRev: z.number(),
  rev: z.number(),
  append: z.array(LiveSegment),
  partial: LiveTranscript.shape.partial,
});
export type LiveTranscriptDelta = z.infer<typeof LiveTranscriptDelta>;

/**
 * Renderer-side reducer for the stream. Returns null on a gap (missed delta, different meeting) so the caller
 * refetches the snapshot instead of guessing.
 */
export function applyTranscriptDelta(
  t: LiveTranscript | null,
  d: LiveTranscriptDelta,
): LiveTranscript | null {
  if (!t || t.meetingId !== d.meetingId || t.rev !== d.baseRev) return null;
  const seen = new Set(t.segments.map((s) => s.id));
  return {
    meetingId: t.meetingId,
    rev: d.rev,
    segments: [...t.segments, ...d.append.filter((s) => !seen.has(s.id))],
    partial: d.partial,
  };
}
