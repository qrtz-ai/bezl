// ADR 0205 (echo cycle 65): "we, we" / "they, they" collapses only when the first pronoun opens its sentence (alone
// or after so/and/but/then/because) and a plain modal follows. The same cases as crates/qrtz-talk/tests/hardening65.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts'] }];

describe('ADR 0205 a subject stutter before a modal collapses', () => {
  it.each([
    ['So we, we can ship it tomorrow.', 'So we can ship it tomorrow.'],
    ['We, we can try again tomorrow.', 'We can try again tomorrow.'],
    ['They, they should be here by now.', 'They should be here by now.'],
    ['But we, we could ask Sam first.', 'But we could ask Sam first.'],
    [
      'And they, they will send the invoice.',
      'And they will send the invoice.',
    ],
    ['Because they, they might not come.', 'Because they might not come.'],
    ['Then we, we would need a new room.', 'Then we would need a new room.'],
    ['So, we, we can ship it tomorrow.', 'So, we can ship it tomorrow.'],
    [
      "I'm tired. So we, we should go home.",
      "I'm tired. So we should go home.",
    ],
    ['Um, so we, we can ship it', 'So we can ship it.'],
    ['so we, we can ship cue arts', 'So we can ship qrtz.'],
    ['Notes new line we, we must finish today', 'Notes\nWe must finish today.'],
    [
      'Is it done? They, they may need more time.',
      'Is it done? They may need more time.',
    ],
    ['So we, we shall see.', 'So we shall see.'],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it.each([
    ['Neither have we, we were busy.', 'Neither have we, we were busy.'],
    ['Neither can we, we can only wait.', 'Neither can we, we can only wait.'],
    ['So do we, we just never say it.', 'So do we, we just never say it.'],
    ['So can they, they can come too.', 'So can they, they can come too.'],
    [
      "They're faster than we, we can keep up.",
      "They're faster than we, we can keep up.",
    ],
    [
      'You and we, we can share the cost.',
      'You and we, we can share the cost.',
    ],
    ['Tell them we, we can do it.', 'Tell them we, we can do it.'],
    ['And then we, we will see.', 'And then we, we will see.'],
    ['Okay we, we can do that.', 'Okay we, we can do that.'],
    [
      "Where are they, they can't be far.",
      "Where are they, they can't be far.",
    ],
    ['So we, we went home early.', 'So we, we went home early.'],
    ["So we, we can't make it.", "So we, we can't make it."],
    ['So we, we cannot make it.', 'So we, we cannot make it.'],
    ['So we, we candidly told them.', 'So we, we candidly told them.'],
    ['So they, they May be back.', 'So they, they May be back.'],
    ["So we, we're going home.", "So we, we're going home."],
    ['We, we.', 'We, we'],
    ['So we, we, we can ship it.', 'So we, we, we can ship it.'],
    ['So we,  we can ship it.', 'So we, we can ship it.'],
    ['Thank you, you can go now.', 'Thank you, you can go now.'],
    ['I love you, you know.', 'I love you, you know.'],
    ['Ask him, he can help.', 'Ask him, he can help.'],
    ['Take it, it will help.', 'Take it, it will help.'],
    ['So he, he can drive.', 'So he, he can drive.'],
    ['We, WE can do it.', 'We, WE can do it.'],
    ['Set they, they_can to true.', 'Set they, they_can to true.'],
  ])('%s keeps both words', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });
});
