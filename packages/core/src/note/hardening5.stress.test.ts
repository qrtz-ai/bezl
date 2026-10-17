/**
 * Hardening cycle 5 (N21, middle-drop): a transcript over the prompt budget is shown as evenly spaced,
 * line-aligned windows across the whole meeting. Before, only the head and the tail were kept, so the middle
 * of a 3 h meeting never reached the summary. The budget is never exceeded and no cut leaves a lone surrogate.
 */
import { describe, expect, it } from 'vitest';
import { formatTranscriptForPrompt } from './llm';
import type { Segment } from './types';

const seg = (i: number, text: string, stepMs = 4000): Segment =>
  ({
    id: `s${i}`,
    startMs: i * stepMs,
    endMs: i * stepMs + 3500,
    text,
    source: 'live',
  }) as Segment;
const MARKER = /^\[… \d+:\d\d–\d+:\d\d not shown …\]$/;
const lone =
  /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
const threeHours = () =>
  Array.from({ length: 2700 }, (_, i) =>
    seg(i, `line ${i}: we talked about the roadmap and the launch date again`),
  );

describe('hardening 5: the middle of a long meeting reaches the summary (N21)', () => {
  it('a 3 h meeting: no stretch longer than 1/6 of it is left out, and the middle minute is in', () => {
    const segs = threeHours();
    segs[1350] = seg(1350, 'we decided to cut the pricing tier ZEBRA');
    const out = formatTranscriptForPrompt(segs);
    expect(out.length).toBeLessThanOrEqual(60_000);
    const kept = out
      .split('\n')
      .filter((l) => !MARKER.test(l))
      .map((l) =>
        Number(
          /^\[\d+:\d\d\] Speaker: line (\d+):/.exec(l)?.[1] ??
            (l.includes('ZEBRA') ? 1350 : NaN),
        ),
      );
    expect(kept.every((n) => Number.isInteger(n))).toBe(true); // whole lines only
    expect(kept[0]).toBe(0);
    expect(kept.at(-1)).toBe(2699);
    expect(out).toContain('ZEBRA');
    let gap = 0;
    for (let i = 1; i < kept.length; i++)
      gap = Math.max(gap, kept[i]! - kept[i - 1]!);
    expect(gap).toBeLessThanOrEqual(2700 / 6);
  });

  it('every gap is named by a marker that brackets exactly the stretch left out', () => {
    const out = formatTranscriptForPrompt(threeHours());
    const rows = out.split('\n');
    const at = (l: string) => /^\[(\d+:\d\d)\]/.exec(l)?.[1];
    const mins = (c: string) => {
      const [m, s] = c.split(':').map(Number);
      return m! * 60 + s!;
    };
    let markers = 0;
    for (let i = 1; i < rows.length - 1; i++) {
      const m = /^\[… (\d+:\d\d)–(\d+:\d\d) not shown …\]$/.exec(rows[i]!);
      if (!m) continue;
      markers++;
      expect(mins(m[1]!)).toBe(mins(at(rows[i - 1]!)!) + 4);
      expect(mins(m[2]!)).toBe(mins(at(rows[i + 1]!)!) - 4);
    }
    expect(markers).toBeGreaterThanOrEqual(1);
    expect(markers).toBeLessThanOrEqual(7);
  });

  it('never exceeds the budget, whatever the budget and line lengths', () => {
    const segs = Array.from({ length: 900 }, (_, i) =>
      seg(i, 'x'.repeat(10 + ((i * 7919) % 400))),
    );
    const over: number[] = [];
    for (let max = 200; max < 80_000; max += 997)
      if (formatTranscriptForPrompt(segs, max).length > max) over.push(max);
    expect(over).toEqual([]);
  });

  it('a single segment longer than the whole budget is cut, surrogate-safe, to the budget', () => {
    const giant = [seg(0, '🚀'.repeat(50_000))];
    for (let max = 1000; max < 1020; max++) {
      const out = formatTranscriptForPrompt(giant, max);
      expect(out.length).toBeLessThanOrEqual(max);
      expect(lone.test(out)).toBe(false);
      expect(out.startsWith('[0:00] Speaker: 🚀')).toBe(true);
    }
  });

  it('giant emoji lines among short ones: no lone surrogate, the last line (a giant one) still starts in', () => {
    const segs = Array.from({ length: 60 }, (_, i) =>
      seg(i, i % 7 === 3 ? '😀𠀋'.repeat(3000) : `short ${i}`),
    );
    for (let max = 5000; max < 60_000; max += 3331) {
      const out = formatTranscriptForPrompt(segs, max);
      expect(out.length).toBeLessThanOrEqual(max);
      expect(lone.test(out)).toBe(false);
      expect(out).toContain('[3:56] Speaker: 😀'); // the last line is a giant one: its start is in
    }
  });
});
