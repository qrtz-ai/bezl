import { describe, expect, it } from 'vitest';
import { applyDictionary, ruleCleanup } from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts', 'quartz'] }];

describe('talk cleanup', () => {
  it('rules: fillers, stutters, capitalization, terminal punctuation, dictionary', () => {
    expect(
      ruleCleanup(
        'um so I I think we should ship cue arts talk on friday',
        dict,
      ),
    ).toBe('So I think we should ship qrtz talk on friday.');
    expect(ruleCleanup('uh, hello there , how are you')).toBe(
      'Hello there, how are you.',
    );
    expect(
      ruleCleanup('first item new line second item new paragraph done'),
    ).toBe('First item\nSecond item\n\nDone.');
    expect(ruleCleanup('is it ready?')).toBe('Is it ready?');
    expect(ruleCleanup('i think i can')).toBe('I think I can.');
    expect(ruleCleanup('')).toBe('');
  });

  it('ADR 0101: numbers, doubles and contractions are kept; short dictations get no period; dotted tokens stay', () => {
    expect(ruleCleanup('my pin is 1 1 2 3')).toBe('My pin is 1 1 2 3.');
    expect(ruleCleanup('she had had enough by then')).toBe(
      'She had had enough by then.',
    );
    expect(ruleCleanup("thank you, you're the best")).toBe(
      "Thank you, you're the best.",
    );
    expect(ruleCleanup('the the plan is ready')).toBe('The plan is ready.');
    expect(ruleCleanup('Sounds good.')).toBe('Sounds good');
    expect(ruleCleanup('is it ready?')).toBe('Is it ready?');
    expect(ruleCleanup('the site is qrtz.ai now')).toBe(
      'The site is qrtz.ai now.',
    );
    expect(ruleCleanup('use node.js, i.e. the runtime')).toBe(
      'Use node.js, i.e. the runtime.',
    );
  });

  it('dictionary keeps the user spelling, case-insensitively and across hyphens', () => {
    expect(applyDictionary('Cue-Arts and QUARTZ', dict)).toBe('qrtz and qrtz');
    expect(applyDictionary('quartzite stays', dict)).toBe('quartzite stays');
  });
  it('ADR 0192: a filler takes its comma only after a word no comma follows', () => {
    expect(ruleCleanup('The demo is, um, basically done.')).toBe(
      'The demo is basically done.',
    );
    expect(ruleCleanup('Can you, um, send me the deck?')).toBe(
      'Can you send me the deck?',
    );
    expect(ruleCleanup('I fixed the bug and, uh, pushed the change.')).toBe(
      'I fixed the bug and pushed the change.',
    );
    expect(ruleCleanup('So, uh, the build is green now.')).toBe(
      'So, the build is green now.',
    );
    expect(ruleCleanup('If we can, um, we should ship it today.')).toBe(
      'If we can, we should ship it today.',
    );
    expect(ruleCleanup('Yes we can, um, Sam.')).toBe('Yes we can, Sam.');
  });

  it('ADR 0193: spoken amounts become figures only where the reading is certain', () => {
    expect(ruleCleanup('It costs twenty five dollars.')).toBe('It costs $25.');
    expect(ruleCleanup('We grew by fifteen percent this quarter.')).toBe(
      'We grew by 15% this quarter.',
    );
    expect(ruleCleanup('The meeting is at three thirty p.m.')).toBe(
      'The meeting is at 3:30 p.m.',
    );
    for (const raw of [
      'It costs between ten and twenty dollars.',
      'Give it a hundred percent.',
      'I have twenty five dollar bills.',
      'Twenty five dollars is too much.',
      'It was five dollars and fifty cents.',
    ])
      expect(ruleCleanup(raw), raw).toBe(raw);
  });
});
