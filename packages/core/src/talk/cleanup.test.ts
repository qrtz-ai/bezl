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
});
