/**
 * Hardening cycle 5: the DB and its backup snapshot are owner-only whatever the umask (N25, N26). SQLite gives
 * `-wal` / `-shm` the main file's mode, so creating the main file 0600 is what covers them.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { DatabaseSync } from 'node:sqlite';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const posix = process.platform !== 'win32';
const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h5-'));
  dirs.push(d);
  return d;
};
let umask = 0;
beforeEach(() => {
  umask = process.umask(0o022); // a permissive umask: what most macOS / Linux sessions run with
});
afterEach(() => {
  process.umask(umask);
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});
const mode = (f: string) => fs.statSync(f).mode & 0o777;

describe.runIf(posix)('hardening 5: owner-only database files (N25)', () => {
  it('a new DB and the -wal / -shm SQLite creates for it are 0600 under umask 022', () => {
    const file = path.join(tmp(), 'qrtz.db');
    const db = openDatabase(file);
    db.exec(
      "INSERT INTO meetings (id, title, platform, started_at, status) VALUES ('m1', 't', 'zoom', 1, 'done')",
    );
    expect(mode(file)).toBe(0o600);
    expect(mode(`${file}-wal`)).toBe(0o600);
    expect(mode(`${file}-shm`)).toBe(0o600);
    db.close();
  });

  it('a DB and a crash-left -wal an older build created 0644 are tightened on open, rows intact', () => {
    const file = path.join(tmp(), 'qrtz.db');
    const old = openDatabase(file);
    old.exec(
      "INSERT INTO meetings (id, title, platform, started_at, status) VALUES ('m1', 't', 'zoom', 1, 'done')",
    );
    old.close();
    fs.chmodSync(file, 0o644);
    fs.writeFileSync(`${file}-wal`, '', { mode: 0o644 });
    fs.chmodSync(`${file}-wal`, 0o644);
    const db = openDatabase(file);
    expect(mode(file)).toBe(0o600);
    expect(mode(`${file}-wal`)).toBe(0o600);
    expect(db.prepare('SELECT id FROM meetings').all()).toEqual([{ id: 'm1' }]);
    db.close();
  });

  it('an in-memory DB is untouched (no stray file named :memory:)', () => {
    const cwd = process.cwd();
    const d = tmp();
    process.chdir(d);
    try {
      openDatabase(':memory:').close();
      expect(fs.readdirSync(d)).toEqual([]);
    } finally {
      process.chdir(cwd);
    }
  });
});

describe.runIf(posix)('hardening 5: the backup snapshot (N26)', () => {
  it('is 0600 and never written through a symlink planted at its path', () => {
    const d = tmp();
    const db = openDatabase(path.join(d, 'qrtz.db'));
    const h = createWorkerHandlers(db, { fetch: () => Promise.reject() });
    const victim = path.join(d, 'victim.txt');
    fs.writeFileSync(victim, 'keep me');
    const snap = path.join(d, 'snap.db');
    fs.symlinkSync(victim, snap);
    h['data.snapshot']({ path: snap });
    expect(fs.readFileSync(victim, 'utf8')).toBe('keep me');
    expect(fs.lstatSync(snap).isSymbolicLink()).toBe(false);
    expect(mode(snap)).toBe(0o600);
    const copy = new DatabaseSync(snap, { readOnly: true });
    expect(
      (copy.prepare('PRAGMA user_version').get() as { user_version: number })
        .user_version,
    ).toBeGreaterThanOrEqual(5);
    copy.close();
    db.close();
  });
});
