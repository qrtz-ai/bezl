import { describe, expect, it } from 'vitest';
import { entryMeta, parseTerm } from './format';

describe('talk format', () => {
  it('parses dictionary input', () => {
    expect(parseTerm('qrtz = cue arts, quartz')).toEqual({
      term: 'qrtz',
      aliases: ['cue arts', 'quartz'],
    });
    expect(parseTerm('Grimes')).toEqual({ term: 'Grimes', aliases: [] });
    expect(parseTerm('  = x')).toBeNull();
  });
  it('meta line shows app, engine (fallback) and key-up → insert latency', () => {
    const m = entryMeta({
      id: 'a',
      at: 0,
      raw: 'r',
      text: 't',
      cleanup: 'rules',
      engine: 'on-device',
      failover: true,
      insert: 'paste',
      inserted: true,
      app: 'Notes',
      durationMs: 1000,
      latency: { insertedMs: 412.4, finalMs: 100 },
    });
    expect(m).toMatch(/Notes · on-device \(fallback\) · 412 ms$/);
  });
});
