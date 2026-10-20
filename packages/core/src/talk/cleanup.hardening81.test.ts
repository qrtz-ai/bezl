// ADR 0233 (echo cycle 81): an unmarked question whose auxiliary is contracted onto the wh-word's modifier ("What
// time's the meeting tomorrow", "How long'll it take") ends with "?" like its spelled-out form; a statement or an
// unsure shape keeps its ".". The same cases as crates/qrtz-talk/tests/hardening81.rs.
import { describe, expect, it } from 'vitest';
import {
  ruleCleanup,
  WH_CONTRACTIONS,
  WH_MODIFIER_CONTRACTIONS,
} from './cleanup';

describe('ADR 0233 a question with its auxiliary contracted onto the modifier ends with "?"', () => {
  it.each([
    ["What time's the meeting tomorrow", "What time's the meeting tomorrow?"],
    ["How long's the flight to Denver", "How long's the flight to Denver?"],
    ["How long'll it take to get there", "How long'll it take to get there?"],
    [
      "How much'll it cost to fix the car",
      "How much'll it cost to fix the car?",
    ],
    ["How long've you been waiting here", "How long've you been waiting here?"],
    [
      "How far's the drive from the airport",
      "How far's the drive from the airport?",
    ],
    ["How old's your daughter now", "How old's your daughter now?"],
    ["What day's the party this week", "What day's the party this week?"],
    [
      "How much'd you pay for the tickets",
      "How much'd you pay for the tickets?",
    ],
    [
      "What time'd you get home last night",
      "What time'd you get home last night?",
    ],
    [
      "Hey what time's the dinner tonight",
      "Hey, what time's the dinner tonight?",
    ],
    ["So how long's the drive from here", "So how long's the drive from here?"],
    ["I just parked. How long'll you be", "I just parked. How long'll you be?"],
    [
      'What time\u2019s the game on Sunday',
      'What time\u2019s the game on Sunday?',
    ],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, [])).toBe(want);
  });

  it.each([
    "What time's best depends on your schedule",
    "What time's the meeting depends on the client",
    "How long's the flight matters less than the price",
    "How old's the house doesn't matter to me",
    "What day's the deadline is up to the manager",
    "How far's the drive will decide where we stay",
    "How much's the ticket varies by the day",
    "How long's the line is the real question",
    "How much's the rent determines where we live",
    "How far's the hotel changes the whole plan",
    "What time's the show remains a mystery",
    "How soon's the launch seems up in the air",
    "How long's the meeting has nothing to do with it",
    "What time's the train makes no difference",
    "How old's your car tells you a lot about it",
    "That time's the best one for me",
    "This time's going to be different",
    "Next time's on me for sure",
    "Last time's meeting ran over by an hour",
    "Any time's fine with me this week",
    "When time's up we stop writing",
    "Half time's the best part of the game",
    "Dinner time's at seven for the kids",
    "Show time's at eight tonight",
    "Some time's better than no time at all",
    "Every day's a new chance to get it right",
    "She asked how long's the drive",
    "How much's left in the budget",
    "How long's left on the timer",
    "What time's good for you",
    "How long's it been since we talked",
    "What time's the meeting, Sam",
    "What size's the problem we're facing",
    "What else's on the list today",
    "What kind's the best for us",
    "How long's he ever been late",
    "Who time's the meeting tomorrow",
    "Why long's the meeting tomorrow",
    "What time's'll the meeting be",
    "How long're he staying with us",
    "How long've she been waiting here",
    "What time's they arriving tonight",
    "Well what time's the meeting tomorrow",
    "How time's the meeting tomorrow",
    "What long's the flight to Denver",
    "How long's the flight to Denver is that right",
    "How long'm I supposed to wait here",
  ])('keeps its period: %s', (raw) => {
    expect(ruleCleanup(raw, []).endsWith('?')).toBe(false);
  });

  it('leaves a recogniser mark and a short dictation as they were', () => {
    expect(ruleCleanup("What time's the meeting tomorrow.", [])).toBe(
      "What time's the meeting tomorrow.",
    );
    expect(ruleCleanup("How long's the flight to Denver!", [])).toBe(
      "How long's the flight to Denver!",
    );
    expect(ruleCleanup("How long's it", [])).toBe("How long's it");
  });

  it('takes only the subjects that agree with each tail', () => {
    const subjects = [
      'i',
      'you',
      'we',
      'they',
      'he',
      'she',
      'it',
      'the',
      'this',
      'that',
      'my',
      'your',
      'our',
      'their',
    ];
    for (const [wh, modifiers] of WH_MODIFIER_CONTRACTIONS)
      for (const m of modifiers)
        for (const [tail, , agree] of WH_CONTRACTIONS)
          for (const s of subjects) {
            const raw = `${wh} ${m}'${tail} ${s} going to stay tonight`;
            expect(ruleCleanup(raw, []).endsWith('?'), raw).toBe(
              agree.includes(s),
            );
          }
    for (const raw of [
      "How time's the meeting tomorrow",
      "What long's the flight to Denver",
      "Where time's the meeting tomorrow",
      "When long's the flight to Denver",
      "Who much's the ticket for the show",
    ])
      expect(ruleCleanup(raw, []).endsWith('?'), raw).toBe(false);
  });

  it('leaves "Hey <dictionary name> what time\'s ..." as ADR 0206 left it', () => {
    expect(
      ruleCleanup("Hey Priya what time's the dinner tonight", [
        { term: 'Priya', aliases: [] },
      ]),
    ).toBe("Hey Priya what time's the dinner tonight.");
  });
});
