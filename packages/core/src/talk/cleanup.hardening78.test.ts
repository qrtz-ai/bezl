// ADR 0229 (echo cycle 78): "Let's … , shall we" and "Please … , will / would / can / could you" with no mark of their
// own end with "?" instead of "."; everything else keeps its "." or its own mark. The same cases as
// crates/qrtz-talk/tests/hardening78.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

describe('ADR 0229 an imperative with its tag ends with a question mark', () => {
  it.each([
    [
      "Let's grab lunch after the standup, shall we",
      "Let's grab lunch after the standup, shall we?",
    ],
    [
      "Let's move the design review to Thursday, shall we",
      "Let's move the design review to Thursday, shall we?",
    ],
    [
      'Please close the door on your way out, will you',
      'Please close the door on your way out, will you?',
    ],
    [
      'Please send me the slides before the call, would you',
      'Please send me the slides before the call, would you?',
    ],
    [
      'Please keep an eye on the build tonight, can you',
      'Please keep an eye on the build tonight, can you?',
    ],
    [
      'Please forward this to Priya, could you',
      'Please forward this to Priya, could you?',
    ],
    [
      "Thanks for the notes. Let's pick this up tomorrow, shall we",
      "Thanks for the notes. Let's pick this up tomorrow, shall we?",
    ],
    [
      'Let\u2019s call it a day, shall we',
      'Let\u2019s call it a day, shall we?',
    ],
    [
      'Please, double check the numbers before Friday, will you',
      'Please, double check the numbers before Friday, will you?',
    ],
    [
      'Please pay the twenty dollars at the front desk, would you',
      'Please pay the $20 at the front desk, would you?',
    ],
    ["Let's go, shall we", "Let's go, shall we?"],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, [])).toBe(want);
  });
});

describe('ADR 0229 other openers, orders and marks keep their period', () => {
  it.each([
    ['Close the door for me, will you', 'Close the door for me, will you.'],
    ["Let's see how it goes, we shall", "Let's see how it goes, we shall."],
    [
      "Let's meet at noon if we can, we will",
      "Let's meet at noon if we can, we will.",
    ],
    [
      'Please stop, you will hurt yourself',
      'Please stop, you will hurt yourself.',
    ],
    [
      'Please be patient, you will get it',
      'Please be patient, you will get it.',
    ],
    [
      'Please send the report by Friday, you will',
      'Please send the report by Friday, you will.',
    ],
    [
      'Please, Will, you have to see this',
      'Please, Will, you have to see this.',
    ],
    [
      'Please call me when you can, you know',
      'Please call me when you can, you know.',
    ],
    ['Please do what you will', 'Please do what you will.'],
    [
      'Please share the doc, will you. Thanks',
      'Please share the doc, will you. Thanks.',
    ],
    ['Please send it over, will you!', 'Please send it over, will you!'],
    ["Let's ship it, shall we.", "Let's ship it, shall we."],
    ["Please don't, you will regret it", "Please don't, you will regret it."],
    ["Let's be honest, we can't", "Let's be honest, we can't."],
    [
      'Please, if you can, you should come',
      'Please, if you can, you should come.',
    ],
    [
      'Please let me know whether you will',
      'Please let me know whether you will.',
    ],
    ["Let's wait and see if they will", "Let's wait and see if they will."],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, [])).toBe(want);
  });
});
