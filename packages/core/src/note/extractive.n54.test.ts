import { describe, expect, it } from 'vitest';
import {
  extractiveSummary,
  MAX_ACTION_ITEMS,
  MAX_NOTE_ACTION_ITEMS,
  MAX_QA,
} from './extractive';
import type { Segment } from './types';

/** N54 (ADR 0048): capped outputs and one-pass lookups (same as crates/qrtz-llm/tests/hardening14.rs). */
const seg = (i: number, speakerName: string, text: string): Segment => ({
  id: `s${i}`,
  startMs: i * 1000,
  endMs: i * 1000 + 900,
  text,
  source: 'live',
  speakerName,
});

describe('extractive summary bounds (N54)', () => {
  it('a 3 h meeting gives capped action items and Q&A, fast', () => {
    const texts: Record<number, string> = {
      0: 'Can you send the rollout plan by Friday?',
      3: 'What is blocking the importer right now?',
      7: 'What is blocking the importer right now?',
      5: 'We need to fix the flaky upload test before the release.',
    };
    const segs = Array.from({ length: 10_800 }, (_, i) =>
      seg(
        i,
        i % 2 ? 'Ben' : 'Ana',
        texts[i % 10] ??
          'The parser handles the new transcript format and the tests are green.',
      ),
    );
    const notes = Array.from({ length: 40 }, (_, i) => ({
      id: `n${i}`,
      atMs: i,
      text: `TODO item ${i}`,
      createdAt: 1,
    }));
    const t0 = performance.now();
    const s = extractiveSummary(segs, { notes });
    const ms = performance.now() - t0;
    expect(s.actionItems).toHaveLength(
      MAX_ACTION_ITEMS + MAX_NOTE_ACTION_ITEMS,
    );
    expect(s.actionItems.at(-1)).toEqual({
      text: `item ${MAX_NOTE_ACTION_ITEMS - 1}`,
    });
    expect(s.qa).toHaveLength(MAX_QA);
    expect(s.qa[0]).toEqual({
      question: 'Can you send the rollout plan by Friday?',
      answer:
        'The parser handles the new transcript format and the tests are green.',
      askedBy: 'Ana',
    });
    expect(ms).toBeLessThan(3000);
  });

  it('one speaker asking only questions is linear (was quadratic)', () => {
    const segs = Array.from({ length: 10_800 }, (_, i) =>
      seg(i, 'Ana', 'Is this thing on? Can anyone hear me?'),
    );
    const t0 = performance.now();
    const s = extractiveSummary(segs);
    expect(s.qa).toEqual([]);
    expect(performance.now() - t0).toBeLessThan(3000);
  });
});
