// ADR 0201 (echo cycle 62): a dictated list's line commas go; greetings, sign-offs, names, verse and anything
// unclear keep them. The same cases as crates/qrtz-talk/tests/hardening62.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts'] }];

describe('ADR 0201 list line commas', () => {
  it.each([
    ['Groceries, new line, eggs, new line, milk.', 'Groceries\nEggs\nMilk'],
    ['To do, new line, call mom, new line, pay rent.', 'To do\nCall mom\nPay rent'],
    [
      'Shopping, new line, olive oil, new line, rice, new line, paper towels',
      'Shopping\nOlive oil\nRice\nPaper towels',
    ],
    ['Options, new line, red, new line, blue, new line, green.', 'Options\nRed\nBlue\nGreen'],
    ['Groceries, new line, eggs, new line, milk', 'Groceries\nEggs\nMilk'],
    ['Groceries,\neggs,\nmilk.', 'Groceries\nEggs\nMilk'],
  ])('drops the commas of %j', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it.each([
    ['Hi team, new line, thanks, new line, Jared', 'Hi team,\nThanks,\nJared'],
    ['Dear team, new line, quick update, new line, lunch moved.', 'Dear team,\nQuick update,\nLunch moved'],
    ['Thanks, new line, love, new line, mom', 'Thanks,\nLove,\nMom'],
    ['Cheers, new line, see you, new line, soon.', 'Cheers,\nSee you,\nSoon'],
    ['Talk soon, new line, take care, new line, bye', 'Talk soon,\nTake care,\nBye'],
    ['Hey Sam, new line, quick one, new line, call me.', 'Hey Sam,\nQuick one,\nCall me'],
    ['Morning all, new line, standup moved, new line, sorry.', 'Morning all,\nStandup moved,\nSorry'],
    [
      'Kind regards, new line, the cue arts team, new line, San Francisco',
      'Kind regards,\nThe qrtz team,\nSan Francisco',
    ],
    [
      'Jared Grimes, new line, Naples Street, new line, San Francisco',
      'Jared Grimes,\nNaples Street,\nSan Francisco',
    ],
    ['Groceries, new line, eggs, new line, Sam', 'Groceries,\nEggs,\nSam'],
    [
      'Roses are red, new line, violets are blue, new line, sugar is sweet.',
      'Roses are red,\nViolets are blue,\nSugar is sweet',
    ],
    [
      'Twinkle twinkle, new line, little star, new line, how I wonder.',
      'Twinkle twinkle,\nLittle star,\nHow I wonder',
    ],
    ["Hello, new line, it's me, new line, Jared.", "Hello,\nIt's me,\nJared"],
    ['Parts, new line, two bolts, new line, 4 nuts.', 'Parts,\nTwo bolts,\n4 nuts'],
    ['Agenda, new line, Q3 numbers, new line, hiring.', 'Agenda,\nQ3 numbers,\nHiring'],
    ['Groceries, new line, eggs, milk, new line, bread.', 'Groceries,\nEggs, milk,\nBread'],
    ['Groceries, new line, eggs new line milk', 'Groceries,\nEggs\nMilk'],
    ['Groceries, new line, eggs, new line, milk!', 'Groceries,\nEggs,\nMilk!'],
    [
      'Groceries, new paragraph, eggs, new line, milk, new line, bread.',
      'Groceries,\n\nEggs,\nMilk,\nBread.',
    ],
    [
      'Agenda, new line, budget, new line, hiring, new line, what do we do next?',
      'Agenda,\nBudget,\nHiring,\nWhat do we do next?',
    ],
    ['Eggs, new line, milk.', 'Eggs,\nMilk.'],
    ['groceries, new line, eggs, new line, milk', 'Groceries,\nEggs,\nMilk'],
  ])('keeps the commas of %j', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });
});
