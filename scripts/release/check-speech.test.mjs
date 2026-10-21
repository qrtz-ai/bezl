// check-speech.mjs (QRT-306): {"speech":"none"} or exactly five valid pins; anything else fails.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { generateKeyPairSync } from 'node:crypto';
import { checkSpeech } from './check-speech.mjs';

const raw = () =>
  generateKeyPairSync('ed25519')
    .publicKey.export({ format: 'der', type: 'spki' })
    .subarray(12)
    .toString('hex');
const good = () => ({
  QRTZ_SPEECH_URL: 'https://speech.example.com/v1',
  QRTZ_SPEECH_KEY_ID: 'k1',
  QRTZ_SPEECH_KEY: raw(),
  QRTZ_SPEECH_ROOT_KEY_ID: 'root1',
  QRTZ_SPEECH_ROOT_KEY: raw(),
});
const j = (o) => JSON.stringify(o);

describe('check-speech', () => {
  it('accepts the committed speech.json ({"speech":"none"})', () => {
    const f = path.join(
      import.meta.dirname,
      '../../apps/desktop/src-tauri/speech.json',
    );
    expect(checkSpeech(readFileSync(f, 'utf8'))).toEqual({ mode: 'none' });
  });
  it('accepts exactly five valid pins', () => {
    expect(checkSpeech(j(good())).mode).toBe('pinned');
  });
  it('rejects other "speech" values and extra keys next to it', () => {
    expect(() => checkSpeech(j({ speech: 'off' }))).toThrow();
    expect(() => checkSpeech(j({ speech: 'none', x: 1 }))).toThrow();
    expect(() => checkSpeech(j({}))).toThrow(/missing/);
  });
  it('rejects partial and extra keys', () => {
    const p = good();
    delete p.QRTZ_SPEECH_ROOT_KEY;
    expect(() => checkSpeech(j(p))).toThrow(/missing QRTZ_SPEECH_ROOT_KEY/);
    expect(() => checkSpeech(j({ ...good(), extra: 'x' }))).toThrow(
      /unexpected/,
    );
    expect(() => checkSpeech(j({ ...good(), speech: 'none' }))).toThrow(
      /unexpected/,
    );
  });
  it('rejects malformed values', () => {
    for (const [k, v] of [
      ['QRTZ_SPEECH_URL', 'http://speech.example.com'],
      ['QRTZ_SPEECH_URL', 'https://u:p@speech.example.com'],
      ['QRTZ_SPEECH_URL', 'https://speech.example.com/?t=1'],
      ['QRTZ_SPEECH_URL', 'not a url'],
      ['QRTZ_SPEECH_KEY_ID', ''],
      ['QRTZ_SPEECH_KEY_ID', 'a b'],
      ['QRTZ_SPEECH_KEY', 'ab'],
      ['QRTZ_SPEECH_KEY', '0'.repeat(64)],
      ['QRTZ_SPEECH_ROOT_KEY', 'G'.repeat(64)],
      ['QRTZ_SPEECH_ROOT_KEY', 5],
    ])
      expect(
        () => checkSpeech(j({ ...good(), [k]: v })),
        `${k}=${v}`,
      ).toThrow();
    const g = good();
    expect(() =>
      checkSpeech(j({ ...g, QRTZ_SPEECH_ROOT_KEY: g.QRTZ_SPEECH_KEY })),
    ).toThrow(/differ/);
    expect(() => checkSpeech('{')).toThrow(/json/);
    expect(() => checkSpeech('[]')).toThrow(/object/);
  });
});
