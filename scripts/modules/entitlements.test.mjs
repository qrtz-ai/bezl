// ADR 0185 (P3.4): mac-gate's e6 entitlement rule (scripts/modules/entitlements.sh) gives the same verdict as the
// app's runtime parser (echo_updates.rs, tested on the same vectors) on every vector in spec/vectors/echo-entitlements.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';

const dir = path.join(
  import.meta.dirname,
  '../../spec/vectors/echo-entitlements',
);
const lib = path.join(import.meta.dirname, 'entitlements.sh');

const shell = (fn, file) => {
  try {
    return execFileSync('bash', ['-c', `. "$1"; ${fn} <"$2"`, 'x', lib, file], {
      encoding: 'utf8',
    });
  } catch {
    return null;
  }
};

describe('scripts/modules/entitlements.sh', () => {
  const vectors = fs.readdirSync(dir).filter((f) => f.endsWith('.xml'));
  it('has vectors', () => expect(vectors.length).toBeGreaterThanOrEqual(6));
  for (const v of vectors) {
    it(`${v}: ${v.startsWith('ok-') ? 'microphone only' : 'refused'}`, () => {
      const ok = shell('entitlements_are_mic_only', path.join(dir, v)) !== null;
      expect(ok).toBe(v.startsWith('ok-'));
    });
  }
  it('lists what a plist grants, false values left out', () => {
    expect(
      shell(
        'granted_entitlements',
        path.join(dir, 'bad-library-validation.xml'),
      ),
    ).toBe(
      'com.apple.security.cs.disable-library-validation\ncom.apple.security.device.audio-input\n',
    );
    expect(
      shell('granted_entitlements', path.join(dir, 'ok-mic-camera-false.xml')),
    ).toBe('com.apple.security.device.audio-input\n');
  });
});
