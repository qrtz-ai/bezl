/**
 * Backup zip parity, Electron <-> Rust (Tauri migration, ADR 0042). Fixtures in spec/fixtures/backup:
 *
 * - electron-v1.zip: written here by `writeBackup` from the library in `seedLibrary` (QRTZ_FIXTURES_UPDATE=1).
 * - expected.json: that library's dump (audio paths reduced to file name + sha256).
 * - rust-v1.zip: electron-v1.zip imported by crates/qrtz-backup and exported again by it
 *   (`QRTZ_FIXTURES_UPDATE=1 cargo test -p qrtz-backup --test parity`).
 * - malicious/*.zip + malicious/cases.json: written by the Rust test; every one must be rejected by both sides,
 *   leaving nothing behind.
 *
 * Importing either zip here must reproduce expected.json: Electron -> Rust -> Electron.
 */
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { NoteRepo, type FetchLike } from '@qrtz/core';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { readBackup, writeBackup } from './backup';

const DIR = fileURLToPath(
  new URL('../../../../spec/fixtures/backup/', import.meta.url),
);
const UPDATE = process.env.QRTZ_FIXTURES_UPDATE === '1';
const noFetch: FetchLike = async () => {
  throw new Error('network disabled in tests');
};
const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-bparity-'));
const sha = (b: Buffer) => createHash('sha256').update(b).digest('hex');

/** Deterministic WAV-ish bytes. */
const wav = (n: number, seed: number) =>
  Buffer.from(Array.from({ length: n }, (_, i) => (i * 31 + seed) & 0xff));

function seedLibrary(dir: string) {
  const db = openDatabase(path.join(dir, 'qrtz.db'));
  const repo = new NoteRepo(db);
  const rec = path.join(dir, 'recordings');
  fs.mkdirSync(rec);
  fs.writeFileSync(path.join(rec, 'm-a.wav'), wav(70_000, 1));
  fs.writeFileSync(path.join(rec, 'orphan.wav'), wav(1_000, 2));
  repo.upsertMeeting({
    id: 'm-a',
    title: 'Weekly sync — Δ planning',
    platform: 'zoom',
    startedAt: 1_760_000_000_000,
    endedAt: 1_760_003_600_000,
    status: 'done',
    participants: ['Ada', 'Bob'],
    audioPath: path.join(rec, 'm-a.wav'),
    durationMs: 3_600_000,
  });
  repo.setTemplate('m-a', 'one-on-one');
  repo.replaceSegments('m-a', 'live', [
    {
      id: 'a1',
      startMs: 0,
      endMs: 4000,
      text: "Let's review the budget.",
      source: 'live',
      speaker: 'S1',
    },
    {
      id: 'a2',
      startMs: 5000,
      endMs: 9000,
      text: 'Fifty thousand, 会议 ok.',
      source: 'live',
      speakerName: 'Bob',
    },
  ]);
  repo.replaceSegments('m-a', 'post', [
    {
      id: 'p1',
      startMs: 0,
      endMs: 9000,
      text: "Let's review the budget. Fifty thousand.",
      source: 'post',
      speakerName: 'Ada',
      confidence: 0.92,
    },
  ]);
  repo.saveSummary(
    'm-a',
    {
      summary: 'Budget agreed.',
      actionItems: [{ text: 'Send numbers', owner: 'Bob' }],
      qa: [{ question: 'How much?', answer: '50k', askedBy: 'Ada' }],
      provider: 'extractive',
      template: 'one-on-one',
    },
    1_760_003_700_000,
  );
  repo.addNote('m-a', {
    id: 'n1',
    atMs: 6200,
    text: 'Pilot 50k',
    createdAt: 1_760_000_006_200,
  });
  repo.upsertMeeting({
    id: 'm-b',
    title: 'No audio here',
    platform: 'teams',
    startedAt: 1_760_100_000_000,
    status: 'done',
    participants: [],
  });
  repo.replaceSegments('m-b', 'live', [
    { id: 'b1', startMs: 0, endMs: 1000, text: 'hi', source: 'live' },
  ]);
  repo.upsertMeeting({
    id: 'm-c',
    title: 'Audio missing from the archive',
    platform: 'other',
    startedAt: 1_760_200_000_000,
    status: 'failed',
    participants: ['Cy'],
    audioPath: path.join(rec, 'gone.wav'),
  });
  return { db, rec };
}

type Db = ReturnType<typeof openDatabase>;
function dump(db: Db) {
  const repo = new NoteRepo(db);
  return repo
    .listMeetings(1_000_000)
    .sort((a, b) => a.id.localeCompare(b.id))
    .map((m) => {
      const { audioPath, ...rest } = m;
      const summary = repo.getSummary(m.id);
      return {
        meeting: rest,
        audio: audioPath
          ? {
              name: path.basename(audioPath),
              sha256: fs.existsSync(audioPath)
                ? sha(fs.readFileSync(audioPath))
                : null,
            }
          : null,
        segments: repo.getSegments(m.id),
        summary: summary ?? null,
        notes: repo.getNotes(m.id),
      };
    });
}

async function importInto(zip: string) {
  const dir = tmp();
  const db = openDatabase(path.join(dir, 'qrtz.db'));
  const h = createWorkerHandlers(db, { fetch: noFetch });
  const audioDir = path.join(dir, 'recordings');
  const r = await readBackup({
    source: zip,
    workDir: path.join(dir, 'w'),
    audioDir,
  });
  const rep = h['data.importDb']({ path: r.dbPath, audioMap: r.audioMap });
  return { dir, db, audioDir, rep };
}

describe('backup zip parity with the Rust build', () => {
  it('electron-v1.zip is the seeded library; importing it reproduces expected.json', async () => {
    const electron = path.join(DIR, 'electron-v1.zip');
    if (UPDATE || !fs.existsSync(electron)) {
      const dir = tmp();
      const { db, rec } = seedLibrary(dir);
      const h = createWorkerHandlers(db, { fetch: noFetch });
      const snap = path.join(dir, 'snap.db');
      const { meetings } = h['data.snapshot']({ path: snap });
      await writeBackup({
        target: electron,
        dbSnapshot: snap,
        audioFiles: fs
          .readdirSync(rec)
          .sort()
          .map((f) => path.join(rec, f)),
        appVersion: '0.1.0-fixture',
        schemaVersion: (h['db.health']() as { schemaVersion: number })
          .schemaVersion,
        meetings,
        now: 1_760_500_000_000,
      });
      const d = dump(db);
      // The source library's m-c points at a file that never existed; after import it has no audio at all.
      fs.writeFileSync(
        path.join(DIR, 'expected.json'),
        `${JSON.stringify(
          d.map((m) => (m.audio?.sha256 === null ? { ...m, audio: null } : m)),
          null,
          1,
        )}\n`,
      );
      fs.rmSync(dir, { recursive: true, force: true });
    }
    const expected = JSON.parse(
      fs.readFileSync(path.join(DIR, 'expected.json'), 'utf8'),
    );
    const { dir, db, rep } = await importInto(electron);
    expect(rep).toEqual({ imported: 3, skipped: 0 });
    expect(JSON.parse(JSON.stringify(dump(db)))).toEqual(expected);
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('rust-v1.zip (Electron -> Rust export) imports here to the same library', async () => {
    const rust = path.join(DIR, 'rust-v1.zip');
    expect(fs.existsSync(rust)).toBe(true);
    const expected = JSON.parse(
      fs.readFileSync(path.join(DIR, 'expected.json'), 'utf8'),
    );
    const { dir, db, rep } = await importInto(rust);
    expect(rep).toEqual({ imported: 3, skipped: 0 });
    expect(JSON.parse(JSON.stringify(dump(db)))).toEqual(expected);
    const again = h2(db);
    const r = await readBackup({
      source: rust,
      workDir: path.join(dir, 'w2'),
      audioDir: path.join(dir, 'recordings'),
    });
    expect(
      again['data.importDb']({ path: r.dbPath, audioMap: r.audioMap }),
    ).toEqual({ imported: 0, skipped: 3 });
    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('every malicious or corrupt zip is rejected and leaves nothing behind', async () => {
    const cases = JSON.parse(
      fs.readFileSync(path.join(DIR, 'malicious', 'cases.json'), 'utf8'),
    ) as { file: string; error: string; stage: 'read' | 'import' }[];
    expect(cases.length).toBeGreaterThanOrEqual(12);
    for (const c of cases) {
      const dir = tmp();
      const db = openDatabase(path.join(dir, 'qrtz.db'));
      const audioDir = path.join(dir, 'recordings');
      const run = async () => {
        const r = await readBackup({
          source: path.join(DIR, 'malicious', c.file),
          workDir: path.join(dir, 'w'),
          audioDir,
        });
        h2(db)['data.importDb']({ path: r.dbPath, audioMap: r.audioMap });
      };
      await expect(run(), c.file).rejects.toThrow(c.error);
      expect(new NoteRepo(db).listMeetings(10), c.file).toEqual([]);
      if (c.stage === 'read')
        expect(
          fs.existsSync(audioDir) ? fs.readdirSync(audioDir) : [],
          c.file,
        ).toEqual([]);
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});

const h2 = (db: Db) => createWorkerHandlers(db, { fetch: noFetch });
