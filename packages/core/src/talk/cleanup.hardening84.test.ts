// ADR 0236 (echo cycle 84): a run of commas types as one comma ("Friday,, but" -> "Friday, but", was "Friday, ,
// but"). The same cases as crates/qrtz-talk/tests/hardening84.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const priya = [{ term: 'Priya', aliases: ['pria'] }];
const guard = [{ term: 'Smith, , Jones', aliases: [] }];

describe('ADR 0236 a comma run types as one comma', () => {
  it.each([
    [
      'We could ship it on Friday,, but the tests are still red',
      false,
      'We could ship it on Friday, but the tests are still red.',
    ],
    [
      'I talked to the design team,, and they liked the new layout',
      false,
      'I talked to the design team, and they liked the new layout.',
    ],
    [
      'So , um , we should probably wait until Monday',
      false,
      'So, we should probably wait until Monday.',
    ],
    [
      'The numbers look good,,, honestly better than last quarter',
      false,
      'The numbers look good, honestly better than last quarter.',
    ],
    [
      "Thanks for the notes,, I'll update the doc tonight",
      false,
      "Thanks for the notes, I'll update the doc tonight.",
    ],
    [
      'If the build passes,, , we can cut the release',
      false,
      'If the build passes, we can cut the release.',
    ],
    [
      "My flight lands at six,, so I'll call you after",
      false,
      "My flight lands at six, so I'll call you after.",
    ],
    [
      'We met with Mister Patel,, and he signed the lease',
      false,
      'We met with Mr. Patel, and he signed the lease.',
    ],
    [
      "Let's meet on October twenty first,, after the review",
      false,
      "Let's meet on October 21st, after the review.",
    ],
    [
      'The demo went well,, I think the client is happy',
      false,
      'The demo went well, I think the client is happy.',
    ],
    [
      'We met at the café,, then walked home',
      false,
      'We met at the café, then walked home.',
    ],
    ['one,, two,,, three', false, 'One, two, three.'],
    ['Pick one,\n, then the other', false, 'Pick one, then the other.'],
    ['Fill in row a,,b of the sheet', false, 'Fill in row a, b of the sheet.'],
    [
      'Hi pria,, can you send the deck',
      true,
      'Hi Priya, can you send the deck.',
    ],
    [
      'Ship it Friday,,, but only if QA passes',
      false,
      'Ship it Friday, but only if QA passes.',
    ],
    [
      'we could ship it friday,, but not today',
      false,
      'We could ship it friday, but not today.',
    ],
    ['It costs 5,, maybe 6 dollars', false, 'It costs 5, maybe 6 dollars.'],
    ['Call José,, he has the keys', false, 'Call José, he has the keys.'],
    [
      'The meeting (the long one),, ran late',
      false,
      'The meeting (the long one), ran late.',
    ],
    ['She said "fine",, then hung up', false, 'She said "fine", then hung up.'],
    [
      'We tried the ‘fast path’,, but it broke',
      false,
      'We tried the ‘fast path’, but it broke.',
    ],
    ['Okay,, so do you want it', false, 'Okay, so do you want it.'],
  ] as const)('%j', (raw, dict, want) => {
    expect(ruleCleanup(raw, dict ? priya : [])).toBe(want);
  });

  it.each([
    [
      'We could ship it on Friday,, but the tests are still red',
      'We could ship it on Friday, but the tests are still red',
    ],
    [
      'So , um , we should probably wait until Monday',
      'So, um, we should probably wait until Monday',
    ],
    [
      'We met with Mister Patel,, and he signed the lease',
      'We met with Mister Patel, and he signed the lease',
    ],
    [
      "Let's meet on October twenty first,, after the review",
      "Let's meet on October twenty first, after the review",
    ],
    [
      'The numbers look good,,, honestly better than last quarter',
      'The numbers look good, honestly better than last quarter',
    ],
    [
      "Hi pria,, I'll send the deck tonight",
      "Hi pria, I'll send the deck tonight",
    ],
    [
      'We met at the café,, then walked home',
      'We met at the café, then walked home',
    ],
  ] as const)('%j reads as one comma', (run, one) => {
    expect(ruleCleanup(run, [])).toBe(ruleCleanup(one, []));
    expect(ruleCleanup(run, priya)).toBe(ruleCleanup(one, priya));
  });

  it.each([
    [
      'The total came to 1,,000 dollars.',
      false,
      'The total came to 1, ,000 dollars.',
    ],
    [
      'The plan,; but nobody read it.',
      false,
      'The plan, ; but nobody read it.',
    ],
    ['Okay, , . Next', false, 'Okay, , . Next.'],
    [
      'We tried it,, (twice) and it failed',
      false,
      'We tried it, , (twice) and it failed.',
    ],
    [
      'Ship it Friday,, but only if QA passes',
      true,
      'Ship it Friday, , but only if QA passes.',
    ],
    [
      'We met in 東京,, then flew home',
      false,
      'We met in 東京, , then flew home.',
    ],
    ['so,, ,', false, 'So'],
    ['The list is a,,\nb', false, 'The list is a, ,\nB.'],
    ['Wait,,"quote" here', false, 'Wait, ,"quote" here'],
    [
      'Over 1,000 people signed up last week.',
      false,
      'Over 1,000 people signed up last week.',
    ],
    ["Well, well, look who's here.", false, "Well, well, look who's here."],
    ['Call me at 5, 6, or 7.', false, 'Call me at 5, 6, or 7.'],
    ['Thanks, Sam, see you tomorrow.', false, 'Thanks, Sam, see you tomorrow.'],
  ] as const)('leaves %j as before', (raw, withGuard, want) => {
    expect(ruleCleanup(raw, withGuard ? guard : [])).toBe(want);
  });
});
