import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { HostToSpeech, SpeechToHost } from './protocol';

const dir = path.resolve(__dirname, '../../../../spec/vectors/speech');
const lines = (f: string) =>
  fs
    .readFileSync(path.join(dir, f), 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as unknown);

describe('speech protocol fixtures (shared with crates/qrtz-speech)', () => {
  it('every valid module line parses, and every message type has a fixture', () => {
    const v = lines('speech-to-host.jsonl');
    for (const l of v)
      expect(SpeechToHost.safeParse(l).success, JSON.stringify(l)).toBe(true);
    const types = new Set(v.map((l) => (l as { type: string }).type));
    expect([...types].sort()).toEqual(
      SpeechToHost.options.map((o) => o.shape.type.value).sort(),
    );
  });
  it('every host line parses and covers every type', () => {
    const v = lines('host-to-speech.jsonl');
    for (const l of v) expect(HostToSpeech.safeParse(l).success).toBe(true);
    expect(new Set(v.map((l) => (l as { type: string }).type)).size).toBe(
      HostToSpeech.options.length,
    );
  });
  it('rejects every invalid line', () => {
    for (const l of lines('invalid-speech-to-host.jsonl'))
      expect(SpeechToHost.safeParse(l).success, JSON.stringify(l)).toBe(false);
  });
});
