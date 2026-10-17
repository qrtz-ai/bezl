import { z } from 'zod';
import { MeetingPlatform } from './types';
import { CaptureSource } from '../helper/protocol';

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

/**
 * N95 (ADR 0062): why `asr-unavailable` is set, when a speech session said (the Rust runtime only; the helper's own
 * `asr.unavailable` carries no reason).
 */
export const AsrUnavailableReason = z.enum([
  'not-installed',
  'revoked',
  'stale',
  'crashed',
  'start-failed',
  'timeout',
  'protocol-violations',
  'network',
  'cancelled',
]);
export type AsrUnavailableReason = z.infer<typeof AsrUnavailableReason>;

/**
 * N29: a capture source the helper reported silent (`capture.silent`), live and transient: `capture.audible`, a
 * helper loss or the stop end it. Kept apart from `degraded[]` (sticky permission/ASR flags). `sinceMs` is meeting
 * time (the helper's part-relative value plus `partOffsetMs`).
 */
export const SilentInput = z.object({
  source: CaptureSource,
  sinceMs: z.number(),
});
export type SilentInput = z.infer<typeof SilentInput>;

/**
 * N29: a closed silent stretch of one source, meeting time. `endMs` is missing only when the app died while the
 * source was silent (crash recovery: the stretch runs to the end of the recording).
 */
export const SilentSpan = z.object({
  source: CaptureSource,
  startMs: z.number(),
  endMs: z.number().optional(),
});
export type SilentSpan = z.infer<typeof SilentSpan>;

/** H8: one audio source that failed to start (`capture.fallback`), stamped in meeting time. */
export const CaptureFallback = z.object({
  from: z.string(),
  to: z.string().nullable(),
  reason: z.string(),
  atMs: z.number(),
});
export type CaptureFallback = z.infer<typeof CaptureFallback>;

/**
 * N35: a capture device that failed inside a running source (`capture.degraded`: the in-source mic of the tap or
 * SCK went busy, was unplugged, engine error). Only the flags it newly took away, stamped in meeting time. Never
 * written for a permission revoke, so the meeting can tell "the mic failed" from "mic access was revoked".
 */
export const DeviceFailure = z.object({
  degraded: z.array(Degradation),
  reason: z.string(),
  atMs: z.number(),
});
export type DeviceFailure = z.infer<typeof DeviceFailure>;

/**
 * N126 (ADR 0085): from `atMs` (meeting time) on, `source`'s speakers are not separated for the rest of that
 * recording part: the helper's diarizer stood down (`backlog`) or its model didn't load (`model-unavailable`). Lines
 * after it keep no speaker; this says why ("not separated", not "nobody spoke").
 */
export const SpeakersUnavailable = z.object({
  source: CaptureSource,
  atMs: z.number(),
  reason: z.enum(['backlog', 'model-unavailable']),
});
export type SpeakersUnavailable = z.infer<typeof SpeakersUnavailable>;

/** What a recording's capture looked like, carried from the session to its pipeline (N29, H8, N35). */
export const CaptureReport = z.object({
  silentSpans: z.array(SilentSpan).optional(),
  fallbacks: z.array(CaptureFallback).optional(),
  deviceFailures: z.array(DeviceFailure).optional(),
  /**
   * N39: stretches in which the helper did not deliver a source at all, because `degraded[]` took it away (a revoke,
   * a device failure, no source left). Same shape as a silent stretch: the summary treats a lost source like a
   * silent one, so a lost mic plus silent system audio is "no audio at all", not a one-sided gap.
   */
  lostSpans: z.array(SilentSpan).optional(),
  /** N126: channels whose speakers stopped being separated, in order. */
  speakersUnavailable: z.array(SpeakersUnavailable).optional(),
});
export type CaptureReport = z.infer<typeof CaptureReport>;

/** Bounds on the capture record: a flapping source or a helper restart loop can't grow the session row. */
export const CAPTURE_LIMITS = {
  silentSpans: 256,
  fallbacks: 8,
  /** N35: `capture.degraded` records kept per recording (each reconnect can fail the device again). */
  deviceFailures: 8,
  /** N39: stretches of a lost source kept per recording (each reconnect can lose and restore the mic again). */
  lostSpans: 16,
  /** N126: speaker stand-downs kept per recording (one per channel per part). */
  speakersUnavailable: 16,
  /** `capture.fallback.reason` / `capture.degraded.reason` are helper error text: kept to this many code points. */
  reasonChars: 200,
} as const;

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
  /**
   * H7 (ADR 0032): each part's `partOffsetMs`, parallel to `audioParts` (`[0, …]`), so finalize can put each
   * reconnect gap back as silence. Written by the first `helper-restored`, so it is omitted for a one-part
   * recording. Missing with several parts (a row from an older build): the parts are stitched back to back.
   */
  partOffsetsMs: z.array(z.number()).optional(),
  /** N29: sources silent right now (omitted when none). */
  silent: z.array(SilentInput).optional(),
  /** N29: closed silent stretches of this recording (omitted when none). */
  silentSpans: z.array(SilentSpan).optional(),
  /** H8: capture sources that failed to start during this recording (omitted when none). */
  fallbacks: z.array(CaptureFallback).optional(),
  /**
   * N35: the `degraded[]` flags held only because a capture device failed (`capture.degraded`), not a permission.
   * Sticky like the rest until a reconnect: `helper-restored` drops them from `degraded[]` (the new helper starts
   * from its own state). A later revoke of the same flag takes it over (it then stays). Omitted when none.
   */
  deviceLost: z.array(Degradation).optional(),
  /** N35: device failures of this recording, for the meeting record (omitted when none). */
  deviceFailures: z.array(DeviceFailure).optional(),
  /**
   * N39: stretches in which a source was lost (see `CaptureReport.lostSpans`). One without `endMs` is still lost; it
   * ends at the reconnect that brings the source back (a device failure), or at the stop.
   */
  lostSpans: z.array(SilentSpan).optional(),
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
  /** `at` (wall clock) closes a silent stretch of a source the flag takes away (N29). */
  | { type: 'degraded'; flag: Degradation; at?: number }
  /** N29 `capture.silent`: `sinceMs` is helper (part-relative) time. */
  | { type: 'input-silent'; source: CaptureSource; sinceMs: number }
  /** N29 `capture.audible`, stamped by the host (the message carries no time). */
  | { type: 'input-audible'; source: CaptureSource; at: number }
  /** N35 `capture.degraded`, stamped by the host. */
  | {
      type: 'capture-degraded';
      degraded: Degradation[];
      reason: string;
      at: number;
    }
  /** H8 `capture.fallback`, stamped by the host. */
  | {
      type: 'capture-fallback';
      from: string;
      to: string | null;
      reason: string;
      at: number;
    }
  /** N42 (ADR 0046 §3b): the current part's speech session reached ready. */
  | { type: 'speech-ready' }
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

/** Sources a degradation takes away: their silence means nothing any more (N29). */
const SOURCES_LOST: Partial<Record<Degradation, readonly CaptureSource[]>> = {
  'mic-only': ['system'],
  'no-mic': ['mic'],
  'audio-unavailable': ['system', 'mic'],
};

/** True when `degraded[]` says the helper no longer delivers `source` (so it never reports it silent). */
export const sourceLost = (
  degraded: readonly Degradation[],
  src: CaptureSource,
) => degraded.some((d) => SOURCES_LOST[d]?.includes(src));

/** Meeting-time ms of a wall-clock instant, never before `floor` (a clock that jumped back). */
const meetingMs = (s: RecordingState, at: number, floor: number) =>
  Math.max(floor, at - (s.startedAt ?? at));

/** Sets or drops an optional array field: empty arrays are omitted (the persisted/IPC form stays as before). */
function withList<
  K extends
    | 'silent'
    | 'silentSpans'
    | 'fallbacks'
    | 'deviceLost'
    | 'deviceFailures'
    | 'lostSpans',
>(s: RecordingState, k: K, v: NonNullable<RecordingState[K]>): RecordingState {
  const { [k]: _old, ...rest } = s;
  return (v.length ? { ...rest, [k]: v } : rest) as RecordingState;
}

/**
 * Closes the silent stretches of `sources` (all when omitted) at wall clock `at`; they move from `silent` to
 * `silentSpans` (capped). `at` undefined: zero-length (the end is unknown and the source is gone).
 */
function closeSilent(
  s: RecordingState,
  at: number | undefined,
  sources?: readonly CaptureSource[],
): RecordingState {
  const open = s.silent ?? [];
  const closing = open.filter((x) => !sources || sources.includes(x.source));
  if (!closing.length) return s;
  const spans = [...(s.silentSpans ?? [])];
  for (const x of closing)
    if (spans.length < CAPTURE_LIMITS.silentSpans)
      spans.push({
        source: x.source,
        startMs: x.sinceMs,
        endMs: at === undefined ? x.sinceMs : meetingMs(s, at, x.sinceMs),
      });
  return withList(
    withList(
      s,
      'silent',
      open.filter((x) => !closing.includes(x)),
    ),
    'silentSpans',
    spans,
  );
}

/**
 * N39: opens a lost stretch for each of `sources` that has none open, at wall clock `at` (meeting time; capped, so
 * past the cap a loss is not recorded and the summary falls back to the silent stretches alone).
 */
function openLost(
  s: RecordingState,
  at: number,
  sources: readonly CaptureSource[],
): RecordingState {
  const spans = [...(s.lostSpans ?? [])];
  for (const src of sources)
    if (
      !spans.some((x) => x.source === src && x.endMs === undefined) &&
      spans.length < CAPTURE_LIMITS.lostSpans
    )
      spans.push({ source: src, startMs: meetingMs(s, at, 0) });
  return spans.length === (s.lostSpans ?? []).length
    ? s
    : withList(s, 'lostSpans', spans);
}

/** N39: ends, at wall clock `at`, the open lost stretches of the sources `degraded[]` no longer takes away (all when `all`). */
function closeLost(s: RecordingState, at: number, all = false): RecordingState {
  const spans = s.lostSpans ?? [];
  if (!spans.some((x) => x.endMs === undefined)) return s;
  return withList(
    s,
    'lostSpans',
    spans.map((x) =>
      x.endMs === undefined && (all || !sourceLost(s.degraded, x.source))
        ? { ...x, endMs: meetingMs(s, at, x.startMs) }
        : x,
    ),
  );
}

/**
 * N42 (ADR 0034): flags a helper reports at each recording start, before `recording.started`
 * (`error{asr.unavailable}`, and `capture.fallback{to: null}` + `error{audio.unavailable}`). They describe that
 * helper process, so they die with it: the next helper reports its own while the session is `reconnecting`.
 */
const HELPER_SCOPED: readonly Degradation[] = [
  'asr-unavailable',
  'audio-unavailable',
];

/** N42: drops the helper-scoped flags at a helper loss (wall clock `at`); a source they took away is no longer lost. */
function dropHelperScoped(s: RecordingState, at: number): RecordingState {
  if (!s.degraded.some((f) => HELPER_SCOPED.includes(f))) return s;
  const keep = (f: Degradation) => !HELPER_SCOPED.includes(f);
  return closeLost(
    withList(
      { ...s, degraded: s.degraded.filter(keep) },
      'deviceLost',
      (s.deviceLost ?? []).filter(keep),
    ),
    at,
  );
}

/** Code-point cut (same in Rust: `chars().take(n)`), so a cut never lands inside a surrogate pair. */
export const cutChars = (v: string, n: number) =>
  Array.from(v).slice(0, n).join('');

/**
 * N29/H8: the capture record a stopped recording hands to its pipeline. Silent stretches still open (the app died
 * mid-stretch: crash recovery) are kept without an end. `undefined` when there is nothing to report.
 */
export function captureReport(s: RecordingState): CaptureReport | undefined {
  const spans: SilentSpan[] = [
    ...(s.silentSpans ?? []),
    ...(s.silent ?? []).map((x) => ({ source: x.source, startMs: x.sinceMs })),
  ].slice(0, CAPTURE_LIMITS.silentSpans);
  const fallbacks = s.fallbacks ?? [];
  const deviceFailures = s.deviceFailures ?? [];
  const lostSpans = s.lostSpans ?? [];
  if (
    !spans.length &&
    !fallbacks.length &&
    !deviceFailures.length &&
    !lostSpans.length
  )
    return undefined;
  return {
    ...(spans.length ? { silentSpans: spans } : {}),
    ...(fallbacks.length ? { fallbacks } : {}),
    ...(deviceFailures.length ? { deviceFailures } : {}),
    ...(lostSpans.length ? { lostSpans } : {}),
  };
}

/**
 * N29: what the live view says about silent input. `noAudio`: every source the helper still delivers is silent
 * ("no audio at all"). A source counts as delivered unless `degraded[]` took it away or its permission is denied
 * (a recording started without the mic carries no flag for it).
 */
export function silentInputView(
  s: RecordingState,
  denied: { microphone?: boolean; screenRecording?: boolean } = {},
): { silent?: SilentInput[]; noAudio?: true } {
  const silent = isCapturing(s.phase) ? (s.silent ?? []) : [];
  if (!silent.length) return {};
  const delivered = (['system', 'mic'] as const).filter(
    (src) =>
      !sourceLost(s.degraded, src) &&
      !(src === 'mic' ? denied.microphone : denied.screenRecording),
  );
  const all =
    delivered.length > 0 &&
    delivered.every((src) => silent.some((x) => x.source === src));
  return { silent, ...(all ? { noAudio: true as const } : {}) };
}

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
    case 'degraded': {
      if (!isCapturing(s.phase)) return null;
      if (s.degraded.includes(e.flag)) {
        // N35: a revoke of a flag a device failure already set: the permission owns it now, so it outlives a
        // reconnect. Anything else already set is a duplicate.
        if (!s.deviceLost?.includes(e.flag)) return null;
        return withList(
          s,
          'deviceLost',
          s.deviceLost.filter((f) => f !== e.flag),
        );
      }
      // A source this flag takes away is no longer tracked by the helper: close its silent stretch now (N29).
      // N39: and it is lost from now on (the summary must not read its absence as the other side being quiet).
      const lost = SOURCES_LOST[e.flag];
      const n = { ...s, degraded: [...s.degraded, e.flag] };
      // An event without a time (only older callers) records no lost stretch: it can't be placed.
      if (!lost) return n;
      const closed = closeSilent(n, e.at, lost);
      return e.at === undefined ? closed : openLost(closed, e.at, lost);
    }
    case 'input-silent': {
      // Only while the helper is capturing; a duplicate (already silent) is idempotent; a source the helper no
      // longer delivers (no-mic, mic-only, audio-unavailable) can't be silent.
      if (s.phase !== 'starting' && s.phase !== 'recording') return null;
      if ((s.silent ?? []).some((x) => x.source === e.source)) return null;
      if (sourceLost(s.degraded, e.source)) return null;
      return withList(s, 'silent', [
        ...(s.silent ?? []),
        {
          source: e.source,
          sinceMs: Math.max(0, Math.round(e.sinceMs) + s.partOffsetMs),
        },
      ]);
    }
    case 'input-audible':
      // Audible without a prior silent is ignored.
      if (s.phase !== 'starting' && s.phase !== 'recording') return null;
      if (!(s.silent ?? []).some((x) => x.source === e.source)) return null;
      return closeSilent(s, e.at, [e.source]);
    case 'capture-degraded': {
      // N35: applied like a revoke (N4), but remembered as a device failure. Only the flags not already set count:
      // a duplicate, or a device failure after a revoke of the same source, changes nothing. Only while recording:
      // the helper sends it after recording.started (a source that then fails to start is capture.fallback), so one
      // that arrives before the ack (starting) or while reconnecting is a stray line and is ignored.
      if (s.phase !== 'recording') return null;
      const fresh = e.degraded.filter(
        (f, i) => !s.degraded.includes(f) && e.degraded.indexOf(f) === i,
      );
      if (!fresh.length) return null;
      const failures = s.deviceFailures ?? [];
      let n = withList(
        { ...s, degraded: [...s.degraded, ...fresh] },
        'deviceLost',
        [...(s.deviceLost ?? []), ...fresh],
      );
      if (failures.length < CAPTURE_LIMITS.deviceFailures)
        n = withList(n, 'deviceFailures', [
          ...failures,
          {
            degraded: fresh,
            reason: cutChars(e.reason, CAPTURE_LIMITS.reasonChars),
            atMs: meetingMs(s, e.at, 0),
          },
        ]);
      // A source it takes away is no longer tracked: its silent stretch ends now; later silence of it is ignored.
      const lost = [...new Set(fresh.flatMap((f) => SOURCES_LOST[f] ?? []))];
      return lost.length ? openLost(closeSilent(n, e.at, lost), e.at, lost) : n;
    }
    case 'capture-fallback': {
      if (!isCapturing(s.phase)) return null;
      const list = s.fallbacks ?? [];
      let n =
        list.length < CAPTURE_LIMITS.fallbacks
          ? withList(s, 'fallbacks', [
              ...list,
              {
                from: cutChars(e.from, CAPTURE_LIMITS.reasonChars),
                to:
                  e.to === null
                    ? null
                    : cutChars(e.to, CAPTURE_LIMITS.reasonChars),
                reason: cutChars(e.reason, CAPTURE_LIMITS.reasonChars),
                atMs: meetingMs(s, e.at, 0),
              },
            ])
          : s;
      // No source left: nothing is captured (`error{audio.unavailable}` follows and is then a no-op).
      if (e.to === null && !n.degraded.includes('audio-unavailable'))
        n = openLost(
          closeSilent(
            { ...n, degraded: [...n.degraded, 'audio-unavailable'] },
            e.at,
          ),
          e.at,
          ['system', 'mic'],
        );
      return n === s ? null : n;
    }
    case 'helper-lost':
      if (s.phase !== 'recording' && s.phase !== 'starting') {
        if (s.phase !== 'reconnecting') return null;
        // N42: a helper that died while reconnecting takes its own start reports with it, too.
        return {
          ...dropHelperScoped(s, e.at),
          reconnect: { ...s.reconnect!, attempt: s.reconnect!.attempt + 1 },
        };
      }
      // The helper's silence trackers die with it: close every silent stretch at the loss (N29). The new helper
      // reports afresh; the restart interval itself is the `transcript-gap`. N42: so do its start reports.
      return {
        ...dropHelperScoped(closeSilent(s, e.at), e.at),
        phase: 'reconnecting',
        reconnect: { attempt: 1, since: e.at, deadlineAt: e.deadlineAt },
      };
    case 'helper-restored': {
      if (s.phase !== 'reconnecting') return null;
      // N35: a device failure lasts until the reconnect: the new helper starts from its own state.
      const { reconnect: _r, deviceLost: lost = [], ...rest } = s;
      const kept = s.degraded.filter((f) => !lost.includes(f));
      // Clamp: a backward clock jump must not invent negative meeting-time offsets.
      // N45 (ADR 0037): the runtime stamps `e.at` from a monotonic clock for helper-lost/restored, so a wall jump
      // during a reconnect does not slide this offset; H2 still clamps a hostile or legacy stamp.
      const partOffsetMs = Math.max(0, e.at - (s.startedAt ?? e.at));
      // H7: the offsets are kept only while they line up with the parts (a row from an older build has none).
      const prev =
        s.partOffsetsMs?.length === s.audioParts.length
          ? s.partOffsetsMs
          : s.audioParts.length === 1 && !s.partOffsetsMs
            ? [0]
            : undefined;
      const { partOffsetsMs: _o, ...base } = rest;
      // N39: a source the reconnect brings back is no longer lost (its lost stretch ends at the restore).
      return closeLost(
        {
          ...base,
          phase: 'recording',
          audioParts: [...s.audioParts, e.audioPath],
          partOffsetMs,
          ...(prev ? { partOffsetsMs: [...prev, partOffsetMs] } : {}),
          degraded: kept.includes('transcript-gap')
            ? kept
            : [...kept, 'transcript-gap'],
        },
        e.at,
      );
    }
    case 'stop-requested': {
      if (!isCapturing(s.phase)) return null;
      // A source still silent at the stop: its stretch ends at the stop (N29).
      // N39: and a source still lost is lost until the stop.
      const { reconnect: _r, ...rest } = closeLost(
        closeSilent(s, e.at),
        e.at,
        true,
      );
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
    case 'speech-ready': {
      // N42 (ADR 0046 §3b): speech-unavailable is scoped to the speech session that reported it.
      if (s.phase !== 'recording' || !s.degraded.includes('asr-unavailable'))
        return null;
      const keep = (f: Degradation) => f !== 'asr-unavailable';
      return withList(
        { ...s, degraded: s.degraded.filter(keep) },
        'deviceLost',
        (s.deviceLost ?? []).filter(keep),
      );
    }
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
  /** H7: meeting-time start of each part (parallel to `audioParts`); finalize pads the gaps. Omitted: no padding. */
  partOffsetsMs: z.array(z.number()).optional(),
  active: z.array(z.object({ atMs: z.number(), name: z.string() })),
  helperCrashed: z.boolean(),
  /**
   * Diarized turns in meeting time with per-channel speaker ids (`mic:0`, `system:1`; ADR 0058 §12). Written by the
   * Rust runtime only (Electron does not diarize); omitted when empty.
   */
  speakerTurns: z
    .array(
      z.object({
        speaker: z.string().regex(/^(mic|system):[0-7]$/),
        startMs: z.number(),
        endMs: z.number(),
      }),
    )
    .optional(),
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
  /** N29/H8: silent stretches and capture fallbacks of the recording (omitted when there were none). */
  capture: CaptureReport.optional(),
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
