/** Hardening cycle 5 (N26): a backup archive is every transcript and recording, so it is written owner-only. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { readBackup, writeBackup } from './backup';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h5-bk-'));
  dirs.push(d);
  return d;
};
let umask = 0;
beforeEach(() => {
  umask = process.umask(0o022);
});
afterEach(() => {
  process.umask(umask);
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

describe.runIf(process.platform !== 'win32')(
  'hardening 5: owner-only backup archive (N26)',
  () => {
    it('is 0600 under umask 022, even over a 0644 .partial an interrupted export left, and still restores', async () => {
      const d = tmp();
      const snap = path.join(d, 'snap.db');
      fs.writeFileSync(snap, 'not really sqlite, the zip does not care');
      const wav = path.join(d, 'm1.wav');
      fs.writeFileSync(wav, Buffer.alloc(64));
      const target = path.join(d, 'out', 'qrtz-backup.zip');
      fs.mkdirSync(path.dirname(target));
      fs.writeFileSync(`${target}.partial`, 'stale', { mode: 0o644 });
      fs.chmodSync(`${target}.partial`, 0o644);
      await writeBackup({
        target,
        dbSnapshot: snap,
        audioFiles: [wav],
        appVersion: 'test',
        schemaVersion: 5,
        meetings: 1,
        now: 1,
      });
      expect(fs.statSync(target).mode & 0o777).toBe(0o600);
      expect(fs.existsSync(`${target}.partial`)).toBe(false);
      const r = await readBackup({
        source: target,
        workDir: path.join(d, 'work'),
        audioDir: path.join(d, 'rec'),
      });
      expect(fs.readFileSync(r.dbPath, 'utf8')).toBe(
        'not really sqlite, the zip does not care',
      );
    });
  },
);
