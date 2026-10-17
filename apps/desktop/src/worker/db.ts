import fs from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { getSchemaVersion, migrate, migrations } from '@qrtz/core';

/** SQLite's companion files: the unix VFS creates them with the main file's mode. */
const SIDECARS = ['-wal', '-shm', '-journal'];
const onDisk = (file: string) =>
  file !== '' && file !== ':memory:' && !file.startsWith('file:');

/**
 * N25: the DB holds every transcript, summary and note, so it is owner-only (0600) whatever the umask. The
 * file is created 0600 before SQLite opens it, because SQLite gives `-wal`, `-shm` and `-journal` the mode of
 * the main file: they follow without a chmod race. Files an older build left at the umask default are
 * chmod-ed. Best effort on files that are not ours to change (the open itself still decides).
 */
export function ownerOnlyDbFiles(file: string) {
  if (!onDisk(file) || process.platform === 'win32') return;
  try {
    fs.closeSync(fs.openSync(file, 'a', 0o600));
  } catch {
    return; // missing directory, read-only volume: let SQLite report it
  }
  for (const f of [file, ...SIDECARS.map((x) => file + x)])
    try {
      fs.chmodSync(f, 0o600);
    } catch {
      /* absent (sidecars usually are) or not ours */
    }
}

/**
 * A new, empty owner-only file for `VACUUM INTO` (backup snapshots, N26). Exclusive create: an existing path,
 * or a symlink planted in a shared temp dir, is removed first and never followed.
 */
export function freshOwnerOnlyFile(file: string) {
  fs.rmSync(file, { force: true });
  fs.closeSync(fs.openSync(file, 'wx', 0o600));
}

export function openDatabase(file: string) {
  ownerOnlyDbFiles(file);
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
