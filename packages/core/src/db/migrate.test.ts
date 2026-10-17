import { describe, expect, it } from 'vitest';
import { DatabaseSync } from 'node:sqlite';
import { migrate, migrations, type Migration } from './migrate';

describe('migrate', () => {
  it('applies in order and is idempotent', () => {
    const db = new DatabaseSync(':memory:');
    expect(migrate(db, migrations)).toMatchObject({
      from: 0,
      to: 4,
      applied: ['1_kv', '2_memo', '3_segment_vectors', '4_notes_templates'],
    });
    expect(migrate(db, migrations).applied).toEqual([]);
  });
  it('rolls back a failing migration', () => {
    const db = new DatabaseSync(':memory:');
    const ms: Migration[] = [
      ...migrations,
      { version: 5, name: 'bad', up: 'CREATE TABLE t(x); NOT SQL' },
    ];
    expect(() => migrate(db, ms)).toThrow(/5_bad/);
    expect(db.prepare('PRAGMA user_version').get()).toEqual({
      user_version: 4,
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
});
