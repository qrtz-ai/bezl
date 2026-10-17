import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { hashFile, readBackup, writeBackup } from './backup';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-backup-'));

async function make(audioBytes = 64) {
  const dir = tmp();
  const db = path.join(dir, 'snap.db');
  fs.writeFileSync(db, 'SQLite format 3\0fake');
  const wav = path.join(dir, 'abc.wav');
  fs.writeFileSync(wav, Buffer.alloc(audioBytes, 7));
  const target = path.join(dir, 'out', 'b.zip');
  await writeBackup({
    target,
    dbSnapshot: db,
    audioFiles: [wav],
    appVersion: 't',
    schemaVersion: 4,
    meetings: 1,
  });
  return { dir, target, wav };
}

describe('streaming backup zip', () => {
  it('round-trips with a verified manifest', async () => {
    const { dir, target } = await make();
    const r = await readBackup({
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

  it('streams a large recording with bounded memory', async () => {
    const big = 38 * 8 * 1024 * 1024;
    const { dir, target, wav } = await make(0);
    const fd = fs.openSync(wav, 'w');
    const chunk = Buffer.alloc(8 * 1024 * 1024, 3);
    for (let w = 0; w < big; w += chunk.length) fs.writeSync(fd, chunk);
    fs.closeSync(fd);
    const before = process.memoryUsage().rss;
    let peak = before;
    const t = setInterval(
      () => (peak = Math.max(peak, process.memoryUsage().rss)),
      20,
    );
    await writeBackup({
      target,
      dbSnapshot: path.join(dir, 'snap.db'),
      audioFiles: [wav],
      appVersion: 't',
      schemaVersion: 4,
      meetings: 1,
    });
    const r = await readBackup({
      source: target,
      workDir: path.join(dir, 'w'),
      audioDir: path.join(dir, 'a'),
    });
    clearInterval(t);
    expect((await hashFile(r.audioMap['abc.wav']!)).bytes).toBe(big);
    expect(peak - before).toBeLessThan(200 * 1024 * 1024);
    fs.rmSync(dir, { recursive: true, force: true });
  }, 60_000);

  it('rejects tampered data, extra entries and non-zips, leaving nothing behind', async () => {
    const { dir, target } = await make(4096);
    const buf = fs.readFileSync(target);
    const i = buf.indexOf(Buffer.alloc(64, 7)) + 10;
    buf[i] = 9;
    const bad = path.join(dir, 'bad.zip');
    fs.writeFileSync(bad, buf);
    const audioDir = path.join(dir, 'a2');
    await expect(
      readBackup({ source: bad, workDir: path.join(dir, 'w2'), audioDir }),
    ).rejects.toThrow(/integrity/);
    expect(fs.readdirSync(audioDir)).toEqual([]);
    fs.writeFileSync(bad, 'nope');
    await expect(
      readBackup({ source: bad, workDir: dir, audioDir }),
    ).rejects.toThrow(/Backup rejected/);
  });
});
