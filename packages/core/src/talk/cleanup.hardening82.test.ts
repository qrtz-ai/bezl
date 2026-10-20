// ADR 0234 (echo cycle 82): a spoken title before a name is written as its abbreviation ("Mister Hopkins" ->
// "Mr. Hopkins", "Mister and Missus Lee" -> "Mr. and Mrs. Lee"); a vocative "mister", a sentence after it, the
// "Mister Rogers" names and a dictionary that spells the title keep the words. The same cases as
// crates/qrtz-talk/tests/hardening82.rs.
import { describe, expect, it } from 'vitest';
import {
  ruleCleanup,
  SPOKEN_TITLES,
  TITLE_KEPT,
  TITLE_NOT_NAMES,
} from './cleanup';

describe('ADR 0234 a spoken title before a name is written as its abbreviation', () => {
  it.each([
    [
      'I had a call with Mister Patel about the contract',
      'I had a call with Mr. Patel about the contract.',
    ],
    [
      'Please send the invoice to Missus Chen by Friday',
      'Please send the invoice to Mrs. Chen by Friday.',
    ],
    [
      'Mister Thompson will join the meeting at noon',
      'Mr. Thompson will join the meeting at noon.',
    ],
    [
      'Can you ask mister Garcia to call me back',
      'Can you ask Mr. Garcia to call me back?',
    ],
    [
      'Mister and Missus Lee are coming to dinner on Saturday',
      'Mr. and Mrs. Lee are coming to dinner on Saturday.',
    ],
    [
      'We got a lovely card from Mister and Mrs. Walker.',
      'We got a lovely card from Mr. and Mrs. Walker.',
    ],
    ['Missus and Mister Smith are here', 'Mrs. and Mr. Smith are here.'],
    ['Thank you, Mister President.', 'Thank you, Mr. President.'],
    [
      "I left a message for Mister O'Brien at the front desk",
      "I left a message for Mr. O'Brien at the front desk.",
    ],
    [
      "Mister Kim's flight lands at six tonight",
      "Mr. Kim's flight lands at six tonight.",
    ],
    [
      'Missus Chen\u2019s office is upstairs',
      'Mrs. Chen\u2019s office is upstairs.',
    ],
    [
      "Mister Smiths' house is the blue one",
      "Mr. Smiths' house is the blue one.",
    ],
    [
      'The package is for Missus Rodriguez-Lopez in apartment four',
      'The package is for Mrs. Rodriguez-Lopez in apartment four.',
    ],
    [
      'Mister Smith, Missus Jones and Mister Lee signed it',
      'Mr. Smith, Mrs. Jones and Mr. Lee signed it.',
    ],
    [
      'Attendees new line Mister Hall new line Missus Grant',
      'Attendees\nMr. Hall\nMrs. Grant',
    ],
    ['Mister Smith?', 'Mr. Smith?'],
    [
      'So, to the surprise of the Democratic Committee and all his friends, mister Hopkins announced',
      'So, to the surprise of the Democratic Committee and all his friends, Mr. Hopkins announced.',
    ],
    [
      'Lady Crichton, Mister and Mrs. Vanderpool, Mister Harry Cresswell and his sister',
      'Lady Crichton, Mr. and Mrs. Vanderpool, Mr. Harry Cresswell and his sister.',
    ],
  ])('%j -> %j', (raw, want) => {
    expect(ruleCleanup(raw)).toBe(want);
  });

  it.each([
    ['Hey mister, you dropped your wallet.'],
    ['Thanks mister. That was really kind of you.'],
    ['Excuse me mister I think you dropped this.'],
    ['Excuse me mister i think you dropped this.'],
    ["Listen mister I'm not going to ask you again."],
    ["Sorry mister It's not for sale."],
    ['Thank you mister The car is ready out front.'],
    ['Bye mister See you tomorrow.'],
    ['Good morning mister What can I get you?'],
    ['Thanks mister May I ask your name?'],
    ['Thanks mister Nobody else stopped to help.'],
    ['Hey Mister, Sam says hi.'],
    ['Mister, Smith is here to see you.'],
    ['We watched Mister Rogers with the kids last night.'],
    ['The Mister Softee truck came by again today.'],
    ['Grandpa loved watching Mister Ed reruns.'],
    ['There is a Mister Donut right by the station.'],
    ['Mister Mister was a band in the eighties.'],
    ['She called him a little mister all day long.'],
    ['The missus wants us home by six.'],
    ['Say hi to the mister and the missus for me.'],
    ['We met Mister on the train, or so he called himself.'],
    ['Mister and Missus'],
    ['Mister T pities the fool.'],
    ['Hey Mister DJ, play our song.'],
    ["Ask Mister O'brien about the lease."],
    ['Mister Smith- is here.'],
    ['I met Mister Jos\u00e9 Ramos at the conference.'],
    ['The meeting with Mister Monday is canceled.'],
    ['Doctor Who is my favorite show.'],
    ['Saint Paul is a lovely city in the summer.'],
    ['Miss Daisy will see you now.'],
    ['Mr. Smith is already in the lobby.'],
    ['Mrs Smith and Mr Jones are both here.'],
    ['Mister and Mrs Smith arrived early.'],
    ['Mister and misses Smith arrived early.'],
    ['MISTER SMITH IS HERE.'],
    ['MISTER Smith is here.'],
    ['Misters Smith and Jones are here.'],
    ["Mister's here already"],
    ['(Mister Smith) is here.'],
    ['Talk to mister\nSmith tomorrow.'],
  ])('%j keeps its title', (raw) => {
    const got = ruleCleanup(raw);
    expect(!got.includes('Mr.') || raw.includes('Mr.')).toBe(true);
    expect(!got.includes('Mrs.') || raw.includes('Mrs.')).toBe(true);
  });

  it('keeps the text byte for byte where no other step applies', () => {
    for (const raw of [
      'Excuse me mister I think you dropped this.',
      'We watched Mister Rogers with the kids last night.',
      'Mister and misses Smith arrived early.',
      'Talk to mister\nSmith tomorrow.',
    ])
      expect(ruleCleanup(raw)).toBe(raw);
  });

  it('is off when a dictionary term or alias spells the title', () => {
    const raw = 'I had a call with Mister Patel about the contract';
    for (const dict of [
      [{ term: 'Mister Softee', aliases: [] }],
      [{ term: 'Mrs. Chen', aliases: ['missus chen'] }],
    ])
      expect(ruleCleanup(raw, dict)).toBe(
        'I had a call with Mister Patel about the contract.',
      );
    expect(ruleCleanup(raw, [{ term: 'MISTER', aliases: [] }])).toBe(
      'I had a call with MISTER Patel about the contract.',
    );
    expect(
      ruleCleanup(raw, [
        { term: 'Priya', aliases: ['pria'] },
        { term: 'Patel', aliases: [] },
      ]),
    ).toBe('I had a call with Mr. Patel about the contract.');
  });

  it('reads the same names after every title', () => {
    const names: [string, boolean][] = [
      ['Smith', true],
      ["Smith's", true],
      ['Smith\u2019s', true],
      ["Jones'", true],
      ["O'Brien", true],
      ['O\u2019Brien', true],
      ['McDonald', true],
      ['Rodriguez-Lopez', true],
      ['Smith,', true],
      ['Smith;', true],
      ['President', true],
      ['smith', false],
      ['SMITH', false],
      ['S', false],
      ['I', false],
      ["I'm", false],
      ["It's", false],
      ["Don't", false],
      ["O'brien", false],
      ['Smith-', false],
      ['Smith-jones', false],
      ['Jos\u00e9', false],
      ['Smith2', false],
      ['May', false],
      ['Friday', false],
      ['Rogers', false],
      ['Ed', false],
    ];
    for (const [spoken, abbr] of SPOKEN_TITLES)
      for (const [name, converts] of names) {
        const out = ruleCleanup(
          `We spoke with ${spoken} ${name} about the lease`,
        );
        if (converts)
          expect(out).toBe(`We spoke with ${abbr} ${name} about the lease.`);
        else expect(out).toContain(spoken);
      }
    for (const w of [...TITLE_KEPT, ...TITLE_NOT_NAMES])
      expect(
        ruleCleanup(`We spoke with Mister ${w} about the lease`),
      ).toContain('Mister');
  });

  it('has the same lists as cleanup.rs', () => {
    expect(TITLE_NOT_NAMES.size).toBe(203);
    expect([...TITLE_KEPT]).toEqual(['Rogers', 'Ed', 'Softee', 'Donut']);
  });
});
