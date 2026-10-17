import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  arbiterStep,
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
  it('has the expected coverage', () => expect(vectors.cases.length).toBe(10));
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
      { cmd: 'recording.start' },
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
