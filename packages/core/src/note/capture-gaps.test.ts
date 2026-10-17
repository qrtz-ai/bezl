import { describe, expect, it } from 'vitest';
import { buildSummaryPrompt, captureGapsForPrompt } from './llm';
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
    expect(p[0]!.content).toContain('never summarize, guess or attribute');
    expect(p[1]!.content).toContain(
      "Capture gaps:\n- 0:00–2:00: no audio from the other participants (the computer's sound)\n\nTranscript:",
    );
  });
});
