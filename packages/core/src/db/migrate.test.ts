import { describe, expect, it } from 'vitest';
import { DatabaseSync } from 'node:sqlite';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { migrate, migrations, type Migration } from './migrate';

describe('migrate', () => {
  it('applies in order and is idempotent', () => {
    const db = new DatabaseSync(':memory:');
    expect(migrate(db, migrations)).toMatchObject({
      from: 0,
      to: 6,
      applied: [
        '1_kv',
        '2_memo',
        '3_segment_vectors',
        '4_notes_templates',
        '5_recording_lifecycle',
        '6_calendar',
      ],
    });
    expect(migrate(db, migrations).applied).toEqual([]);
  });
  it('rolls back a failing migration', () => {
    const db = new DatabaseSync(':memory:');
    const ms: Migration[] = [
      ...migrations,
      { version: 7, name: 'bad', up: 'CREATE TABLE t(x); NOT SQL' },
    ];
    expect(() => migrate(db, ms)).toThrow(/7_bad/);
    expect(db.prepare('PRAGMA user_version').get()).toEqual({
      user_version: 6,
    });
    expect(
      db.prepare("SELECT name FROM sqlite_master WHERE name='t'").get(),
    ).toBeUndefined();
  });
  it('rejects gaps and newer databases', () => {
    const db = new DatabaseSync(':memory:');
    expect(() => migrate(db, [{ version: 2, name: 'x', up: '' }])).toThrow(
      /contiguous/,
    );
    db.exec('PRAGMA user_version = 9');
    expect(() => migrate(db, migrations)).toThrow(/newer/);
  });
  it('has FTS5', () => {
    const db = new DatabaseSync(':memory:');
    db.exec('CREATE VIRTUAL TABLE f USING fts5(x)');
  });
  it('matches the shared migration manifest (spec/schema/migrations.json, also checked by Rust)', () => {
    const root = fileURLToPath(new URL('../../../../', import.meta.url));
    const manifest = JSON.parse(
      readFileSync(`${root}spec/schema/migrations.json`, 'utf8'),
    ) as {
      migrations: {
        version: number;
        name: string;
        file: string;
        sha256: string;
      }[];
    };
    expect(migrations.map((m) => [m.version, m.name])).toEqual(
      manifest.migrations.map((m) => [m.version, m.name]),
    );
    for (const [i, m] of manifest.migrations.entries()) {
      const text = readFileSync(`${root}${m.file}`, 'utf8');
      // The ?raw import is the file byte-for-byte, and a shipped file never changes.
      expect(migrations[i]!.up).toBe(text);
      expect(createHash('sha256').update(text).digest('hex')).toBe(m.sha256);
    }
  });
});
