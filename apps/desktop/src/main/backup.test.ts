import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import AdmZip from 'adm-zip';
import { describe, expect, it } from 'vitest';
import { readBackup, writeBackup } from './backup';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-backup-'));

function make() {
  const dir = tmp();
  const db = path.join(dir, 'snap.db');
  fs.writeFileSync(db, 'SQLite format 3\0fake');
  const wav = path.join(dir, 'abc.wav');
  fs.writeFileSync(wav, Buffer.alloc(64, 7));
  const target = path.join(dir, 'out', 'b.zip');
  writeBackup({
    target,
    dbSnapshot: db,
    audioFiles: [wav],
    appVersion: 't',
    schemaVersion: 4,
    meetings: 1,
  });
  return { dir, target };
}

describe('backup zip', () => {
  it('round-trips with a verified manifest', () => {
    const { dir, target } = make();
    const r = readBackup({
      source: target,
      workDir: path.join(dir, 'w'),
      audioDir: path.join(dir, 'a'),
    });
    expect(r.manifest.files.map((f) => f.path)).toEqual([
      'db/qrtz.db',
      'audio/abc.wav',
    ]);
    expect(fs.readFileSync(r.audioMap['abc.wav']!)).toEqual(
      Buffer.alloc(64, 7),
    );
  });
  it('rejects tampered files, extra entries and non-zips', () => {
    const { dir, target } = make();
    const z = new AdmZip(target);
    z.updateFile('audio/abc.wav', Buffer.alloc(64, 8));
    const bad = path.join(dir, 'bad.zip');
    z.writeZip(bad);
    expect(() =>
      readBackup({ source: bad, workDir: dir, audioDir: dir }),
    ).toThrow(/integrity/);
    const z2 = new AdmZip(target);
    z2.addFile('../evil.sh', Buffer.from('x'));
    z2.writeZip(bad);
    expect(() =>
      readBackup({ source: bad, workDir: dir, audioDir: dir }),
    ).toThrow(/unexpected/);
    fs.writeFileSync(bad, 'nope');
    expect(() =>
      readBackup({ source: bad, workDir: dir, audioDir: dir }),
    ).toThrow(/Backup rejected/);
  });
});
