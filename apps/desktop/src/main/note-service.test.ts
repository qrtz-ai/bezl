import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  AppError,
  EventBus,
  Supervisor,
  createMockHelper,
  defaultConfig,
  parseHelperLine,
  type Config,
  type EventPayload,
  type FetchLike,
  type PipelineView,
  type SessionView,
} from '@qrtz/core';
import { DatabaseSync } from 'node:sqlite';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { NoteService } from './note-service';
import type { WorkerClient } from './worker-client';

// Integration: the real NoteService + Supervisor + TS mock helper (standup scenario, real timers, fast) +
// the real worker handlers on a file DB. Covers every failure path of docs/recording-lifecycle.md.

const nullLog = {
  debug() {},
  info() {},
  warn() {},
  error() {},
  child() {
    return nullLog;
  },
} as never;

type Handlers = ReturnType<typeof createWorkerHandlers>;
interface Rig {
  note: NoteService;
  bus: EventBus;
  helper: Supervisor;
  db: DatabaseSync;
  dbPath: string;
  dir: string;
  h: Handlers;
  sessions: SessionView[];
  pipelines: PipelineView[];
  transcript: EventPayload<'note:transcript'>[];
  mocks: ReturnType<typeof createMockHelper>[];
  writes: { helper: number; type: string }[];
  failWorker: Set<string>;
  llm: { fail: boolean; status: number; calls: number };
  close(): void;
}

const rigs: Rig[] = [];
afterEach(() => {
  for (const r of rigs.splice(0)) r.close();
});

function rig(
  o: {
    dir?: string;
    permissions?: Parameters<typeof createMockHelper>[0] extends infer P
      ? P extends { permissions?: infer Q }
        ? Q
        : never
      : never;
    llmFail?: boolean;
    llmStatus?: number;
    noStart?: boolean;
  } = {},
): Rig {
  const dir = o.dir ?? fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-lc-'));
  const dbPath = path.join(dir, 'qrtz.db');
  const db = openDatabase(dbPath);
  const llm = { fail: !!o.llmFail, status: o.llmStatus ?? 503, calls: 0 };
  const fetch: FetchLike = async () => {
    llm.calls++;
    if (llm.fail)
      return new Response('{"error":{"message":"upstream overloaded"}}', {
        status: llm.status,
      });
    return new Response(
      JSON.stringify({
        choices: [
          {
            message: {
              role: 'assistant',
              content: JSON.stringify({
                summary: 'STUB-AI summary',
                actionItems: [],
                qa: [],
              }),
            },
          },
        ],
      }),
      { status: 200 },
    );
  };
  const h = createWorkerHandlers(db, { fetch });
  const failWorker = new Set<string>();
  const worker = {
    async call(method: string, params: unknown) {
      if (failWorker.has(method))
        throw new AppError('E_UNAVAILABLE', `${method} unavailable`, {
          retryable: true,
        });
      const fn = (h as Record<string, (p: unknown) => unknown>)[method]!;
      // Structured-clone semantics like the real utilityProcess boundary.
      const r = await fn(
        params === undefined ? undefined : JSON.parse(JSON.stringify(params)),
      );
      return r === undefined ? r : JSON.parse(JSON.stringify(r));
    },
  } as unknown as WorkerClient;
  const mocks: ReturnType<typeof createMockHelper>[] = [];
  const writes: { helper: number; type: string }[] = [];
  const helper = new Supervisor({
    name: 'helper',
    baseDelayMs: 20,
    maxDelayMs: 40,
    heartbeatTimeoutMs: 0,
    spawn: () => {
      const m = createMockHelper({
        scenario: 'standup',
        speed: 0.01,
        detectAfterMs: 5,
        ...(o.permissions ? { permissions: o.permissions } : {}),
      });
      mocks.push(m);
      const write = m.write.bind(m);
      m.write = (line: string) => {
        writes.push({ helper: mocks.indexOf(m), type: JSON.parse(line).type });
        write(line);
      };
      return m;
    },
  });
  const bus = new EventBus();
  const base = defaultConfig();
  const config: Config = {
    ...base,
    note: { ...base.note, llmProvider: 'openrouter', autoRecord: false },
  } as Config;
  const note = new NoteService({
    helper,
    worker,
    bus,
    log: nullLog,
    secrets: {
      get: async () => 'sk-test',
      set: async () => {},
      delete: async () => {},
    } as never,
    config: () => config,
    recordingsDir: path.join(dir, 'recordings'),
    retryScale: 0.005,
  });
  let wasRunning = false;
  helper.onStatus((s) => {
    if (s.state === 'running') wasRunning = true;
    else if (wasRunning && s.state !== 'starting') {
      wasRunning = false;
      note.onHelperDown(s.lastError ?? s.state);
    }
    if (s.state === 'failed') note.onHelperFailed('failed');
  });
  helper.onLine((line) => {
    const r = parseHelperLine(line);
    if (r.ok) note.handle(r.msg);
  });
  const sessions: SessionView[] = [];
  const pipelines: PipelineView[] = [];
  const transcript: EventPayload<'note:transcript'>[] = [];
  bus.on('note:session', (v) => sessions.push(v));
  bus.on('note:pipeline', (v) => pipelines.push(v));
  bus.on('note:transcript', (v) => transcript.push(v));
  const r: Rig = {
    note,
    bus,
    helper,
    db,
    dbPath,
    dir,
    h,
    sessions,
    pipelines,
    transcript,
    mocks,
    writes,
    failWorker,
    llm,
    close: () => {
      helper.stop();
      try {
        db.close();
      } catch {
        /* closed */
      }
    },
  };
  rigs.push(r);
  void note.recover();
  if (!o.noStart) helper.start();
  return r;
}

async function until(fn: () => boolean | Promise<boolean>, ms = 5000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await fn()) return;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error('timed out waiting for condition');
}

const detected = (r: Rig) => until(() => r.note.view().phase === 'detected');
const finals = (r: Rig) => r.note.liveTranscript()?.segments.length ?? 0;

describe('NoteService lifecycle (integration with mock helper + real worker handlers)', () => {
  it('happy path: streams partials then finals, stops, post-processes every step, persists', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    expect(r.note.view().phase).toBe('recording');
    await until(() => finals(r) >= 3);
    // Partials streamed before finals, and finals replace their partial.
    const firstFinal = r.transcript.findIndex((d) => d.append.length);
    expect(r.transcript.slice(0, firstFinal).some((d) => d.partial)).toBe(true);
    const revs = r.transcript.map((d) => d.rev);
    expect(revs).toEqual([...revs].sort((a, b) => a - b));
    expect(
      r.transcript.every(
        (d, i) => i === 0 || d.baseRev === r.transcript[i - 1]!.rev,
      ),
    ).toBe(true);
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    const v = r.note.view();
    expect(v.pipeline?.steps.finalize.status).toBe('done');
    expect(v.pipeline?.steps.summary.status).toBe('done');
    await until(() =>
      ['done', 'skipped'].includes(r.note.view().pipeline!.steps.index.status),
    );
    const id = v.meetingId!;
    const m = await r.h['note.get']({ id });
    expect(m.meeting.status).toBe('done');
    expect(m.summary?.summary).toContain('STUB-AI');
    expect(m.segments.length).toBeGreaterThanOrEqual(3);
    expect(r.h['pipeline.get']({ id })?.steps.summary.status).toBe('done');
    expect(r.h['lifecycle.loadSession']()?.phase).toBe('done');
  });

  it('helper crash mid-recording: reconnects, resumes into a second WAV part, stitches it at finalize', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 2);
    const before = finals(r);
    r.mocks.at(-1)!.crash(1);
    // Every transition is broadcast (never coalesced away), so the renderer sees `reconnecting`.
    await until(() => r.sessions.some((s) => s.phase === 'reconnecting'));
    await until(
      () =>
        r.note.view().phase === 'recording' &&
        r.note.view().degraded.includes('transcript-gap'),
    );
    await until(() => finals(r) > before + 2);
    // Segments from the second part are namespaced and shifted into meeting time.
    const segs = r.note.liveTranscript()!.segments;
    const p1 = segs.filter((s) => s.id.startsWith('p1-'));
    expect(p1.length).toBeGreaterThan(0);
    const offset = r.note.machine.state.partOffsetMs;
    expect(offset).toBeGreaterThan(0);
    expect(Math.min(...p1.map((s) => s.startMs))).toBeGreaterThanOrEqual(
      offset,
    );
    // Fake audio for both parts (the TS mock does not write WAVs) to exercise stitching.
    const parts = r.note.machine.state.audioParts;
    expect(parts).toHaveLength(2);
    for (const f of parts) writeWav(f, 1600);
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    expect(fs.existsSync(parts[1]!)).toBe(false);
    expect(fs.statSync(parts[0]!).size).toBe(44 + 2 * 1600 * 2);
    expect(r.note.view().pipeline?.stopReason).toBe('manual');
  });

  it('a privacy pause survives a helper restart (the new helper is paused before resuming)', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    r.note.pause(true);
    r.mocks.at(-1)!.crash(1);
    await until(
      () => r.note.view().phase === 'recording' && r.mocks.length === 2,
    );
    const second = r.writes.filter((w) => w.helper === 1).map((w) => w.type);
    expect(second.indexOf('recording.pause')).toBeGreaterThan(
      second.indexOf('recording.start'),
    );
    expect(r.note.view().paused).toBe(true);
    await r.note.stop();
  });

  it('racing starts create exactly one meeting; detection during post-processing is accepted', async () => {
    const r = rig({ llmFail: true });
    await detected(r);
    const results = await Promise.allSettled([r.note.start(), r.note.start()]);
    expect(results.filter((x) => x.status === 'fulfilled')).toHaveLength(1);
    expect(r.h['note.list']({})).toHaveLength(1);
    await until(() => finals(r) >= 1);
    await r.note.stop();
    expect(r.note.view().phase).toBe('processing');
    r.note.handle({
      type: 'meeting.detected',
      key: 'next',
      title: 'Next',
      platform: 'zoom',
      app: 'Zoom',
    });
    expect(r.note.view().phase).toBe('detected');
    await r.note.start('Back to back');
    expect(r.note.view().phase).toBe('recording');
    await r.note.stop();
  });

  it('helper never comes back within the reconnect window: salvages with stop reason helper-lost', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 2);
    r.helper.stop(); // supervisor stops for good: nothing will restart it
    r.note.onHelperDown('killed');
    await until(() => r.note.view().phase === 'done', 8000);
    expect(r.note.view().stopReason).toBe('helper-lost');
    const m = await r.h['note.get']({ id: r.note.view().meetingId! });
    expect(m.meeting.status).toBe('done');
    expect(m.segments.length).toBeGreaterThanOrEqual(2);
  });

  it('permission denied (no system audio and no mic): start refused, error is state, no meeting row', async () => {
    const r = rig({
      permissions: { 'screen-recording': 'denied', microphone: 'denied' },
    });
    await detected(r);
    await until(() => r.note.getPermissions().microphone === 'denied');
    await expect(r.note.start()).rejects.toThrow(
      /Screen & System Audio Recording or Microphone/,
    );
    const v = r.note.view();
    expect(v.phase).toBe('detected');
    expect(v.errorInfo).toMatchObject({
      code: 'permission-denied',
      retryable: true,
    });
    expect(r.h['note.list']({})).toHaveLength(0);
    r.note.dismiss();
    expect(r.note.view().errorInfo).toBeUndefined();
  });

  it('screen recording denied but mic granted: records degraded mic-only; accessibility denied flags ax-unavailable', async () => {
    const r = rig({
      permissions: { 'screen-recording': 'denied', accessibility: 'denied' },
    });
    await detected(r);
    await until(() => r.note.getPermissions()['screen-recording'] === 'denied');
    await r.note.start();
    expect(r.note.view().degraded).toEqual(
      expect.arrayContaining(['mic-only', 'ax-unavailable']),
    );
    await r.note.stop();
  });

  it('LLM failure: retries with backoff, falls back to the on-device summary, manual retry then succeeds', async () => {
    const r = rig({ llmFail: true });
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 3);
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    const id = r.note.view().meetingId!;
    const s = r.note.view().pipeline!.steps.summary;
    expect(s).toMatchObject({ status: 'failed', attempts: 3, fallback: true });
    expect(s.error?.message).toContain('503');
    expect(r.llm.calls).toBe(3);
    expect(r.pipelines.some((p) => p.steps.summary.status === 'retrying')).toBe(
      true,
    );
    const m = await r.h['note.get']({ id });
    expect(m.summary?.llmError).toContain('503');
    expect(m.meeting.status).toBe('done');
    r.llm.fail = false;
    await r.note.retryStep(id, 'summary');
    await until(() => r.note.view().pipeline!.steps.summary.status === 'done');
    expect((await r.h['note.get']({ id })).summary?.summary).toContain(
      'STUB-AI',
    );
  });

  it('non-retryable LLM error (401) is not retried', async () => {
    const r = rig({ llmFail: true, llmStatus: 401 });
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    expect(r.note.view().pipeline!.steps.summary).toMatchObject({
      status: 'failed',
      attempts: 1,
    });
    expect(r.llm.calls).toBe(1);
  });

  it('finalize keeps failing: meeting marked failed, session error, manual retry recovers it', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    r.failWorker.add('note.stepFinalize');
    await r.note.stop();
    await until(() => r.note.view().phase === 'error');
    const id = r.note.view().meetingId!;
    expect(r.note.view().pipeline!.steps.finalize).toMatchObject({
      status: 'failed',
      attempts: 3,
    });
    expect(r.note.view().pipeline!.steps.summary.status).toBe('pending');
    expect((await r.h['note.get']({ id })).meeting.status).toBe('failed');
    r.failWorker.delete('note.stepFinalize');
    await r.note.retryStep(id, 'finalize');
    await until(() => r.note.view().pipeline!.steps.summary.status === 'done');
    expect((await r.h['note.get']({ id })).meeting.status).toBe('done');
  });

  it('live segment persistence retries transient worker failures', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    r.failWorker.add('note.appendLive');
    await until(() => finals(r) >= 2);
    setTimeout(() => r.failWorker.delete('note.appendLive'), 5);
    await until(
      async () =>
        (await r.h['note.get']({ id: r.note.view().meetingId! })).segments
          .length >= 2,
    );
    await r.note.stop();
  });

  it('app restart mid post-processing: pipeline resumes from the persisted step, no duplicate work', async () => {
    const a = rig({ llmFail: true });
    await detected(a);
    await a.note.start();
    await until(() => finals(a) >= 3);
    await a.note.stop();
    await until(
      () => a.note.view().pipeline?.steps.summary.status === 'retrying',
    );
    const id = a.note.view().meetingId!;
    // "Crash": tear everything down while summary is between attempts.
    const dir = a.dir;
    a.close();
    rigs.splice(rigs.indexOf(a), 1);
    const b = rig({ dir, noStart: true });
    await b.note.ready;
    await until(
      () => b.h['pipeline.get']({ id })?.steps.summary.status === 'done',
    );
    const p = b.h['pipeline.get']({ id })!;
    expect(p.steps.finalize).toMatchObject({ status: 'done', attempts: 1 });
    expect(p.steps.summary.attempts).toBeGreaterThanOrEqual(2);
    expect((await b.h['note.get']({ id })).summary?.summary).toContain(
      'STUB-AI',
    );
  });

  it('app killed mid-recording: next launch opens a crash-recovered pipeline from the persisted session', async () => {
    const a = rig();
    await detected(a);
    await a.note.start();
    await until(() => finals(a) >= 3);
    const id = a.note.view().meetingId!;
    await new Promise((res) => setTimeout(res, 50)); // let the session snapshot land
    const dir = a.dir;
    a.close();
    rigs.splice(rigs.indexOf(a), 1);
    const b = rig({ dir, noStart: true });
    await until(
      () => b.h['pipeline.get']({ id })?.steps.summary.status === 'done',
    );
    expect(b.h['pipeline.get']({ id })?.stopReason).toBe('crash-recovered');
    expect(b.note.view().phase).toBe('idle');
    const m = await b.h['note.get']({ id });
    expect(m.meeting.status).toBe('done');
    expect(m.segments.length).toBeGreaterThanOrEqual(3);
  });
});

function writeWav(file: string, samples: number) {
  const h = Buffer.alloc(44);
  h.write('RIFF', 0, 'ascii');
  h.writeUInt32LE(0, 4);
  h.write('WAVEfmt ', 8, 'ascii');
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20);
  h.writeUInt16LE(1, 22);
  h.writeUInt32LE(16000, 24);
  h.writeUInt32LE(32000, 28);
  h.writeUInt16LE(2, 32);
  h.writeUInt16LE(16, 34);
  h.write('data', 36, 'ascii');
  h.writeUInt32LE(0, 40);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, Buffer.concat([h, Buffer.alloc(samples * 2)]));
}
