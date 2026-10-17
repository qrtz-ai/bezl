/**
 * Electron <-> Rust database cross-open (Tauri migration, docs/rust-core-api.md). The ops in
 * spec/fixtures/db/seed.json are applied through the real worker handlers/NoteRepo; the resulting dump must equal
 * expected.json, and the same dump must come out of electron.sqlite (made here) and rust.sqlite (made by
 * crates/qrtz-storage/tests/parity.rs). The Rust test checks the mirror image.
 *
 * TS is the oracle: `QRTZ_FIXTURES_UPDATE=1` rewrites expected.json and electron.sqlite from this implementation;
 * then regenerate rust.sqlite with `QRTZ_FIXTURES_UPDATE=1 cargo test -p qrtz-storage --test parity`.
 */
import {
  copyFileSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { DatabaseSync } from 'node:sqlite';
import { afterAll, describe, expect, it } from 'vitest';
import { NoteRepo, toFtsQuery, type FetchLike } from '@qrtz/core';
import { openDatabase } from './db';
import { createWorkerHandlers } from './handlers';

const DIR = fileURLToPath(
  new URL('../../../../spec/fixtures/db/', import.meta.url),
);
const UPDATE = process.env.QRTZ_FIXTURES_UPDATE === '1';
const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};

type Op = { op: string } & Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
interface Seed {
  ops: Op[];
  queries: {
    list: { query?: string; limit?: number; offset?: number }[];
    details: string[];
    searchSegments: string[];
    missingVectors: { model: string; limit: number; meetingId?: string }[];
    vectorSources: { model: string; meetingId?: string }[];
    pipelines: string[];
    notes: string[];
    ftsQueries: string[];
  };
}
const seed = JSON.parse(readFileSync(join(DIR, 'seed.json'), 'utf8')) as Seed;
const plain = (v: unknown) => JSON.parse(JSON.stringify(v)) as unknown;
const hex = (v: Uint8Array) => Buffer.from(v).toString('hex');
const handlers = (db: DatabaseSync) =>
  createWorkerHandlers(db, { fetch: noFetch });

function rowid(
  db: DatabaseSync,
  meetingId: string,
  source: string,
  id: string,
): number {
  const r = db
    .prepare(
      'SELECT rowid AS r FROM segments WHERE meeting_id=? AND source=? AND id=?',
    )
    .get(meetingId, source, id) as { r: number } | undefined;
  if (!r) throw new Error(`seed: no segment ${meetingId}/${source}/${id}`);
  return Number(r.r);
}

/** Applies the seed; returns the results of ops that report one. */
function apply(db: DatabaseSync, ops: Op[]): unknown[] {
  const h = handlers(db);
  const repo = new NoteRepo(db);
  const out: unknown[] = [];
  for (const o of ops) {
    switch (o.op) {
      case 'upsertMeeting':
        repo.upsertMeeting(o.meeting);
        break;
      case 'setTemplate':
        repo.setTemplate(o.id, o.template);
        break;
      case 'setStatus':
        repo.setStatus(o.id, o.status);
        break;
      case 'replaceSegments':
        repo.replaceSegments(o.meetingId, o.source, o.segments);
        break;
      case 'appendSegment':
        repo.appendSegment(o.meetingId, o.segment);
        break;
      case 'renameSpeaker':
        out.push(repo.renameSpeaker(o.meetingId, o.from, o.to));
        break;
      case 'addNote':
        repo.addNote(o.meetingId, o.note);
        break;
      case 'updateNote':
        out.push(repo.updateNote(o.id, o.text));
        break;
      case 'deleteNote':
        out.push(repo.deleteNote(o.id));
        break;
      case 'saveSummary':
        repo.saveSummary(o.meetingId, o.summary, o.now);
        break;
      case 'deleteMeeting':
        repo.deleteMeeting(o.id);
        break;
      case 'saveSession':
        h['lifecycle.saveSession']({ state: o.state });
        break;
      case 'savePipeline':
        out.push(h['pipeline.save']({ state: o.state, settled: o.settled }).ok);
        break;
      case 'saveVectors':
        repo.saveVectors(
          o.model,
          (
            o.rows as {
              meetingId: string;
              source: string;
              segmentId: string;
              vecHex: string;
            }[]
          ).map((r) => ({
            rowid: rowid(db, r.meetingId, r.source, r.segmentId),
            meetingId: r.meetingId,
            vec: new Uint8Array(Buffer.from(r.vecHex, 'hex')),
          })),
        );
        break;
      default:
        throw new Error(`seed: unknown op ${o.op}`);
    }
  }
  return out;
}

function dump(db: DatabaseSync, opResults: unknown[]): unknown {
  const h = handlers(db);
  const repo = new NoteRepo(db);
  const q = seed.queries;
  const attempt = (f: () => unknown) => {
    try {
      return f();
    } catch (e) {
      return { error: (e as { code?: string }).code ?? String(e) };
    }
  };
  return plain({
    opResults,
    userVersion: (
      db.prepare('PRAGMA user_version').get() as { user_version: number }
    ).user_version,
    health: h['db.health'](),
    schema: db
      .prepare(
        'SELECT type, name, tbl_name AS tbl, sql FROM sqlite_master ORDER BY type, name',
      )
      .all(),
    list: q.list.map((p) => h['note.list'](p)),
    details: Object.fromEntries(
      q.details.map((id) => [id, attempt(() => h['note.get']({ id }))]),
    ),
    searchSegments: q.searchSegments.map((s) => repo.searchSegments(s)),
    missingVectors: q.missingVectors.map((p) =>
      repo.missingVectors(p.model, p.limit, p.meetingId),
    ),
    vectorSources: q.vectorSources.map((p) =>
      repo
        .vectorSources(p.model, p.meetingId)
        .map((v) => ({ ...v, vec: hex(v.vec) })),
    ),
    session: h['lifecycle.loadSession'](),
    pipelines: Object.fromEntries(
      q.pipelines.map((id) => [id, h['pipeline.get']({ id })]),
    ),
    unsettled: h['pipeline.listUnsettled'](),
    orphans: h['pipeline.orphans'](),
    notes: Object.fromEntries(q.notes.map((id) => [id, repo.getNotes(id)])),
    ftsQueries: q.ftsQueries.map((s) => toFtsQuery(s)),
  });
}

const tmp = mkdtempSync(join(tmpdir(), 'qrtz-parity-'));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));
let n = 0;
/** Opens a scratch copy so the committed fixture is never touched (open switches it to WAL). */
function openCopy(name: string): DatabaseSync {
  const p = join(tmp, `${n++}-${name}`);
  copyFileSync(join(DIR, name), p);
  return openDatabase(p);
}

describe('db parity (Electron <-> Rust)', () => {
  const fresh = openDatabase(':memory:');
  const results = apply(fresh, seed.ops);
  const got = dump(fresh, results);

  if (UPDATE) {
    writeFileSync(
      join(DIR, 'expected.json'),
      JSON.stringify(got, null, 1) + '\n',
    );
    const out = join(DIR, 'electron.sqlite');
    rmSync(out, { force: true });
    const file = openDatabase(out);
    apply(file, seed.ops);
    file.exec('PRAGMA journal_mode = DELETE');
    file.close();
  }
  const expected = JSON.parse(
    readFileSync(join(DIR, 'expected.json'), 'utf8'),
  ) as unknown;

  it('a fresh Electron DB seeded with the shared ops matches expected.json', () => {
    expect(got).toEqual(expected);
  });

  it('opens electron.sqlite (committed) and reads the same data', () => {
    expect(dump(openCopy('electron.sqlite'), results)).toEqual(expected);
  });

  it.skipIf(!existsSync(join(DIR, 'rust.sqlite')))(
    'opens the Rust-created rust.sqlite and reads the same data',
    () => {
      expect(dump(openCopy('rust.sqlite'), results)).toEqual(expected);
    },
  );

  it('keeps writing to a Rust-created DB (notes, segments, vectors, pipeline, delete)', () => {
    const db = openCopy(
      existsSync(join(DIR, 'rust.sqlite')) ? 'rust.sqlite' : 'electron.sqlite',
    );
    const h = handlers(db);
    const repo = new NoteRepo(db);
    repo.addNote('m-b', {
      id: 'n9',
      atMs: 9,
      text: 'added by Electron',
      createdAt: 9,
    });
    repo.appendSegment('m-c', {
      id: 'c0',
      startMs: 0,
      endMs: 10,
      text: 'Electron friday append',
      source: 'live',
    });
    const c0 = rowid(db, 'm-c', 'live', 'c0');
    repo.saveVectors('minilm-q8', [
      { rowid: c0, meetingId: 'm-c', vec: new Uint8Array([7]) },
    ]);
    expect(
      h['note.list']({ query: 'electron friday' }).map((r) => r.meeting.id),
    ).toEqual(['m-c']);
    expect(repo.getNotes('m-b').map((x) => x.id)).toEqual(['n4', 'n9']);
    expect(
      repo.vectorSources('minilm-q8', 'm-c').map((v) => hex(v.vec)),
    ).toEqual(['07']);
    repo.deleteMeeting('m-a');
    expect(h['note.list']({ query: 'rollback' })).toEqual([]);
    expect(repo.vectorSources('minilm-q8', 'm-a')).toEqual([]);
    expect(h['pipeline.get']({ id: 'm-a' })).toBeNull();
    expect(
      (
        db.prepare('PRAGMA integrity_check').get() as {
          integrity_check: string;
        }
      ).integrity_check,
    ).toBe('ok');
  });
});
