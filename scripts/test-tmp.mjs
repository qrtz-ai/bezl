// One private temp dir per `vitest run` (N50, ADR 0040).
//
// Tests make their scratch dirs with `fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-…'))`. Many never removed
// them, so every run left about 70 dirs on the shared box disk (9.7k, 6.8 GB had piled up). Instead of relying on
// each test's cleanup, the config points TMPDIR at one run dir and this global setup removes it at the end. A run
// that was killed leaves its dir behind; the next run sweeps any `qrtz-vitest-*` dir older than an hour, so two
// runs going at once never remove each other's.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const RUN_PREFIX = 'qrtz-vitest-';
export const STALE_MS = 60 * 60 * 1000;

/** The run dir: reused when the config is loaded again in the same process (watch mode). */
export function runTmpDir(env = process.env) {
  if (env.QRTZ_TEST_TMP && fs.existsSync(env.QRTZ_TEST_TMP))
    return env.QRTZ_TEST_TMP;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), RUN_PREFIX));
  env.QRTZ_TEST_TMP = dir;
  return dir;
}

/** Remove `qrtz-vitest-*` dirs in `parent` last modified before `now - STALE_MS`, except `keep`. */
export function sweepStale(parent, keep, now = Date.now()) {
  const removed = [];
  let names = [];
  try {
    names = fs.readdirSync(parent);
  } catch {
    return removed;
  }
  for (const name of names) {
    if (!name.startsWith(RUN_PREFIX)) continue;
    const full = path.join(parent, name);
    if (full === keep) continue;
    try {
      const st = fs.lstatSync(full);
      if (!st.isDirectory() || now - st.mtimeMs < STALE_MS) continue;
      fs.rmSync(full, { recursive: true, force: true });
      removed.push(full);
    } catch {
      // Another run removed it first, or it is not ours to read: leave it.
    }
  }
  return removed;
}

/** vitest globalSetup: sweep old run dirs now, remove this run's dir at teardown. */
export default function setup() {
  const dir = process.env.QRTZ_TEST_TMP;
  if (dir) sweepStale(path.dirname(dir), dir);
  return () => {
    if (dir) fs.rmSync(dir, { recursive: true, force: true });
  };
}
