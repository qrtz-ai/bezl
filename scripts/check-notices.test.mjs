import { describe, expect, it } from 'vitest';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { check, REQUIRED } from './check-notices.mjs';

describe('check-notices', () => {
  it('passes on the repo as it is', () => {
    expect(check()).toEqual([]);
  });
  it('verify-bundle fails a bundle missing any notice', () => {
    const app = mkdtempSync(path.join(os.tmpdir(), 'cn-')) + '/qrtz.app';
    mkdirSync(`${app}/Contents/MacOS`, { recursive: true });
    mkdirSync(`${app}/Contents/Resources/licenses`, { recursive: true });
    writeFileSync(
      `${app}/Contents/Resources/THIRD_PARTY_NOTICES.md`,
      '# Third-party notices\n',
    );
    const r = spawnSync(
      'bash',
      [path.join(import.meta.dirname, 'release/verify-bundle.sh'), app],
      { encoding: 'utf8' },
    );
    expect(r.status).not.toBe(0);
    for (const dst of Object.keys(REQUIRED).filter(
      (d) => d !== 'THIRD_PARTY_NOTICES.md',
    ))
      expect(r.stderr).toContain(
        `Contents/Resources/${dst} is missing or empty`,
      );
    expect(r.stderr).not.toContain(
      'Contents/Resources/THIRD_PARTY_NOTICES.md is missing',
    );
  });
});
