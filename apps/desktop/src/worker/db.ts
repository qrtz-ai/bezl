import { DatabaseSync } from 'node:sqlite';
import { getSchemaVersion, migrate, migrations } from '@qrtz/core';

export function openDatabase(file: string) {
  const db = new DatabaseSync(file);
  db.exec(
    'PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;',
  );
  migrate(db, migrations);
  return db;
}

export function dbHealth(db: DatabaseSync) {
  let fts5 = false;
  try {
    db.exec(
      'CREATE VIRTUAL TABLE IF NOT EXISTS temp._fts_probe USING fts5(x); DROP TABLE temp._fts_probe;',
    );
    fts5 = true;
  } catch {
    fts5 = false;
  }
  return { ok: true, schemaVersion: getSchemaVersion(db), fts5 };
}
