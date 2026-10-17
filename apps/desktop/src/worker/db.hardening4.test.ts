/** Hardening cycle 4 (N19): a database from a newer app is refused before anything writes to it. */
import { afterEach, describe, expect, it } from 'vitest';
import { DatabaseSync } from 'node:sqlite';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { openDatabase } from './db';

const dirs: string[] = [];
const tmp = () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-h4-'));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

describe('hardening 4: a database from a newer app (N19)', () => {
  it('is refused before anything writes to it: journal mode, schema and rows unchanged', () => {
    const d = tmp();
    const file = path.join(d, 'qrtz.db');
    const raw = new DatabaseSync(file);
    raw.exec(
      `PRAGMA journal_mode = DELETE; CREATE TABLE future (x); INSERT INTO future VALUES (42); PRAGMA user_version = 99;`,
    );
    raw.close();
    const before = fs.readFileSync(file);
    expect(() => openDatabase(file)).toThrow(/newer than this app/);
    const after = fs.readFileSync(file);
    expect(after.equals(before)).toBe(true);
    expect(fs.existsSync(`${file}-wal`)).toBe(false);
    const check = new DatabaseSync(file);
    expect(
      (check.prepare('PRAGMA journal_mode').get() as { journal_mode: string })
        .journal_mode,
    ).toBe('delete');
    expect(check.prepare('SELECT x FROM future').get()).toEqual({ x: 42 });
    check.close();
  });

  it('an older DB (v2) still migrates forward on open', () => {
    const d = tmp();
    const file = path.join(d, 'qrtz.db');
    const db = openDatabase(file);
    const latest = (
      db.prepare('PRAGMA user_version').get() as { user_version: number }
    ).user_version;
    db.close();
    expect(latest).toBeGreaterThanOrEqual(5);
    const again = openDatabase(file);
    expect(
      (again.prepare('PRAGMA journal_mode').get() as { journal_mode: string })
        .journal_mode,
    ).toBe('wal');
    again.close();
  });
});
