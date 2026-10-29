// ADR 0238 (echo cycle 85): a comma before a curly closing quote does not get a space (N205). Same cases as
// crates/qrtz-talk/tests/hardening85.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const priya = [{ term: 'Priya', aliases: ['pria'] }];

describe('ADR 0238 curly close after comma keeps no space', () => {
  it.each([
    [
      'The plan was \u201Cship it Friday,\u201D then cut the release',
      false,
      'The plan was \u201Cship it Friday,\u201D then cut the release.',
    ],
    [
      'She said \u201CI\u2019m ready,\u201D and left',
      false,
      'She said \u201CI\u2019m ready,\u201D and left.',
    ],
    [
      'Call it \u2018done,\u2019 then move on',
      false,
      'Call it \u2018done,\u2019 then move on.',
    ],
    [
      'We marked it \u201Cblocked,\u201D so nobody starts it',
      false,
      'We marked it \u201Cblocked,\u201D so nobody starts it.',
    ],
    [
      'His note ended \u201Cthanks,\u201D',
      false,
      'His note ended \u201Cthanks,\u201D',
    ],
    [
      'Pick \u201CA,\u201D \u201CB,\u201D or \u201CC\u201D',
      false,
      'Pick \u201CA,\u201D \u201CB,\u201D or \u201CC\u201D',
    ],
    [
      'The label read \u201Cfragile,\u201D on every box',
      false,
      'The label read \u201Cfragile,\u201D on every box.',
    ],
    [
      'Write \u201Curgent,\u201D in the subject',
      false,
      'Write \u201Curgent,\u201D in the subject.',
    ],
    [
      'I told Priya \u201Cship tonight,\u201D before the freeze',
      true,
      'I told Priya \u201Cship tonight,\u201D before the freeze.',
    ],
    [
      'Use \u201Cok,\u201D; then restart',
      false,
      'Use \u201Cok,\u201D; then restart.',
    ],
    [
      'Tag it \u201Cv1,\u201D and publish',
      false,
      'Tag it \u201Cv1,\u201D and publish.',
    ],
  ] as const)('%s', (raw, useDict, want) => {
    const got = ruleCleanup(raw, useDict ? priya : []);
    expect(got).toBe(want);
    expect(got.includes(', \u201D') || got.includes(', \u2019')).toBe(false);
  });
});

describe('ADR 0238 curly close after comma refusals', () => {
  it.each([
    [
      'The plan was "ship it Friday," then cut the release',
      false,
      'The plan was "ship it Friday," then cut the release.',
    ],
    ["Call it 'done,' then move on", false, "Call it 'done,' then move on."],
    ['He said,\u201Chello\u201D', false, 'He said, \u201Chello\u201D'],
    ['She whispered,\u2018wait\u2019', false, 'She whispered, \u2018wait\u2019'],
    [
      'Odd form \u201Cplan A, \u201D then',
      false,
      'Odd form \u201Cplan A, \u201D then.',
    ],
    [
      'Well, well, well, look who it is',
      false,
      'Well, well, well, look who it is.',
    ],
    ['Over 1,000 people signed up', false, 'Over 1,000 people signed up.'],
    [
      'It\u2019s a nice day, isn\u2019t it',
      false,
      'It\u2019s a nice day, isn\u2019t it?',
    ],
    [
      'Let\u2019s call it a day, shall we',
      false,
      'Let\u2019s call it a day, shall we?',
    ],
    ['See (a,) then (b)', false, 'See (a,) then (b)'],
    ['See [a,] then [b]', false, 'See [a,] then [b]'],
    [
      'Hey Priya, thanks for the update',
      true,
      'Hey Priya, thanks for the update.',
    ],
    ['Dit-il,\u00BBoui\u00AB', false, 'Dit-il, \u00BBoui\u00AB'],
  ] as const)('%s', (raw, useDict, want) => {
    expect(ruleCleanup(raw, useDict ? priya : [])).toBe(want);
  });
});
