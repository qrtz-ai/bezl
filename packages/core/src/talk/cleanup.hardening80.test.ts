// ADR 0232 (echo cycle 80): an unmarked question that opens with a contracted wh-word ("What's", "How'd",
// "What're") ends with "?" like its spelled-out form; a statement or an unsure shape keeps its ".". The same cases as
// crates/qrtz-talk/tests/hardening80.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup, WH_CONTRACTIONS } from './cleanup';

describe('ADR 0232 a question opening with a wh-contraction ends with "?"', () => {
  it.each([
    ["What's the plan for tonight", "What's the plan for tonight?"],
    [
      "Where's the meeting this afternoon",
      "Where's the meeting this afternoon?",
    ],
    ["How's the new job going", "How's the new job going?"],
    ["When's the next train to the city", "When's the next train to the city?"],
    ["Who's the lead on this project", "Who's the lead on this project?"],
    ["Why's the build so slow today", "Why's the build so slow today?"],
    ["How'd it go with the client", "How'd it go with the client?"],
    ["Where'd you put the charger", "Where'd you put the charger?"],
    [
      "What're your plans for the weekend",
      "What're your plans for the weekend?",
    ],
    ["Who're they meeting with today", "Who're they meeting with today?"],
    ["What've you been up to lately", "What've you been up to lately?"],
    ["When'll it be ready for review", "When'll it be ready for review?"],
    ["Where's the guy who called you", "Where's the guy who called you?"],
    [
      'What\u2019s the best way to get there',
      'What\u2019s the best way to get there?',
    ],
    ["So what's the plan for tomorrow", "So what's the plan for tomorrow?"],
    ["So, what's the plan for tonight", "So, what's the plan for tonight?"],
    ["And how'd the meeting go", "And how'd the meeting go?"],
    [
      "Hey what's the status on the release",
      "Hey, what's the status on the release?",
    ],
    ["Okay, how's the weather out there", "Okay, how's the weather out there?"],
    [
      "I just got back. What's the plan now",
      "I just got back. What's the plan now?",
    ],
    ["what's the plan for tonight", "What's the plan for tonight?"],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw)).toBe(want);
  });

  it.each([
    "What's done is done and we move on",
    "What's left needs to go in the fridge",
    "What's more we saved a lot of money",
    "What's important is that we ship on time",
    "What's the right answer depends on the context",
    "When's the launch matters less than the quality",
    "What's the best plan really varies by team",
    "What's the plan depends on the weather",
    "What's your loss is my gain",
    "What's on the agenda is the budget",
    "How's it going is what I always ask",
    "What's it like to be you is the question",
    "Where's the line between work and life is a good question",
    "Who's coming is up to you",
    "Who's on call has the pager this week",
    "What's she said is true",
    "What's his name called me this morning",
    "What's her name from accounting stopped by",
    "What's-his-name called me this morning",
    "Here's what's happening with the launch",
    "That's what's up with the build",
    "She's what's holding the team together",
    "Whatever's left goes to the food bank",
    "Let's see what's left in the budget",
    "He's the lead on this project",
    "It's the plan for tonight",
    "Well what's the plan for tonight",
    "Where's Waldo is my favorite book",
    "What's mine stays mine",
    "Who's coming to the party on Saturday",
    'What\u2019s up with that',
    "What's the deal with Sam's car",
    "What's the plan, Sam",
    "What's the plan we talked about yesterday",
    "What're you doing tonight I'm bored",
    "Why's he ever late to the standup",
    "What's they doing over there",
    "Where've he gone this time",
    "What's'll the plan for tonight",
    "I asked. What'd you know",
  ])('keeps its period: %s', (raw) => {
    expect(ruleCleanup(raw).endsWith('?')).toBe(false);
  });

  it('leaves a recogniser mark and a short dictation as they were', () => {
    expect(ruleCleanup("What's the plan for tonight.")).toBe(
      "What's the plan for tonight.",
    );
    expect(ruleCleanup("Where's the meeting this afternoon!")).toBe(
      "Where's the meeting this afternoon!",
    );
    expect(ruleCleanup("What's the plan")).toBe("What's the plan");
    expect(ruleCleanup("What'd you know")).toBe("What'd you know");
  });

  it('takes only the subjects that agree with each contraction', () => {
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
    for (const [tail, , agree] of WH_CONTRACTIONS)
      for (const s of subjects)
        expect(
          ruleCleanup(`Where'${tail} ${s} going to stay tonight`).endsWith('?'),
        ).toBe(agree.includes(s));
  });

  it('leaves "Hey <dictionary name> what\'s ..." as ADR 0206 did', () => {
    const dict = [{ term: 'Priya', aliases: [] }];
    expect(ruleCleanup("Hey Priya what's the plan for today", dict)).toBe(
      "Hey Priya what's the plan for today.",
    );
    expect(ruleCleanup('Hey Priya where is the meeting today', dict)).toBe(
      'Hey Priya, where is the meeting today?',
    );
  });
});
