/**
 * Hardening cycle 4: huge transcripts against the LLM prompt budget (N21). A 3h meeting is cut to fit the prompt
 * budget (head and tail in cycle 4, evenly spaced windows since cycle 5); the cut never splits a character (emoji, CJK astral) into a lone surrogate.
 */
import { describe, expect, it } from 'vitest';
import { formatTranscriptForPrompt } from './llm';
import type { Segment } from './types';

const seg = (i: number, text: string): Segment =>
  ({
    id: `s${i}`,
    startMs: i * 4000,
    endMs: i * 4000 + 3500,
    text,
    source: 'live',
  }) as Segment;

const lone =
  /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;

describe('hardening 4: huge transcripts and the prompt budget', () => {
  it('a 3 h meeting fits the budget and keeps its first and last lines', () => {
    const segs = Array.from({ length: 2700 }, (_, i) =>
      seg(
        i,
        `line ${i}: we talked about the roadmap and the launch date again`,
      ),
    );
    const out = formatTranscriptForPrompt(segs);
    expect(out.length).toBeLessThanOrEqual(60_003);
    expect(out).toContain('line 0:');
    expect(out).toContain('line 2699:');
    // Cycle 5 (N21): the cut is now evenly spaced windows with "not shown" markers, and the middle hour is
    // represented (hardening5.stress.test.ts). Cycle 4 kept only the head and the tail here.
    expect(out).toMatch(/\[… \d+:\d\d–\d+:\d\d not shown …\]/);
    expect(out).toContain('line 1350:');
  });

  it('the cut never leaves a lone surrogate, whatever the offset', () => {
    const segs = Array.from({ length: 300 }, (_, i) => seg(i, '🚀😀𠀋🎉'));
    const bad: number[] = [];
    for (let max = 1000; max < 1040; max++)
      if (lone.test(formatTranscriptForPrompt(segs, max))) bad.push(max);
    expect(bad).toEqual([]);
  });

  it('a short transcript is passed through untouched', () => {
    const out = formatTranscriptForPrompt([seg(0, 'hello 👋')]);
    expect(out).toBe('[0:00] Speaker: hello 👋');
  });
});
