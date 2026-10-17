/** Minimal synchronous SQL surface; satisfied by node:sqlite DatabaseSync. */
export interface SqlDb {
  exec(sql: string): void;
  prepare(sql: string): {
    get(...params: unknown[]): unknown;
    all(...params: unknown[]): unknown[];
    run(...params: unknown[]): unknown;
  };
}

export interface Migration {
  version: number;
  name: string;
  up: string | ((db: SqlDb) => void);
}

export function getSchemaVersion(db: SqlDb): number {
  const row = db.prepare('PRAGMA user_version').get() as {
    user_version: number;
  };
  return row.user_version;
}

/**
 * Applies pending migrations in order, each in its own transaction.
 * Idempotent: re-running is a no-op. Refuses to run if the DB is newer than the code.
 */
export function migrate(
  db: SqlDb,
  migrations: readonly Migration[],
): { from: number; to: number; applied: string[] } {
  const sorted = [...migrations].sort((a, b) => a.version - b.version);
  sorted.forEach((m, i) => {
    if (m.version !== i + 1)
      throw new Error(
        `Migrations must be contiguous from 1; got ${m.version} at #${i + 1}`,
      );
  });
  const from = getSchemaVersion(db);
  const latest = sorted.at(-1)?.version ?? 0;
  if (from > latest)
    throw new Error(
      `Database schema v${from} is newer than this app (v${latest})`,
    );
  const applied: string[] = [];
  for (const m of sorted) {
    if (m.version <= from) continue;
    db.exec('BEGIN IMMEDIATE');
    try {
      if (typeof m.up === 'string') db.exec(m.up);
      else m.up(db);
      db.exec(`PRAGMA user_version = ${m.version}`);
      db.exec('COMMIT');
      applied.push(`${m.version}_${m.name}`);
    } catch (err) {
      db.exec('ROLLBACK');
      throw new Error(
        `Migration ${m.version}_${m.name} failed: ${(err as Error).message}`,
        { cause: err },
      );
    }
  }
  return { from, to: getSchemaVersion(db), applied };
}

import { noteMigrations } from '../note/schema';

/** App migrations. Append only; never edit a shipped migration. */
export const migrations: readonly Migration[] = [
  {
    version: 1,
    name: 'kv',
    up: `CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);`,
  },
  ...noteMigrations,
];
