// QRT-279: sign-mac.sh refuses real dev stubs but not real binaries that merely contain the marker string.
import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const src = fs.readFileSync(
  path.join(import.meta.dirname, 'sign-mac.sh'),
  'utf8',
);
const fn = src.split('\n').find((l) => l.startsWith('is_dev_stub()'));
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-stub-'));
const check = (bytes) => {
  const f = path.join(dir, `b${Math.random()}`);
  fs.writeFileSync(f, bytes, { mode: 0o755 });
  return spawnSync('bash', ['-c', `${fn}\nis_dev_stub "$1"`, '_', f]).status;
};

describe('sign-mac.sh dev-stub check', () => {
  it('blocks the stub build.rs writes', () => {
    expect(
      check("#!/bin/sh\necho 'qrtz: dev stub (qrtz); run x' >&2\nexit 1\n"),
    ).toBe(0);
  });
  it('passes a Mach-O that contains the marker string', () => {
    const macho = Buffer.concat([
      Buffer.from([0xcf, 0xfa, 0xed, 0xfe]),
      Buffer.from('\0\0qrtz: dev stub\0'),
    ]);
    expect(check(macho)).not.toBe(0);
  });
  it('is called on every nested binary, never skipped', () => {
    expect(fn).toBeDefined();
    expect(src).toMatch(
      /if is_dev_stub "\$bin"; then echo "\$bin is a dev stub" >&2; exit 1; fi/,
    );
    expect(src).not.toMatch(/QRTZ_SKIP|SKIP_STUB/);
  });
});
