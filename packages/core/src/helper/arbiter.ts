/**
 * Helper session arbiter: the child-protocol rules for concurrent recording.* and dictation.* on one helper, the
 * warm mic, and what a mid-capture TCC revocation does to each. Pure and deterministic. The Swift helper mirrors
 * it (native/qrtz-helper/Sources/QrtzShared/Arbiter.swift) and both are tested against the same vectors
 * (spec/vectors/helper/arbiter.json, arbiter-edges.json). Spec: docs/helper-protocol.md.
 *
 * Decision: MUX. A meeting recording and a dictation run side by side. Both read the microphone through one
 * shared tap (a refcounted MicHub in the helper); the meeting additionally reads system audio. Each kind is
 * single-instance: a second recording.start or dictation.start while one runs is ignored and answered with
 * error{code: recording.busy | dictation.busy}, never queued.
 *
 * The arbiter is ENFORCING: the helper performs exactly the mic.subscribe / mic.unsubscribe effects it returns
 * (on commands, configure, revoke and re-grant), so its `mic` list is the hub's reader list.
 *
 * Invariant (asserted after every step): a running recording always has a file, `recording` iff
 * `activeAudioPath !== null`. A recording.start without an audioPath is refused, and when the helper cannot create
 * the file (`recordingFileFailed`) the recording is torn down at once. Busy errors and path-scoped stops can then
 * always name the recording that owns the helper; there is no pathless state to stop.
 */
import type { Degradation } from '../note/lifecycle';

export type HelperPermission =
  | 'microphone'
  | 'screen-recording'
  | 'accessibility';
export type MicSubscriber = 'recording' | 'dictation';

export interface HelperArbiterState {
  recording: boolean;
  /** The file the running recording writes; null iff not recording (invariant, see assertArbiterInvariant). */
  activeAudioPath: string | null;
  dictating: boolean;
  /**
   * Warm mic (dictation.configure warmMic): the dictation reader stays subscribed between dictations so the
   * pre-roll fills. Survives a mic revoke; the reader comes back when the mic is re-granted.
   */
  warm: boolean;
  /** Who currently reads the shared mic tap. The tap runs while this is non-empty. */
  mic: MicSubscriber[];
  /** Revoked/denied permissions. Anything not listed counts as usable (unknown is optimistic, as at start). */
  denied: HelperPermission[];
}

export type HelperArbiterEvent =
  | { cmd: 'recording.start'; audioPath?: string }
  | { cmd: 'recording.stop' }
  /** N2: tear down the recording writing `audioPath` without a flush (scoped; dictation untouched). */
  | { cmd: 'recording.abort'; audioPath: string }
  | { cmd: 'dictation.start' }
  | { cmd: 'dictation.stop' }
  | { cmd: 'dictation.cancel' }
  | { configure: { warmMic: boolean } }
  | { revoked: HelperPermission }
  | { granted: HelperPermission }
  | { helperLost: true }
  /**
   * The helper could not create (open) the file of the recording it just admitted. It fails closed: the recording
   * is torn down (mic and taps released, `recording.lost`), the helper answers
   * error{code: recording.file, audioPath, fatal: false} and the arbiter is idle for recordings again.
   */
  | { recordingFileFailed: { audioPath: string } };

/**
 * Side effects, as strings so vectors stay language-neutral:
 *   mic.subscribe:<who> / mic.unsubscribe:<who>   shared tap refcount (tap starts on first, stops on last)
 *   dictation.stop:<reason>                       the dictation ends without a host stop (dictation.stopped{reason})
 *   recording.lost                                the recording ended without a host stop: with the helper
 *                                                 (helperLost; host: reconnect path) or because its file could
 *                                                 not be created (recordingFileFailed; host: the start fails)
 *   degrade:<flag>                                host adds a recording degradation (lifecycle Degradation)
 */
export type HelperArbiterEffect = string;

export interface HelperArbiterResult {
  state: HelperArbiterState;
  decision: 'accept' | 'ignore' | 'refuse';
  reason?: string;
  /** True when a recording and a live dictation both read the mic tap after this step (N5: not a warm idle mic). */
  mux: boolean;
  effects: HelperArbiterEffect[];
}

export const initialArbiterState = (
  denied: HelperPermission[] = [],
): HelperArbiterState => ({
  recording: false,
  activeAudioPath: null,
  dictating: false,
  warm: false,
  mic: [],
  denied,
});

/** Why `s` breaks the arbiter invariant, or null. A running recording must have a file (and only then). */
export function arbiterInvariantViolation(
  s: HelperArbiterState,
): string | null {
  if (s.recording && !s.activeAudioPath)
    return 'recording without an activeAudioPath';
  if (!s.recording && s.activeAudioPath !== null)
    return 'activeAudioPath set while not recording';
  return null;
}

/** Throws when `s` breaks the invariant (a bug: the helper must never run a recording without a file). */
export function assertArbiterInvariant(s: HelperArbiterState): void {
  const v = arbiterInvariantViolation(s);
  if (v) throw new Error(`helper arbiter invariant broken: ${v}`);
}

/** Recording degradations caused by losing `p`, given what was already denied. */
export function degradationsForRevocation(
  p: HelperPermission,
  alreadyDenied: readonly HelperPermission[],
): Degradation[] {
  const has = (x: HelperPermission) => !alreadyDenied.includes(x) && x !== p;
  if (p === 'accessibility') return ['ax-unavailable'];
  if (p === 'microphone')
    return [has('screen-recording') ? 'no-mic' : 'audio-unavailable'];
  return [has('microphone') ? 'mic-only' : 'audio-unavailable'];
}

/** N4: the `degrade:*` effects of a revocation step, as `permission.revoked.degraded` carries them. */
export function revocationDegradations(
  effects: readonly HelperArbiterEffect[],
): Degradation[] {
  return effects
    .filter((f) => f.startsWith('degrade:'))
    .map((f) => f.slice('degrade:'.length) as Degradation);
}

const without = <T>(xs: T[], x: T) => xs.filter((y) => y !== x);

export function arbiterStep(
  s: HelperArbiterState,
  e: HelperArbiterEvent,
): HelperArbiterResult {
  const out = step(s, e);
  assertArbiterInvariant(out.state);
  return out;
}

function step(
  s: HelperArbiterState,
  e: HelperArbiterEvent,
): HelperArbiterResult {
  const micOk = !s.denied.includes('microphone');
  const hasDict = s.mic.includes('dictation');
  const r = (
    state: HelperArbiterState,
    decision: HelperArbiterResult['decision'],
    effects: HelperArbiterEffect[] = [],
    reason?: string,
  ): HelperArbiterResult => ({
    state,
    decision,
    ...(reason ? { reason } : {}),
    // N5: two live readers. A warm idle mic keeps the `dictation` reader without dictating; that is not a mux.
    mux: state.mic.includes('recording') && state.dictating,
    effects,
  });

  if ('helperLost' in e) {
    // The process (and its hub) is gone: release every reader explicitly so replayed refcounts balance.
    const fx: HelperArbiterEffect[] = [];
    if (s.dictating) fx.push('dictation.stop:helper-lost');
    if (s.recording) fx.push('recording.lost');
    for (const who of s.mic) fx.push(`mic.unsubscribe:${who}`);
    return r(initialArbiterState(s.denied), 'accept', fx);
  }
  if ('recordingFileFailed' in e) {
    // Fail closed: a recording whose file could not be created must not keep running (or holding the mic).
    if (!s.recording) return r(s, 'ignore', [], 'not-recording');
    if (s.activeAudioPath !== e.recordingFileFailed.audioPath)
      return r(s, 'ignore', [], 'other-recording');
    const unsub = s.mic.includes('recording');
    return r(
      {
        ...s,
        recording: false,
        activeAudioPath: null,
        mic: without(s.mic, 'recording'),
      },
      'accept',
      ['recording.lost', ...(unsub ? ['mic.unsubscribe:recording'] : [])],
    );
  }
  if ('configure' in e) {
    const warm = e.configure.warmMic;
    if (warm && micOk && !hasDict)
      return r({ ...s, warm, mic: [...s.mic, 'dictation'] }, 'accept', [
        'mic.subscribe:dictation',
      ]);
    if (!warm && hasDict && !s.dictating)
      return r({ ...s, warm, mic: without(s.mic, 'dictation') }, 'accept', [
        'mic.unsubscribe:dictation',
      ]);
    return r({ ...s, warm }, 'accept');
  }
  if ('granted' in e) {
    // Re-granting never clears a degradation within one recording (recording-lifecycle.md) and never
    // resubscribes a running recording; it lets the next session use the source and revives a warm mic.
    const next = { ...s, denied: without(s.denied, e.granted) };
    if (e.granted === 'microphone' && !micOk && s.warm && !hasDict)
      return r({ ...next, mic: [...next.mic, 'dictation'] }, 'accept', [
        'mic.subscribe:dictation',
      ]);
    return r(next, 'accept');
  }
  if ('revoked' in e) {
    const p = e.revoked;
    if (s.denied.includes(p)) return r(s, 'ignore', [], 'unchanged');
    const fx: HelperArbiterEffect[] = [];
    let next: HelperArbiterState = { ...s, denied: [...s.denied, p] };
    if (p === 'microphone') {
      if (s.dictating) {
        fx.push('dictation.stop:permission-revoked');
        next = { ...next, dictating: false };
      }
      for (const who of s.mic) fx.push(`mic.unsubscribe:${who}`);
      next = { ...next, mic: [] };
    }
    if (s.recording)
      for (const f of degradationsForRevocation(p, s.denied))
        fx.push(`degrade:${f}`);
    return r(next, 'accept', fx);
  }

  switch (e.cmd) {
    case 'recording.start': {
      if (s.recording) return r(s, 'ignore', [], 'already-recording');
      // Never admit a recording that has no file to name it by (busy, stop and started all carry the path).
      if (!e.audioPath) return r(s, 'refuse', [], 'no-audio-path');
      const sub = micOk && !s.mic.includes('recording');
      return r(
        {
          ...s,
          recording: true,
          activeAudioPath: e.audioPath,
          mic: sub ? [...s.mic, 'recording'] : s.mic,
        },
        'accept',
        sub ? ['mic.subscribe:recording'] : [],
      );
    }
    case 'recording.stop': {
      if (!s.recording) return r(s, 'ignore', [], 'not-recording');
      const unsub = s.mic.includes('recording');
      return r(
        {
          ...s,
          recording: false,
          activeAudioPath: null,
          mic: without(s.mic, 'recording'),
        },
        'accept',
        unsub ? ['mic.unsubscribe:recording'] : [],
      );
    }
    case 'recording.abort': {
      // Scoped by file: an abort for another recording (or after this one ended) changes nothing.
      if (!s.recording) return r(s, 'ignore', [], 'not-recording');
      if (s.activeAudioPath !== e.audioPath)
        return r(s, 'ignore', [], 'other-recording');
      const unsub = s.mic.includes('recording');
      return r(
        {
          ...s,
          recording: false,
          activeAudioPath: null,
          mic: without(s.mic, 'recording'),
        },
        'accept',
        unsub ? ['mic.unsubscribe:recording'] : [],
      );
    }
    case 'dictation.start': {
      if (s.dictating) return r(s, 'ignore', [], 'already-dictating');
      if (!micOk) return r(s, 'refuse', [], 'microphone-denied');
      // A warm reader is already subscribed: the dictation reuses it (the pre-roll comes from it).
      return r(
        {
          ...s,
          dictating: true,
          mic: hasDict ? s.mic : [...s.mic, 'dictation'],
        },
        'accept',
        hasDict ? [] : ['mic.subscribe:dictation'],
      );
    }
    case 'dictation.stop':
    case 'dictation.cancel': {
      if (!s.dictating) return r(s, 'ignore', [], 'not-dictating');
      // Warm mic keeps the reader (and the device) open for the next pre-roll.
      if (s.warm && hasDict) return r({ ...s, dictating: false }, 'accept');
      return r(
        { ...s, dictating: false, mic: without(s.mic, 'dictation') },
        'accept',
        hasDict ? ['mic.unsubscribe:dictation'] : [],
      );
    }
  }
}
