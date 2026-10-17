import { describe, expect, it } from 'vitest';
import {
  buildSummaryPrompt,
  captureGapGuidance,
  captureGapsForPrompt,
} from './llm';
import type { SilentSpan } from './lifecycle';

const seg = {
  id: 'u0',
  startMs: 0,
  endMs: 900,
  text: 'We ship on Friday.',
  source: 'live' as const,
};
const min = 60_000;

describe('N29 capture gaps in the summary prompt', () => {
  it('lists both-silent stretches as no audio at all and the rest per source, in meeting order', () => {
    const spans: SilentSpan[] = [
      { source: 'mic', startMs: 2 * min, endMs: 10 * min },
      { source: 'system', startMs: 5 * min, endMs: 12 * min },
      { source: 'system', startMs: 20 * min, endMs: 20 * min + 30_000 }, // too short
    ];
    expect(captureGapsForPrompt(spans)).toEqual([
      "- 2:00–5:00: no audio from this user's microphone",
      '- 5:00–10:00: no audio at all (nothing was captured)',
      "- 10:00–12:00: no audio from the other participants (the computer's sound)",
    ]);
  });

  it('a stretch with no end runs to the meeting length, and is dropped when that is unknown', () => {
    const spans: SilentSpan[] = [{ source: 'mic', startMs: 3 * min }];
    expect(captureGapsForPrompt(spans, 9 * min)).toEqual([
      "- 3:00–9:00: no audio from this user's microphone",
    ]);
    expect(captureGapsForPrompt(spans)).toEqual([]);
  });

  it('overlapping spans of one source merge; at most 20 lines', () => {
    expect(
      captureGapsForPrompt([
        { source: 'mic', startMs: 0, endMs: 2 * min },
        { source: 'mic', startMs: min, endMs: 3 * min },
      ]),
    ).toEqual(["- 0:00–3:00: no audio from this user's microphone"]);
    const many: SilentSpan[] = Array.from({ length: 30 }, (_, i) => ({
      source: 'mic',
      startMs: i * 3 * min,
      endMs: i * 3 * min + 2 * min,
    }));
    expect(captureGapsForPrompt(many)).toHaveLength(20);
  });

  it('the prompt carries the gaps and the rule only when there are large gaps', () => {
    const plain = buildSummaryPrompt('T', [seg]);
    expect(plain[0]!.content).not.toContain('Capture gaps');
    expect(plain[1]!.content).not.toContain('Capture gaps');
    const p = buildSummaryPrompt('T', [seg], {
      silentSpans: [{ source: 'system', startMs: 0, endMs: 2 * min }],
    });
    // N34: a one-sided gap is not "nothing is known": the mic transcript there is real and is summarized.
    expect(p[0]!.content).not.toContain('never summarize');
    expect(p[0]!.content).toContain(
      'the transcript lines in that stretch are real',
    );
    expect(p[1]!.content).toContain(
      "Capture gaps:\n- 0:00–2:00: no audio from the other participants (the computer's sound)\n\nTranscript:",
    );
  });

  it('N34: only a no-audio-at-all stretch gets the never-summarize rule; a mixed recording gets both rules', () => {
    const both = buildSummaryPrompt('T', [seg], {
      silentSpans: [
        { source: 'system', startMs: 0, endMs: 2 * min },
        { source: 'mic', startMs: 0, endMs: 2 * min },
      ],
    });
    expect(both[0]!.content).toContain('never summarize, guess or attribute');
    expect(both[0]!.content).not.toContain('only one source');
    const mixed = buildSummaryPrompt('T', [seg], {
      silentSpans: [
        { source: 'system', startMs: 0, endMs: 5 * min },
        { source: 'mic', startMs: 3 * min, endMs: 5 * min },
      ],
    });
    expect(mixed[0]!.content).toContain('never summarize, guess or attribute');
    expect(mixed[0]!.content).toContain('only one source had no audio');
    expect(captureGapGuidance([])).toBe('');
  });

  it('N39: a lost mic counts as mic silence, so silent system audio after it is no audio at all', () => {
    const silent: SilentSpan[] = [
      { source: 'system', startMs: 2 * min, endMs: 5 * min },
    ];
    const lost: SilentSpan[] = [
      { source: 'mic', startMs: 1 * min, endMs: 6 * min },
    ];
    expect(captureGapsForPrompt(silent, undefined, lost)).toEqual([
      "- 1:00–2:00: no audio from this user's microphone",
      '- 2:00–5:00: no audio at all (nothing was captured)',
      "- 5:00–6:00: no audio from this user's microphone",
    ]);
    // Before N39 the same stretch was one-sided: "the transcript there is real".
    expect(captureGapsForPrompt(silent)).toEqual([
      "- 2:00–5:00: no audio from the other participants (the computer's sound)",
    ]);
    const p = buildSummaryPrompt('T', [seg], {
      silentSpans: silent,
      lostSpans: lost,
    });
    expect(p[0]!.content).toContain('never summarize, guess or attribute');
  });

  it('N39: a lost stretch ended by a reconnect leaves the working mic after it alone (no N34 regression)', () => {
    const silent: SilentSpan[] = [
      { source: 'system', startMs: 4 * min, endMs: 8 * min },
    ];
    const lost: SilentSpan[] = [
      { source: 'mic', startMs: 1 * min, endMs: 3 * min },
    ];
    expect(captureGapsForPrompt(silent, 10 * min, lost)).toEqual([
      "- 1:00–3:00: no audio from this user's microphone",
      "- 4:00–8:00: no audio from the other participants (the computer's sound)",
    ]);
  });

  it('N39: a lost stretch with no end (the app died) runs to the meeting length; both sources lost is no audio at all', () => {
    const lost: SilentSpan[] = [
      { source: 'mic', startMs: 2 * min },
      { source: 'system', startMs: 2 * min },
    ];
    expect(captureGapsForPrompt([], 5 * min, lost)).toEqual([
      '- 2:00–5:00: no audio at all (nothing was captured)',
    ]);
    expect(captureGapsForPrompt([], undefined, lost)).toEqual([]);
  });
});
