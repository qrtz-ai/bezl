// ADR 0208 (echo cycle 68): "Hey <dictionary name> did you <listed verb> ..." is a question (comma after the name,
// "?"); "did you" + an object or complement stays a statement. The same cases as crates/qrtz-talk/tests/hardening68.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const entry = (term: string, aliases: string[] = []) => ({ term, aliases });
const dict = [
  entry('qrtz', ['cue arts']),
  entry('Jared'),
  entry('Priya', ['pria']),
];

describe('ADR 0208 hey name did you and a listed verb', () => {
  it.each([
    [
      'Hey Jared did you finish the report',
      'Hey Jared, did you finish the report?',
    ],
    [
      'hey priya did you hear back from the landlord',
      'Hey Priya, did you hear back from the landlord?',
    ],
    [
      'I sent the draft last night. Hey Jared did you read it yet',
      'I sent the draft last night. Hey Jared, did you read it yet?',
    ],
    [
      'Hey pria did you book the flights for Friday',
      'Hey Priya, did you book the flights for Friday?',
    ],
    [
      'Hey Jared did you try the new build',
      'Hey Jared, did you try the new build?',
    ],
    [
      'Hey Jared did you sign the lease yet',
      'Hey Jared, did you sign the lease yet?',
    ],
    [
      'Hey Jared did you pick up the keys',
      'Hey Jared, did you pick up the keys?',
    ],
    [
      'Hey Jared did you merge the pull request',
      'Hey Jared, did you merge the pull request?',
    ],
    [
      'Hey Jared did you eat lunch already',
      'Hey Jared, did you eat lunch already?',
    ],
    [
      'Hey Priya did you email the client',
      'Hey Priya, did you email the client?',
    ],
    ['Hey Priya did you finish', 'Hey Priya, did you finish?'],
    [
      'Notes new line hey Jared did you ship the fix',
      'Notes\nHey Jared, did you ship the fix?',
    ],
  ])('%s', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it.each([
    'Hey Jared did you a solid last week',
    'Hey Jared did you a favor last week',
    'Hey Jared did you good with that advice',
    'Hey Jared did you wrong',
    'Hey Priya did you proud at the recital',
    'Hey Jared did you justice in that review',
    'Hey Jared did you dirty in that meeting',
    'Hey Jared did you like a pro on that deal',
    'Hey Priya did you a good turn yesterday',
    'Hey Priya did you right by fixing it',
    'Hey Jared did you no harm by staying',
    'Hey Priya did you well in the end',
    'Hey Priya did you credit in the writeup',
    'Hey Sam did you finish the report',
    'Okay Jared did you finish the report',
    'Hey Jared, did you finish the report',
    'Hey Jared did you ever finish the book',
    'So I said hey Jared did you finish the report',
    'Hey Jared did you finish the report that I sent',
    'Hey Jared did you finish the report. Thanks',
  ])('keeps %s', (raw) => {
    expect(ruleCleanup(raw, dict)).toBe(`${raw}.`);
  });

  it('never takes a term spelled like a listed verb for a name', () => {
    const words = [entry('Book'), entry('Ship')];
    expect(ruleCleanup('Hey Book did you finish the report', words)).toBe(
      'Hey Book did you finish the report.',
    );
    expect(ruleCleanup('Hey Ship did you read the notes', words)).toBe(
      'Hey Ship did you read the notes.',
    );
  });
});
