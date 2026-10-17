/**
 * Hardening cycle 11 (N46, ADR 0038): search terms with an apostrophe or combining marks.
 * spec/stress/fts-queries.json is shared with crates/qrtz-storage/tests/hardening11.rs: the query builder must give
 * the same string on both runtimes, and a real FTS5 table with the index's tokenizer must return the same rows.
 */
import fs from 'node:fs';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { migrate, migrations } from '../db/migrate';
import { NoteRepo, toFtsQuery } from './repo';

interface Spec {
  tokenize: string;
  corpus: string[];
  cases: { input: string; query: string | null; matches: number[] }[];
}
const here = path.dirname(fileURLToPath(import.meta.url));
const spec = JSON.parse(
  fs.readFileSync(
    path.resolve(here, '../../../../spec/stress/fts-queries.json'),
    'utf8',
  ),
) as Spec;

describe('hardening 11: search keeps apostrophes and marks inside the term (N46)', () => {
  it('builds the spec query for every input', () => {
    for (const c of spec.cases)
      expect(toFtsQuery(c.input), c.input).toBe(c.query);
  });

  it('a real FTS5 table with the index tokenizer returns the spec rows', () => {
    const db = new DatabaseSync(':memory:');
    db.exec(
      `CREATE VIRTUAL TABLE f USING fts5(text, tokenize='${spec.tokenize}')`,
    );
    spec.corpus.forEach((t, i) =>
      db.prepare('INSERT INTO f(rowid, text) VALUES (?, ?)').run(i + 1, t),
    );
    for (const c of spec.cases) {
      const got = c.query
        ? (
            db
              .prepare('SELECT rowid FROM f WHERE f MATCH ? ORDER BY rowid')
              .all(c.query) as { rowid: number }[]
          ).map((r) => Number(r.rowid) - 1)
        : [];
      expect(got, c.input).toEqual(c.matches);
    }
    db.close();
  });

  // The list path (worker handlers) is pinned by the parity fixture query "don't forget".
  it("end to end: searching don't and O'Reilly's finds the meeting that said them", () => {
    const db = new DatabaseSync(':memory:');
    migrate(db, migrations);
    const repo = new NoteRepo(db);
    repo.upsertMeeting({
      id: 'm',
      title: 'Vendor sync',
      platform: 'zoom',
      startedAt: 1,
      status: 'done',
      participants: [],
    });
    repo.replaceSegments('m', 'live', [
      {
        id: 's',
        startMs: 0,
        endMs: 1,
        text: "Don't forget the O'Reilly contract.",
        source: 'live',
      },
    ]);
    for (const q of [
      "don't",
      "Don't forget",
      "O'Reilly",
      "O'Reilly's contract",
      'o’reilly',
    ])
      expect(
        repo.search(q).map((h) => h.meetingId),
        q,
      ).toEqual(['m']);
    db.close();
  });
});
