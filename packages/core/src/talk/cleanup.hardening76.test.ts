// ADR 0227 (echo cycle 76): an unmarked tag question ("..., isn't it") ends with "?" instead of "."; everything else
// keeps its "." or its own mark. The same cases as crates/qrtz-talk/tests/hardening76.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

describe('ADR 0227 an unmarked tag question ends with a question mark', () => {
  it.each([
    ["It's a nice day, isn't it", "It's a nice day, isn't it?"],
    [
      "You finished the report already, didn't you",
      "You finished the report already, didn't you?",
    ],
    [
      "We should leave before the traffic gets bad, shouldn't we",
      "We should leave before the traffic gets bad, shouldn't we?",
    ],
    [
      "They're still coming to the dinner tonight, aren't they",
      "They're still coming to the dinner tonight, aren't they?",
    ],
    [
      "There's still time to fix the build, isn't there",
      "There's still time to fix the build, isn't there?",
    ],
    [
      "Thanks for checking. That was the last blocker, wasn't it",
      "Thanks for checking. That was the last blocker, wasn't it?",
    ],
    [
      "It's too late to change the venue now, don't you think",
      "It's too late to change the venue now, don't you think?",
    ],
    [
      "I was right about the date, wasn't I",
      "I was right about the date, wasn't I?",
    ],
    ["It's a nice day, isn\u2019t it", "It's a nice day, isn\u2019t it?"],
    ["Thanks\nIt works now, doesn't it", "Thanks\nIt works now, doesn't it?"],
    ["It costs twenty five dollars, doesn't it", "It costs $25, doesn't it?"],
    [
      "That's the whole plan, wouldn't you say",
      "That's the whole plan, wouldn't you say?",
    ],
    ["Sit down for a minute, won't you", "Sit down for a minute, won't you?"],
  ])('%j', (raw, want) => {
    expect(ruleCleanup(raw)).toBe(want);
  });
});

describe('ADR 0227 refusals keep the period or their own mark', () => {
  it.each([
    ["Maybe they will, maybe they won't", "Maybe they will, maybe they won't."],
    [
      "She said she would come, but she didn't",
      "She said she would come, but she didn't.",
    ],
    ["As far as I know, it isn't", "As far as I know, it isn't."],
    ["Whatever happens, don't you worry", "Whatever happens, don't you worry."],
    [
      "If it rains, don't you dare cancel",
      "If it rains, don't you dare cancel.",
    ],
    ["We tried it twice, didn't work", "We tried it twice, didn't work."],
    ['Go up, down, left, right', 'Go up, down, left, right.'],
    ["I'll do it, okay", "I'll do it, okay."],
    ['Close the door for me, will you', 'Close the door for me, will you.'],
    ["It's a nice day isn't it", "It's a nice day isn't it."],
    ["It's a nice day, Isn't it", "It's a nice day, Isn't it."],
    ["The twins are here, isn't they", "The twins are here, isn't they."],
    ["That one works, don't it", "That one works, don't it."],
    ["It's late already, isn't that", "It's late already, isn't that."],
    [
      "They're coming tonight, aren't they Sam",
      "They're coming tonight, aren't they Sam.",
    ],
    ["That's the plan, don't you say", "That's the plan, don't you say."],
    ["It's a nice day, isn't it.", "It's a nice day, isn't it."],
    ["It's a great song, isn't it!", "It's a great song, isn't it!"],
    ["Nice, isn't it", "Nice, isn't it"],
    [
      "We won the game, didn't we. Then we celebrated",
      "We won the game, didn't we. Then we celebrated.",
    ],
    ["It works now, doesn't it\nThanks", "It works now, doesn't it\nThanks."],
    [
      "We need to talk; it's late, isn't it",
      "We need to talk; it's late, isn't it.",
    ],
  ])('%j', (raw, want) => {
    expect(ruleCleanup(raw)).toBe(want);
  });
});
