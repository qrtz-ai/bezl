import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { RUN_PREFIX, STALE_MS, runTmpDir, sweepStale } from './test-tmp.mjs';

describe('test temp dirs (N50)', () => {
  it('every test writes under the run dir, which the run removes', () => {
    const run = process.env.QRTZ_TEST_TMP;
    expect(run).toBeTruthy();
    expect(path.basename(run).startsWith(RUN_PREFIX)).toBe(true);
    expect(os.tmpdir()).toBe(run);
    const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-n50-'));
    expect(d.startsWith(run + path.sep)).toBe(true);
  });

  it('reuses the run dir when the config loads again', () => {
    const env = { QRTZ_TEST_TMP: process.env.QRTZ_TEST_TMP };
    expect(runTmpDir(env)).toBe(process.env.QRTZ_TEST_TMP);
  });

  it('sweeps only run dirs older than an hour, never the current one', () => {
    const parent = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-n50-sweep-'));
    const mk = (name, ageMs) => {
      const p = path.join(parent, name);
      fs.mkdirSync(p);
      fs.writeFileSync(path.join(p, 'f'), 'x');
      const t = (Date.now() - ageMs) / 1000;
      fs.utimesSync(p, t, t);
      return p;
    };
    const old = mk(`${RUN_PREFIX}old`, STALE_MS + 60_000);
    const fresh = mk(`${RUN_PREFIX}fresh`, 60_000);
    const keep = mk(`${RUN_PREFIX}keep`, STALE_MS * 3);
    const other = mk('qrtz-lc-other', STALE_MS * 3);
    const removed = sweepStale(parent, keep);
    expect(removed).toEqual([old]);
    expect(fs.existsSync(old)).toBe(false);
    expect(fs.existsSync(fresh)).toBe(true);
    expect(fs.existsSync(keep)).toBe(true);
    expect(fs.existsSync(other)).toBe(true);
    expect(sweepStale(path.join(parent, 'missing'), keep)).toEqual([]);
  });
});
