// ADR 0204 (echo cycle 64): when ADR 0203 ends a question that opens with "hey", "okay", "ok" or "alright" with no
// comma, the comma goes after the interjection. The same cases as crates/qrtz-talk/tests/hardening64.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts'] }];

describe('ADR 0204 an interjection before a question gets its comma', () => {
  it.each([
    [
      'Hey are you free tomorrow afternoon',
      'Hey, are you free tomorrow afternoon?',
    ],
    [
      'OK can we move the meeting to Thursday',
      'OK, can we move the meeting to Thursday?',
    ],
    ['Okay can you send me the file', 'Okay, can you send me the file?'],
    ['Ok can you call me back', 'Ok, can you call me back?'],
    [
      'Alright are we still on for tonight',
      'Alright, are we still on for tonight?',
    ],
    ["I'm outside. Hey are you home", "I'm outside. Hey, are you home?"],
    ['okay can you send me the file', 'Okay, can you send me the file?'],
    [
      'Plan new line okay can we meet at noon',
      'Plan\nOkay, can we meet at noon?',
    ],
    ['Um, hey are you free tomorrow', 'Hey, are you free tomorrow?'],
    ['Hey can you check cue arts', 'Hey, can you check qrtz?'],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it.each([
    ['Okay do the dishes', 'Okay do the dishes.'],
    ['Okay we can do that', 'Okay we can do that.'],
    ['Okay you can send it', 'Okay you can send it.'],
    ['Hey you did great today', 'Hey you did great today.'],
    ['Hey Sam are you free tomorrow', 'Hey Sam are you free tomorrow.'],
    ['Okay so do you want to go', 'Okay so do you want to go.'],
    ['Hey hey are you there', 'Hey hey are you there.'],
    ['Alright then can we start', 'Alright then can we start.'],
    ['Okay is the store open', 'Okay is the store open.'],
    ['Heyday is it today', 'Heyday is it today.'],
    ['Hey are you free. Thanks', 'Hey are you free. Thanks.'],
    [
      'Hey can you check this. Okay it works',
      'Hey can you check this. Okay it works.',
    ],
    ['Hey are you', 'Hey are you'],
    ['Okay can I', 'Okay can I'],
    ['So do you want to go', 'So do you want to go?'],
    ['Also can you send the slides', 'Also can you send the slides?'],
    ['And did you hear back from them', 'And did you hear back from them?'],
    ['Okay, can you send me the file', 'Okay, can you send me the file?'],
    ['Hey, are you free', 'Hey, are you free?'],
  ])('%s gets no comma', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });
});
