import { describe, expect, it } from 'vitest';
import {
  applyDictionary,
  cleanupDictation,
  plausibleCleanup,
  ruleCleanup,
} from './cleanup';

const dict = [{ term: 'qrtz', aliases: ['cue arts', 'quartz'] }];

describe('talk cleanup', () => {
  it('rules: fillers, stutters, capitalization, terminal punctuation, dictionary', () => {
    expect(
      ruleCleanup(
        'um so I I think we should ship cue arts talk on friday',
        dict,
      ),
    ).toBe('So I think we should ship qrtz talk on friday.');
    expect(ruleCleanup('uh, hello there , how are you')).toBe(
      'Hello there, how are you.',
    );
    expect(
      ruleCleanup('first item new line second item new paragraph done'),
    ).toBe('First item\nSecond item\n\nDone.');
    expect(ruleCleanup('is it ready?')).toBe('Is it ready?');
    expect(ruleCleanup('i think i can')).toBe('I think I can.');
    expect(ruleCleanup('')).toBe('');
  });

  it('dictionary keeps the user spelling, case-insensitively and across hyphens', () => {
    expect(applyDictionary('Cue-Arts and QUARTZ', dict)).toBe('qrtz and qrtz');
    expect(applyDictionary('quartzite stays', dict)).toBe('quartzite stays');
  });

  it('LLM result wins when fast and plausible', async () => {
    const r = await cleanupDictation({
      raw: 'um hello world',
      dictionary: dict,
      enabled: true,
      llm: async () => 'Hello, world! cue arts',
      timeoutMs: 500,
    });
    expect(r).toMatchObject({ method: 'llm', text: 'Hello, world! qrtz' });
  });

  it('falls back to rules on timeout and aborts the request', async () => {
    let aborted = false;
    const r = await cleanupDictation({
      raw: 'um hello world',
      dictionary: [],
      enabled: true,
      llm: (_m, signal) =>
        new Promise((_res, rej) => {
          signal.addEventListener('abort', () => {
            aborted = true;
            rej(new Error('aborted'));
          });
        }),
      timeoutMs: 30,
    });
    expect(r).toMatchObject({
      method: 'rules',
      text: 'Hello world.',
      fallbackReason: 'timeout',
    });
    expect(aborted).toBe(true);
  });

  it('falls back on errors and implausible replies; off leaves words alone', async () => {
    const base = {
      raw: 'hello world',
      dictionary: [],
      enabled: true,
      timeoutMs: 200,
    };
    expect(
      (
        await cleanupDictation({
          ...base,
          llm: async () => Promise.reject(new Error('402')),
        })
      ).method,
    ).toBe('rules');
    expect(
      (
        await cleanupDictation({
          ...base,
          llm: async () => 'Sure! Here is the cleaned text: Hello world.',
        })
      ).method,
    ).toBe('rules');
    expect(
      await cleanupDictation({ ...base, raw: 'um hello', enabled: false }),
    ).toMatchObject({ method: 'off', text: 'um hello' });
    expect(await cleanupDictation({ ...base, llm: null })).toMatchObject({
      method: 'rules',
    });
    expect(plausibleCleanup('hi', 'x'.repeat(200))).toBe(false);
  });
});
