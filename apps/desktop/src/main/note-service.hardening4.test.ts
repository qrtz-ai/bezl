/**
 * Hardening cycle 4: deleting a meeting (N14 served backoffs, N15 audio left on disk, N16 a deleted meeting's
 * pipeline kept running) and the owner-only recordings directory (N18). Talk history: talk-service.hardening4;
 * newer DB: worker/db.hardening4.
 * Real worker handlers on an in-memory or temp-file DB; every temp dir is removed after each test.
 */
import { afterEach, describe, expect, it } from 'vitest';
import {
  AppError,
  EventBus,
  Supervisor,
  createMockHelper,
  defaultConfig,
  newPipeline,
  nullLogger,
  type Config,
  type FetchLike,
} from '@qrtz/core';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { NoteService, insideDir, meetingAudio } from './note-service';
import type { WorkerClient } from './worker-client';

const dirs: string[] = [];
const tmp = (p = 'qrtz-h4-') => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), p));
  dirs.push(d);
  return d;
};
afterEach(() => {
  for (const d of dirs.splice(0))
    fs.rmSync(d, { recursive: true, force: true });
});

const okReply = () =>
  new Response(
    JSON.stringify({
      choices: [
        {
          message: {
            content: JSON.stringify({ summary: 'AI', actionItems: [], qa: [] }),
          },
        },
      ],
    }),
    { status: 200 },
  );
const busy = () =>
  new Response('{"error":{"message":"overloaded"}}', { status: 503 });

function rig(fetch: FetchLike, recordingsDir?: string) {
  const dir = tmp();
  const rec = recordingsDir ?? path.join(dir, 'rec');
  const db = openDatabase(':memory:');
  const h = createWorkerHandlers(db, { fetch });
  const rpc: string[] = [];
  const worker = {
    async call(method: string, params: unknown) {
      rpc.push(method);
      const fn = (h as Record<string, (p: unknown) => unknown>)[method];
      if (!fn) throw new AppError('E_NOT_FOUND', method);
      try {
        const r = await fn(
          params === undefined ? undefined : JSON.parse(JSON.stringify(params)),
        );
        return r === undefined ? r : JSON.parse(JSON.stringify(r));
      } catch (e) {
        throw AppError.fromJSON(AppError.from(e).toJSON());
      }
    },
  } as unknown as WorkerClient;
  const base = defaultConfig();
  const config = {
    ...base,
    note: { ...base.note, llmProvider: 'openrouter', autoRecord: false },
  } as Config;
  const note = new NoteService({
    helper: new Supervisor({
      name: 'helper',
      spawn: () => createMockHelper({ scenario: 'standup' }),
    }),
    worker,
    bus: new EventBus(),
    log: nullLogger,
    secrets: { get: async () => 'sk-test' } as never,
    config: () => config,
    recordingsDir: rec,
    retryScale: 0.02,
  });
  const meeting = (id: string, audioPath?: string, parts: string[] = []) => {
    h['note.create']({
      meeting: {
        id,
        title: 'Weekly',
        platform: 'zoom',
        startedAt: 1,
        status: 'processing',
        participants: [],
        ...(audioPath ? { audioPath } : {}),
      },
    });
    h['note.appendLive']({
      meetingId: id,
      segment: {
        id: 's0',
        startMs: 0,
        endMs: 900,
        text: 'We ship on Friday.',
        source: 'live',
      },
    });
    h['pipeline.save']({
      state: newPipeline(
        id,
        'manual',
        {
          startedAt: 1,
          endedAt: 2,
          durationMs: 1,
          audioParts: parts,
          active: [],
          helperCrashed: false,
        },
        Date.now(),
      ),
      settled: false,
    });
  };
  return { db, h, note, rpc, rec, meeting };
}

async function until(fn: () => boolean, ms = 4000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (fn()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error('timed out waiting for condition');
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe('hardening 4: deleting a meeting', () => {
  it('N16/N14: a meeting deleted while its summary waits to retry runs no further step and leaves no host state', async () => {
    let llm = 0;
    const r = rig(async () => (++llm, busy()));
    r.meeting('m');
    await r.note.recover();
    await until(
      () =>
        r.h['pipeline.get']({ id: 'm' })?.steps.summary.status === 'retrying',
    );
    await r.note.deleteMeeting('m');
    const before = r.rpc.length;
    await sleep(400); // several backoff windows at retryScale 0.02
    const after = r.rpc.slice(before);
    expect(after.filter((m) => m.startsWith('note.step'))).toEqual([]);
    expect(after).not.toContain('pipeline.save');
    expect(llm).toBe(1);
    expect(r.note.pipelineDebug()).toEqual({
      pipelines: 0,
      timers: 0,
      backoffServed: 0,
      manualSteps: 0,
    });
    expect(r.h['pipeline.get']({ id: 'm' })).toBeNull();
  });

  it('N16: a row deleted behind the host (another path) is noticed at the next save and forgotten, no E_NOT_FOUND attempts', async () => {
    let llm = 0;
    const r = rig(async () => (++llm, busy()));
    r.meeting('m');
    await r.note.recover();
    await until(
      () =>
        r.h['pipeline.get']({ id: 'm' })?.steps.summary.status === 'retrying',
    );
    r.h['note.delete']({ id: 'm' });
    await until(() => r.note.pipelineDebug().pipelines === 0, 3000);
    await sleep(200);
    expect(r.rpc.filter((m) => m === 'note.stepSummary')).toHaveLength(1);
    expect(r.note.pipelineDebug().backoffServed).toBe(0);
    expect(llm).toBe(1);
  });

  it('N16: a manual retry of a deleted meeting is refused and starts nothing', async () => {
    const r = rig(async () => okReply());
    r.meeting('m');
    await r.note.recover();
    await until(() =>
      ['done', 'skipped'].includes(
        r.h['pipeline.get']({ id: 'm' })?.steps.index.status ?? '',
      ),
    );
    await r.note.deleteMeeting('m');
    await expect(r.note.retryStep('m', 'summary')).rejects.toMatchObject({
      code: 'E_NOT_FOUND',
    });
    expect(r.note.pipelineDebug().pipelines).toBe(0);
  });

  it('N15: delete removes the recording and every leftover part, never another meeting\u2019s file', async () => {
    const r = rig(async () => okReply());
    fs.mkdirSync(r.rec, { recursive: true });
    const own = path.join(r.rec, 'm1.wav');
    const part = path.join(r.rec, 'm1.p2.wav');
    const listedPart = path.join(r.rec, 'm1.p1.wav');
    const other = path.join(r.rec, 'm10.wav');
    const otherPart = path.join(r.rec, 'm1x.p1.wav');
    for (const f of [own, part, listedPart, other, otherPart])
      fs.writeFileSync(f, 'RIFF');
    r.meeting('m1', own, [own, listedPart]);
    const { removed } = await r.note.deleteMeeting('m1');
    expect(removed.map((f) => path.basename(f)).sort()).toEqual([
      'm1.p1.wav',
      'm1.p2.wav',
      'm1.wav',
    ]);
    expect(fs.readdirSync(r.rec).sort()).toEqual(['m10.wav', 'm1x.p1.wav']);
    expect(r.h['note.get']).toBeDefined();
    expect(() => r.h['note.get']({ id: 'm1' })).toThrow();
  });

  it('N15: a DB path outside the recordings directory (tampered or old backup) is never deleted', async () => {
    const outside = tmp('qrtz-h4-outside-');
    const victim = path.join(outside, 'keep.wav');
    fs.writeFileSync(victim, 'precious');
    const r = rig(async () => okReply());
    fs.mkdirSync(r.rec, { recursive: true });
    const sneaky = path.join(r.rec, '..', path.basename(outside), 'keep.wav');
    r.meeting('m2', victim, [sneaky, '/etc/hosts']);
    const { removed } = await r.note.deleteMeeting('m2');
    expect(removed).toEqual([]);
    expect(fs.readFileSync(victim, 'utf8')).toBe('precious');
  });

  it('N15: delete of a meeting that never had audio, or twice, is fine', async () => {
    const r = rig(async () => okReply());
    r.meeting('m3');
    await expect(r.note.deleteMeeting('m3')).resolves.toEqual({ removed: [] });
    await expect(r.note.deleteMeeting('m3')).resolves.toEqual({ removed: [] });
  });

  it('insideDir / meetingAudio: traversal, siblings with a shared prefix, the dir itself', () => {
    const d = tmp();
    expect(insideDir(d, path.join(d, 'a.wav'))).toBe(true);
    expect(insideDir(d, path.join(d, 'sub', 'a.wav'))).toBe(true);
    expect(insideDir(d, d)).toBe(false);
    expect(insideDir(d, path.join(d, '..', 'a.wav'))).toBe(false);
    expect(insideDir(d, `${d}2/a.wav`)).toBe(false);
    expect(insideDir(d, path.join(d, '..notes.wav'))).toBe(true); // a name, not a parent
    fs.writeFileSync(path.join(d, 'x.wav'), '');
    fs.writeFileSync(path.join(d, 'x.p3.wav'), '');
    fs.writeFileSync(path.join(d, 'x.p.wav'), ''); // not a part name
    fs.writeFileSync(path.join(d, 'xy.wav'), '');
    expect(
      meetingAudio(d, 'x', [])
        .map((f) => path.basename(f))
        .sort(),
    ).toEqual(['x.p3.wav', 'x.wav']);
    expect(meetingAudio(path.join(d, 'missing'), 'x', [])).toEqual([]);
  });

  it('repo.meetingFiles: recording + listed parts, deduplicated; a corrupt pipeline record lists only the recording', () => {
    const r = rig(async () => okReply());
    r.meeting('m4', '/r/m4.wav', ['/r/m4.wav', '/r/m4.p1.wav', '']);
    expect(r.h['note.delete']({ id: 'm4' }).files).toEqual([
      '/r/m4.wav',
      '/r/m4.p1.wav',
    ]);
    r.meeting('m5', '/r/m5.wav');
    r.db
      .prepare(`UPDATE pipeline_state SET json='{nope' WHERE meeting_id='m5'`)
      .run();
    expect(r.h['note.delete']({ id: 'm5' }).files).toEqual(['/r/m5.wav']);
  });
});

describe('hardening 4: owner-only files', () => {
  it('N18: the recordings directory is 0700, also when an older build created it 0755', () => {
    if (process.platform === 'win32') return;
    const d = tmp();
    const rec = path.join(d, 'rec');
    fs.mkdirSync(rec, { mode: 0o755 });
    fs.chmodSync(rec, 0o755);
    rig(async () => okReply(), rec);
    expect(fs.statSync(rec).mode & 0o777).toBe(0o700);
    const fresh = path.join(d, 'fresh');
    rig(async () => okReply(), fresh);
    expect(fs.statSync(fresh).mode & 0o777).toBe(0o700);
  });
});
