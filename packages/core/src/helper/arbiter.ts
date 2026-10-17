/**
 * Helper session arbiter: the child-protocol rules for concurrent recording.* and dictation.* on one helper, and
 * what a mid-capture TCC revocation does to each. Pure and deterministic. The Swift helper mirrors it
 * (native/qrtz-helper/Sources/QrtzHelperCore/Arbiter.swift) and both are tested against the same vectors
 * (spec/vectors/helper/arbiter.json). Spec: docs/helper-protocol.md, "Concurrency" and "Permission revocation".
 *
 * Decision: MUX. A meeting recording and a dictation run side by side. Both read the microphone through one
 * shared tap (a refcounted MicHub in the helper); the meeting additionally reads system audio. Each kind is
 * single-instance: a second recording.start or dictation.start while one runs is ignored (logged), not queued.
 */
import type { Degradation } from '../note/lifecycle';

export type HelperPermission =
  | 'microphone'
  | 'screen-recording'
  | 'accessibility';
export type MicSubscriber = 'recording' | 'dictation';

export interface HelperArbiterState {
  recording: boolean;
  dictating: boolean;
  /** Who currently reads the shared mic tap. The tap runs while this is non-empty. */
  mic: MicSubscriber[];
  /** Revoked/denied permissions. Anything not listed counts as usable (unknown is optimistic, as at start). */
  denied: HelperPermission[];
}

export type HelperArbiterEvent =
  | { cmd: 'recording.start' }
  | { cmd: 'recording.stop' }
  | { cmd: 'dictation.start' }
  | { cmd: 'dictation.stop' }
  | { cmd: 'dictation.cancel' }
  | { revoked: HelperPermission }
  | { granted: HelperPermission }
  | { helperLost: true };

/**
 * Side effects, as strings so vectors stay language-neutral:
 *   mic.subscribe:<who> / mic.unsubscribe:<who>   shared tap refcount (tap starts on first, stops on last)
 *   dictation.stop:<reason>                       helper ends the dictation itself (dictation.stopped{reason})
 *   degrade:<flag>                                host adds a recording degradation (lifecycle Degradation)
 */
export type HelperArbiterEffect = string;

export interface HelperArbiterResult {
  state: HelperArbiterState;
  decision: 'accept' | 'ignore' | 'refuse';
  reason?: string;
  /** True when the command left recording and dictation sharing the mic tap. */
  mux: boolean;
  effects: HelperArbiterEffect[];
}

export const initialArbiterState = (
  denied: HelperPermission[] = [],
): HelperArbiterState => ({
  recording: false,
  dictating: false,
  mic: [],
  denied,
});

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

const without = <T>(xs: T[], x: T) => xs.filter((y) => y !== x);

export function arbiterStep(
  s: HelperArbiterState,
  e: HelperArbiterEvent,
): HelperArbiterResult {
  const micOk = !s.denied.includes('microphone');
  const r = (
    state: HelperArbiterState,
    decision: HelperArbiterResult['decision'],
    effects: HelperArbiterEffect[] = [],
    reason?: string,
  ): HelperArbiterResult => ({
    state,
    decision,
    ...(reason ? { reason } : {}),
    mux: state.mic.includes('recording') && state.mic.includes('dictation'),
    effects,
  });

  if ('helperLost' in e) {
    const fx = s.dictating ? ['dictation.stop:helper-lost'] : [];
    return r(initialArbiterState(s.denied), 'accept', fx);
  }
  if ('granted' in e) {
    // Re-granting never clears a degradation within one recording (recording-lifecycle.md); it only lets the
    // next recording/dictation use the source again.
    return r({ ...s, denied: without(s.denied, e.granted) }, 'accept');
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
      const sub = micOk && !s.mic.includes('recording');
      return r(
        {
          ...s,
          recording: true,
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
        { ...s, recording: false, mic: without(s.mic, 'recording') },
        'accept',
        unsub ? ['mic.unsubscribe:recording'] : [],
      );
    }
    case 'dictation.start': {
      if (s.dictating) return r(s, 'ignore', [], 'already-dictating');
      if (!micOk) return r(s, 'refuse', [], 'microphone-denied');
      return r(
        {
          ...s,
          dictating: true,
          mic: [...without(s.mic, 'dictation'), 'dictation'],
        },
        'accept',
        ['mic.subscribe:dictation'],
      );
    }
    case 'dictation.stop':
    case 'dictation.cancel': {
      if (!s.dictating) return r(s, 'ignore', [], 'not-dictating');
      return r(
        { ...s, dictating: false, mic: without(s.mic, 'dictation') },
        'accept',
        ['mic.unsubscribe:dictation'],
      );
    }
  }
}
