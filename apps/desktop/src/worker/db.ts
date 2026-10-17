import { DatabaseSync } from 'node:sqlite';
import { getSchemaVersion, migrate, migrations } from '@qrtz/core';

export function openDatabase(file: string) {
  const db = new DatabaseSync(file);
  try {
    // Connection-only pragmas first, then refuse a database from a newer app before anything writes to the
    // file: switching to WAL rewrites the header (N19). migrate() re-checks and applies what is pending.
    db.exec('PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;');
    const latest = Math.max(0, ...migrations.map((m) => m.version));
    const v = getSchemaVersion(db);
    if (v > latest)
      throw new Error(
        `Database schema v${v} is newer than this app (v${latest})`,
      );
    db.exec('PRAGMA journal_mode = WAL;');
    migrate(db, migrations);
    return db;
  } catch (e) {
    db.close();
    throw e;
  }
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
