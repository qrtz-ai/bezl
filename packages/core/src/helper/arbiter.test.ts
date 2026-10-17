import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  arbiterInvariantViolation,
  arbiterStep,
  assertArbiterInvariant,
  degradationsForRevocation,
  initialArbiterState,
  type HelperArbiterEvent,
  type HelperPermission,
} from './arbiter';

const vectors = JSON.parse(
  fs.readFileSync(
    path.resolve(__dirname, '../../../../spec/vectors/helper/arbiter.json'),
    'utf8',
  ),
) as {
  cases: {
    name: string;
    initialDenied: HelperPermission[];
    steps: { event: HelperArbiterEvent; expect: Record<string, unknown> }[];
  }[];
};

describe('helper arbiter: shared vectors (TS ⇄ Swift)', () => {
  it('has the expected coverage', () => expect(vectors.cases.length).toBe(15));
  for (const c of vectors.cases)
    it(c.name, () => {
      let s = initialArbiterState(c.initialDenied);
      for (const [i, st] of c.steps.entries()) {
        const r = arbiterStep(s, st.event);
        expect(
          { ...r, reason: r.reason },
          `step ${i} ${JSON.stringify(st.event)}`,
        ).toEqual({
          reason: undefined,
          ...st.expect,
        });
        s = r.state;
      }
    });
});

describe('helper arbiter: invariants', () => {
  it('the mic tap runs iff someone subscribed; mux only when both kinds run', () => {
    const evs: HelperArbiterEvent[] = [
      { cmd: 'recording.start', audioPath: 'a.wav' },
      { cmd: 'dictation.start' },
      { revoked: 'screen-recording' },
      { cmd: 'dictation.cancel' },
      { cmd: 'dictation.start' },
      { cmd: 'recording.stop' },
      { cmd: 'dictation.stop' },
    ];
    let s = initialArbiterState();
    for (const e of evs) {
      const r = arbiterStep(s, e);
      s = r.state;
      expect(new Set(s.mic).size).toBe(s.mic.length);
      expect(r.mux).toBe(s.recording && s.dictating);
    }
    expect(s.mic).toEqual([]);
  });
  it('a recording never runs without a file: a start with no audioPath is refused', () => {
    const s = initialArbiterState();
    const r = arbiterStep(s, { cmd: 'recording.start' });
    expect(r).toMatchObject({
      decision: 'refuse',
      reason: 'no-audio-path',
      effects: [],
    });
    expect(r.state).toEqual(s);
  });
  it('recordingFileFailed tears the recording down and leaves a dictation running', () => {
    let s = initialArbiterState();
    s = arbiterStep(s, { cmd: 'dictation.start' }).state;
    s = arbiterStep(s, { cmd: 'recording.start', audioPath: 'a.wav' }).state;
    expect(s.activeAudioPath).toBe('a.wav');
    // Only the admitted recording's own file counts.
    expect(
      arbiterStep(s, { recordingFileFailed: { audioPath: 'b.wav' } }),
    ).toMatchObject({ decision: 'ignore', reason: 'other-recording' });
    const r = arbiterStep(s, { recordingFileFailed: { audioPath: 'a.wav' } });
    expect(r.decision).toBe('accept');
    expect(r.effects).toEqual(['recording.lost', 'mic.unsubscribe:recording']);
    expect(r.state).toMatchObject({
      recording: false,
      activeAudioPath: null,
      dictating: true,
      mic: ['dictation'],
    });
    expect(
      arbiterStep(r.state, { recordingFileFailed: { audioPath: 'a.wav' } }),
    ).toMatchObject({ decision: 'ignore', reason: 'not-recording' });
  });
  it('asserts the invariant: running implies an activeAudioPath', () => {
    const broken = { ...initialArbiterState(), recording: true };
    expect(arbiterInvariantViolation(broken)).toMatch(
      /without an activeAudioPath/,
    );
    expect(() => assertArbiterInvariant(broken)).toThrow(/invariant broken/);
    expect(() =>
      arbiterStep(broken, { configure: { warmMic: false } }),
    ).toThrow(/invariant broken/);
    expect(
      arbiterInvariantViolation({
        ...initialArbiterState(),
        activeAudioPath: 'a.wav',
      }),
    ).toMatch(/not recording/);
  });
  it('maps revocations to recording degradations', () => {
    expect(degradationsForRevocation('microphone', [])).toEqual(['no-mic']);
    expect(
      degradationsForRevocation('microphone', ['screen-recording']),
    ).toEqual(['audio-unavailable']);
    expect(degradationsForRevocation('screen-recording', [])).toEqual([
      'mic-only',
    ]);
    expect(degradationsForRevocation('accessibility', [])).toEqual([
      'ax-unavailable',
    ]);
  });
});
