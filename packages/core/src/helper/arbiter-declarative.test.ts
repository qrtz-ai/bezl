/**
 * Second, declarative reference model for the helper arbiter (reviewer-owned, written from docs/helper-protocol.md
 * "Concurrency", "The arbiter is enforcing", "Warm mic" and ADR 0004 only; it never calls arbiterStep to decide what
 * is expected). Instead of a transition function it states WHO MUST READ THE MIC after any event:
 *
 *   recording reader  <=> a recording runs AND it started with the mic usable AND no mic revoke since
 *                         (a re-grant never resubscribes a running recording)
 *   dictation reader  <=> mic usable AND (a dictation runs OR warm mic is on)
 *
 * and requires the arbiter's mic effects to be exactly the diff of that set (subscribe for each reader gained,
 * unsubscribe for each reader lost; helperLost releases every reader). Checked exhaustively over every reachable
 * state x every event (all sequence lengths, warm/configure included) and over seeded random length-10 replays.
 *
 * Plus the file rule (helper-protocol.md "A recording never runs without a file"): a recording runs only with the
 * file it was started with; a start with no file is refused, and a failure to create that file ends the recording
 * (recording.lost) exactly as if it had stopped.
 */
import { describe, expect, it } from 'vitest';
import {
  arbiterStep,
  initialArbiterState,
  type HelperArbiterEvent,
  type HelperArbiterState,
  type HelperPermission,
} from './arbiter';

const PERMS: HelperPermission[] = [
  'microphone',
  'screen-recording',
  'accessibility',
];
const EVENTS: HelperArbiterEvent[] = [
  { cmd: 'recording.start', audioPath: 'a.wav' },
  { cmd: 'recording.start', audioPath: 'b.wav' },
  { cmd: 'recording.start' },
  { recordingFileFailed: { audioPath: 'a.wav' } },
  { recordingFileFailed: { audioPath: 'b.wav' } },
  { cmd: 'recording.stop' },
  { cmd: 'dictation.start' },
  { cmd: 'dictation.stop' },
  { cmd: 'dictation.cancel' },
  { configure: { warmMic: true } },
  { configure: { warmMic: false } },
  ...PERMS.map((p) => ({ revoked: p })),
  ...PERMS.map((p) => ({ granted: p })),
  { helperLost: true as const },
];

interface Spec {
  recording: boolean;
  file: string | null; // the running recording's file; null iff not recording
  recMicEligible: boolean; // started with mic usable, no mic revoke since
  dictating: boolean;
  warm: boolean;
  micDenied: boolean;
}

const readers = (s: Spec) => {
  const out = new Set<string>();
  if (s.recording && s.recMicEligible) out.add('recording');
  if (!s.micDenied && (s.dictating || s.warm)) out.add('dictation');
  return out;
};

/** Session facts from the spec; mic readers are derived, never stepped. */
function specStep(
  s: Spec,
  e: HelperArbiterEvent,
): { s: Spec; decision: string; extra: string[] } {
  if ('helperLost' in e) {
    // "every session ends"; warm is host config, re-sent on helper hello.
    const extra = [
      ...(s.dictating ? ['dictation.stop:helper-lost'] : []),
      ...(s.recording ? ['recording.lost'] : []),
    ];
    return {
      s: {
        recording: false,
        file: null,
        recMicEligible: false,
        dictating: false,
        warm: false,
        micDenied: s.micDenied,
      },
      decision: 'accept',
      extra,
    };
  }
  if ('recordingFileFailed' in e)
    return !s.recording || s.file !== e.recordingFileFailed.audioPath
      ? { s, decision: 'ignore', extra: [] }
      : {
          s: { ...s, recording: false, file: null, recMicEligible: false },
          decision: 'accept',
          extra: ['recording.lost'],
        };
  if ('configure' in e)
    return {
      s: { ...s, warm: e.configure.warmMic },
      decision: 'accept',
      extra: [],
    };
  if ('granted' in e)
    return {
      s: e.granted === 'microphone' ? { ...s, micDenied: false } : s,
      decision: 'accept',
      extra: [],
    };
  if ('revoked' in e) return { s, decision: 'revoke', extra: [] }; // handled by caller (needs denied set)
  switch (e.cmd) {
    case 'recording.start':
      if (s.recording) return { s, decision: 'ignore', extra: [] };
      return !e.audioPath
        ? { s, decision: 'refuse', extra: [] }
        : {
            s: {
              ...s,
              recording: true,
              file: e.audioPath,
              recMicEligible: !s.micDenied,
            },
            decision: 'accept',
            extra: [],
          };
    case 'recording.stop':
      return !s.recording
        ? { s, decision: 'ignore', extra: [] }
        : {
            s: { ...s, recording: false, file: null, recMicEligible: false },
            decision: 'accept',
            extra: [],
          };
    case 'dictation.start':
      if (s.dictating) return { s, decision: 'ignore', extra: [] };
      if (s.micDenied) return { s, decision: 'refuse', extra: [] };
      return { s: { ...s, dictating: true }, decision: 'accept', extra: [] };
    default:
      return !s.dictating
        ? { s, decision: 'ignore', extra: [] }
        : { s: { ...s, dictating: false }, decision: 'accept', extra: [] };
  }
}

function degrade(p: HelperPermission, denied: Set<HelperPermission>) {
  if (p === 'accessibility') return 'ax-unavailable';
  const other = p === 'microphone' ? 'screen-recording' : 'microphone';
  if (denied.has(other)) return 'audio-unavailable';
  return p === 'microphone' ? 'no-mic' : 'mic-only';
}

function check(
  a: HelperArbiterState,
  s: Spec,
  denied: Set<HelperPermission>,
  e: HelperArbiterEvent,
  ctx: string,
) {
  const r = arbiterStep(a, e);
  let next: Spec;
  let decision: string;
  let extra: string[] = [];
  const nd = new Set(denied);
  if ('revoked' in e) {
    if (denied.has(e.revoked)) {
      next = s;
      decision = 'ignore';
    } else {
      nd.add(e.revoked);
      decision = 'accept';
      next =
        e.revoked === 'microphone'
          ? { ...s, micDenied: true, dictating: false, recMicEligible: false }
          : s;
      if (e.revoked === 'microphone' && s.dictating)
        extra.push('dictation.stop:permission-revoked');
      if (s.recording) extra.push(`degrade:${degrade(e.revoked, denied)}`);
    }
  } else {
    if ('granted' in e) nd.delete(e.granted);
    const o = specStep(s, e);
    next = o.s;
    decision = o.decision;
    extra = o.extra;
  }
  expect(r.decision, `decision @ ${ctx}`).toBe(decision);
  const before = 'helperLost' in e ? readers(s) : readers(s);
  const after = readers(next);
  const want = [
    ...extra,
    ...[...after]
      .filter((x) => !before.has(x))
      .map((x) => `mic.subscribe:${x}`),
    ...[...before]
      .filter((x) => !after.has(x))
      .map((x) => `mic.unsubscribe:${x}`),
  ];
  expect([...r.effects].sort(), `effects @ ${ctx}`).toEqual(want.sort());
  expect(new Set(r.state.mic), `readers @ ${ctx}`).toEqual(after);
  expect(r.state.mic.length).toBe(after.size);
  expect(r.state.warm, `warm @ ${ctx}`).toBe(next.warm);
  expect(r.state.recording, `recording @ ${ctx}`).toBe(next.recording);
  expect(r.state.activeAudioPath, `file @ ${ctx}`).toBe(next.file);
  expect(new Set(r.state.denied)).toEqual(nd);
  if (r.decision !== 'accept') {
    expect(r.state).toEqual(a);
    expect(r.reason).toBeTruthy();
  }
  if (nd.has('microphone'))
    expect(r.state.mic, `confinement @ ${ctx}`).toEqual([]);
  return { a: r.state, s: next, denied: nd };
}

const subsets = <T>(xs: T[]): T[][] =>
  xs.reduce<T[][]>((acc, x) => [...acc, ...acc.map((y) => [...y, x])], [[]]);
const spec0 = (d: HelperPermission[]): Spec => ({
  recording: false,
  file: null,
  recMicEligible: false,
  dictating: false,
  warm: false,
  micDenied: d.includes('microphone'),
});

describe('helper arbiter: declarative reader-set model (warm mic included)', () => {
  it('exhaustive over reachable states x events', () => {
    let n = 0;
    for (const init of subsets(PERMS)) {
      const seen = new Set<string>();
      const q: [HelperArbiterState, Spec, Set<HelperPermission>, string][] = [
        [
          initialArbiterState([...init]),
          spec0(init),
          new Set(init),
          JSON.stringify(init),
        ],
      ];
      while (q.length) {
        const [a, s, d, t] = q.shift()!;
        const k = JSON.stringify([
          a.recording,
          a.activeAudioPath,
          a.dictating,
          a.warm,
          [...a.mic].sort(),
          [...a.denied].sort(),
          s,
        ]);
        if (seen.has(k)) continue;
        seen.add(k);
        for (const e of EVENTS) {
          const ctx = `${t} -> ${JSON.stringify(e)}`;
          const o = check(a, s, d, e, ctx);
          n++;
          q.push([o.a, o.s, o.denied, ctx]);
        }
      }
    }
    expect(n).toBeGreaterThan(2000);
  });

  it('seeded random length-10 replays', () => {
    let seed = 0xc0ffee;
    const rnd = (k: number) =>
      (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % k;
    const inits = subsets(PERMS);
    for (let i = 0; i < 2000; i++) {
      const init = inits[rnd(inits.length)]!;
      let a = initialArbiterState([...init]);
      let s = spec0(init);
      let d = new Set(init);
      for (let j = 0; j < 10; j++) {
        const e = EVENTS[rnd(EVENTS.length)]!;
        ({
          a,
          s,
          denied: d,
        } = check(a, s, d, e, `#${i}.${j} ${JSON.stringify(e)}`));
      }
    }
  });
});
