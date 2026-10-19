// ADR 0230 (echo cycle 79): a spoken date's day is written as a figure ("on October twenty first" → "on October
// 21st"); a date shape that can read another way stays as spoken. The same cases as
// crates/qrtz-talk/tests/hardening79.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const dayFigure = /\d(st|nd|rd|th)/;

describe('ADR 0230 a spoken date’s day is written as a figure', () => {
  it.each([
    ["Let's meet on October twenty first", "Let's meet on October 21st."],
    [
      'The launch is on October fifth, so plan for it.',
      'The launch is on October 5th, so plan for it.',
    ],
    [
      'Can we push the review to November third',
      'Can we push the review to November 3rd?',
    ],
    [
      'Please send the contract by May second.',
      'Please send the contract by May 2nd.',
    ],
    ['The deadline is March thirty first', 'The deadline is March 31st.'],
    [
      'Starting January tenth we will meet weekly',
      'Starting January 10th we will meet weekly.',
    ],
    [
      "Let's meet on October fifth at three p.m.",
      "Let's meet on October 5th at 3 p.m.",
    ],
    [
      'Saturday, August fifteenth. The sea was calm.',
      'Saturday, August 15th. The sea was calm.',
    ],
    [
      'Our offsite is the week of September twenty-second',
      'Our offsite is the week of September 22nd.',
    ],
    [
      'I have been out since February twelfth.',
      'I have been out since February 12th.',
    ],
    [
      'Hi Priya, the invoice is due by June thirtieth.',
      'Hi Priya, the invoice is due by June 30th.',
    ],
    ['Tuesday, August eighteenth', 'Tuesday, August 18th'],
    ['Tuesday August eighteenth', 'Tuesday August 18th'],
    ['By May first we need the draft', 'By May 1st we need the draft.'],
    [
      'The deadline is October fifth\nTwenty people are coming.',
      'The deadline is October 5th\nTwenty people are coming.',
    ],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw)).toBe(want);
  });

  it('writes every day with its suffix', () => {
    const units =
      'first second third fourth fifth sixth seventh eighth ninth'.split(' ');
    const spoken = [
      ...units,
      ...'tenth eleventh twelfth thirteenth fourteenth fifteenth sixteenth seventeenth eighteenth nineteenth twentieth'.split(
        ' ',
      ),
      ...units.map((u) => `twenty ${u}`),
      'thirtieth',
      'thirty first',
    ];
    spoken.forEach((s, i) => {
      const n = i + 1;
      const suffix =
        n % 100 >= 11 && n % 100 <= 13
          ? 'th'
          : (['th', 'st', 'nd', 'rd'][n % 10] ?? 'th');
      expect(ruleCleanup(`It was due by December ${s}.`)).toBe(
        `It was due by December ${n}${suffix}.`,
      );
    });
  });

  it.each([
    'Give it to April first.',
    'You can go after June first.',
    "Let's focus on October first, then November.",
    'My priority is March first.',
    'In March first we went home.',
    'On April fourth graders take the state test.',
    'We are out on October fifth and sixth.',
    'We are out on October fifth, sixth, and seventh.',
    'We launched on October fifth, 2026, in Austin.',
    'We met on October twenty one at the cafe.',
    'He arrived on October fifth the team cheered.',
    'The Fourth of July parade starts at noon.',
    'We moved to October first, then to November.',
    'I talked to May first and then to Sam.',
    'In June, first we test the app.',
    "I'll check on June first, then on Sam.",
    'Book it for Monday, October fifth, twenty twenty six.',
    'Is it on October fifth or sixth?',
    "It's due on October 5th.",
    'We finished second in March.',
    'Starting March second half sales picked up.',
    'The final is on October thirty second.',
    'On October Fifth we met.',
    'On October, fifth we met.',
    'We met on October twenty, first in line.',
    'We met on October fifth, or the sixth at the latest.',
  ])('keeps %s', (raw) => {
    const out = ruleCleanup(raw);
    expect(!dayFigure.test(out) || dayFigure.test(raw), out).toBe(true);
    expect(out.split(/\s+/).length).toBe(raw.split(/\s+/).length);
  });

  it('leaves a lowercase month and a dictionary-spelled month alone', () => {
    expect(ruleCleanup('on october fifth we met')).toBe(
      'On october fifth we met.',
    );
    expect(
      ruleCleanup('Ask for help since June ninth.', [
        { term: 'June', aliases: [] },
      ]),
    ).toBe('Ask for help since June ninth.');
    expect(ruleCleanup('Ask for help since June ninth.')).toBe(
      'Ask for help since June 9th.',
    );
  });
});
