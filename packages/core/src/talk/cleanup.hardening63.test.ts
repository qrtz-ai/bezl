// ADR 0203 (echo cycle 63): a question after a discourse opener ("So do you want to grab lunch") ends with "?"
// instead of "."; the agreeing ellipsis, exclamatory inversion, commands and anything unclear keep their ".". The
// same cases as crates/qrtz-talk/tests/hardening63.rs.
import { describe, expect, it } from 'vitest';
import { ruleCleanup } from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts'] }];

describe('ADR 0203 a question after a discourse opener', () => {
  it.each([
    'So do you want to grab lunch',
    'So, do you want to grab lunch',
    'Okay, can you send me the file',
    'Hey, can you take a look at this',
    'And did you hear back from them',
    'But do you have time today',
    'And are you coming with us',
    'Also can you send the slides',
    'Alright, are we ready to start',
    'So what time does the store close',
    'And how was the trip',
    'I like it. So do you want to grab lunch',
  ])('%s gets a question mark', (raw) => {
    expect(ruleCleanup(raw, dict)).toBe(`${raw}?`);
  });

  it.each([
    ['Okay can you send me the file', 'Okay, can you send me the file?'],
    ['OK can we move the meeting', 'OK, can we move the meeting?'],
    [
      'Hey are you free tomorrow afternoon',
      'Hey, are you free tomorrow afternoon?',
    ],
  ])('%s gets a question mark and (ADR 0204) its comma', (raw, want) => {
    expect(ruleCleanup(raw, dict)).toBe(want);
  });

  it('a dictionary term and a filler before the opener', () => {
    expect(ruleCleanup('Hey can you check cue arts', dict)).toBe(
      'Hey, can you check qrtz?',
    );
    expect(ruleCleanup('Um so do you want to grab lunch', dict)).toBe(
      'So do you want to grab lunch?',
    );
  });

  it.each([
    'So did we last year',
    'So do I at work',
    'So can we in the summer',
    'So would I if I could',
    'So are we at the moment',
    'So is it on weekends',
    'So have I many times',
    'So could you at that age',
    'So will they at the next one',
    'So did he after the game',
    'And so do I every day',
    'But so did we last year',
    'Well do I remember that day',
    'Well, was he surprised',
    'Oh do I love that song',
    'Oh, was it hot outside',
    'Boy do I need a vacation',
    'Man was I tired last night',
    'Now would you look at that',
    'And was he mad about it',
    'And do I miss it every day',
    'But were we surprised by that',
    'But can we still make it',
    'Okay do the dishes',
    'So do it now',
    'Okay is the store open',
    'Also did it again last night',
    'Okay can you send it we need it today',
    'Hey, should I bring anything',
    'Okay, have I got news for you',
    'But what do you know',
    'Alright, can I just say something',
    'And were we to lose the deal',
    'So, I was thinking we could go',
    'Hey, Sam, can you send it',
    'Okay so do you want to go',
    'So, so do you want to go',
  ])('%s keeps its period', (raw) => {
    expect(ruleCleanup(raw, dict)).toBe(`${raw}.`);
  });

  it('a short one gets no mark', () => {
    expect(ruleCleanup('So do you', dict)).toBe('So do you');
  });
});
