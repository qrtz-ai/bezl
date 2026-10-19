// ADR 0206 (echo cycle 66): "Hey <dictionary name> <question>" gets a comma after the name and "?". The same cases as
// crates/qrtz-talk/tests/hardening66.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const entry = (term: string, aliases: string[] = []) => ({ term, aliases });
const dict = [
  entry('qrtz', ['cue arts']),
  entry('Jared'),
  entry('Priya', ['pria']),
  entry('Kubernetes'),
  entry('Teams'),
];

describe('ADR 0206 a dictionary name after hey before a question', () => {
  it.each([
    ['Hey Jared are you free tomorrow', 'Hey Jared, are you free tomorrow?'],
    [
      'Hey Priya can you send me the deck',
      'Hey Priya, can you send me the deck?',
    ],
    ['Hey Jared did you get my email', 'Hey Jared, did you get my email?'],
    [
      'Hey Priya what time does the store close',
      'Hey Priya, what time does the store close?',
    ],
    [
      "I'm outside. Hey Jared are you home",
      "I'm outside. Hey Jared, are you home?",
    ],
    [
      'hey priya are we still on for tonight',
      'Hey Priya, are we still on for tonight?',
    ],
    ['Hey Jared how was the trip', 'Hey Jared, how was the trip?'],
    [
      'Um, hey Priya could you take a look at this',
      'Hey Priya, could you take a look at this?',
    ],
    [
      'Notes new line hey Jared will you call me back',
      'Notes\nHey Jared, will you call me back?',
    ],
    [
      'Hey Jared where did you put the keys',
      'Hey Jared, where did you put the keys?',
    ],
    [
      'Hey Jared is she coming to the party',
      'Hey Jared, is she coming to the party?',
    ],
    ['Hey pria are you around today', 'Hey Priya, are you around today?'],
    ['Hey  Jared   are you free tomorrow', 'Hey Jared, are you free tomorrow?'],
    [
      'Hey Jared shall we start without them',
      'Hey Jared, shall we start without them?',
    ],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it.each([
    'Hey Sam are you free tomorrow',
    'Hey qrtz can you open my notes',
    'Hey Jared Smith are you free tomorrow',
    'Hey Jared did you a favor last week',
    'Hey Jared has it covered',
    'Hey Jared does it every week',
    'Hey Jared is that guy from the meetup',
    'Hey Priya is there a lot of traffic',
    'Hey Kubernetes is it worth learning',
    'Hey Priya can be late sometimes',
    'Hey Jared who is the new designer',
    'Hey Jared you did great today',
    'Hey Jared I think we can ship it',
    'Hey Jared are you free or should we wait',
    "Hey Jared can't you just wait",
    'So I said hey Jared are you coming',
    'Okay Jared can you send it over',
    'Okay Teams have you covered',
    'Hey Jared, are you free tomorrow',
    'Hey Jared are you',
    'Hey Jared are you free. Thanks',
  ])('keeps %s', (raw) => {
    expect(ruleCleanup(raw, dict)).toBe(`${raw}.`);
  });

  it('needs the dictionary, and never takes a question word for a name', () => {
    expect(ruleCleanup('Hey Jared are you free tomorrow', [])).toBe(
      'Hey Jared are you free tomorrow.',
    );
    const words = [entry('Will'), entry('Who'), entry('Do')];
    expect(ruleCleanup('Hey Will are you coming tonight', words)).toBe(
      'Hey Will are you coming tonight.',
    );
    expect(ruleCleanup('Hey will you send it', words)).toBe(
      'Hey, Will you send it?',
    );
  });
});
