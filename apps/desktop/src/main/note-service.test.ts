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
  encodeHostMessage,
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
import { gapPadBytes } from '../worker/wav-repair';
import { DELETE_WHILE_RECORDING, NoteService } from './note-service';
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
  writes: { helper: number; type: string; msg: Record<string, unknown> }[];
  failWorker: Set<string>;
  /** N29: meeting ids `onNoAudio` fired for, in order. */
  noAudio: string[];
  llm: {
    fail: boolean;
    status: number;
    calls: number;
    inflight: number;
    maxInflight: number;
  };
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
    revoke?: Parameters<typeof createMockHelper>[0] extends infer P
      ? P extends { revoke?: infer Q }
        ? Q
        : never
      : never;
    noStart?: boolean;
    llmDelayMs?: number;
    /** Every mock helper fails to create recording files (fails closed: error recording.file, no started). */
    failRecordingFile?: boolean;
    /** N2: recording.stop is never acked; recording.abort is answered ('ok') or not ('wedged'). */
    wedgeStop?: { abort?: 'ok' | 'wedged' };
    /** Runs before each host→helper line reaches the mock (fault/timing injection). */
    beforeWrite?: (
      type: string,
      m: ReturnType<typeof createMockHelper>,
      note: NoteService,
      msg: Record<string, unknown>,
    ) => void;
    /** Drops a helper→host line before the host sees it (simulates a host that never heard of it). */
    dropLine?: (line: string) => boolean;
    /** N29: the mock's capture.silent / capture.audible script. */
    silence?: Parameters<typeof createMockHelper>[0] extends infer P
      ? P extends { silence?: infer Q }
        ? Q
        : never
      : never;
    /** `note.*` config overrides. */
    noteConfig?: Partial<Config['note']>;
  } = {},
): Rig {
  const dir = o.dir ?? fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-lc-'));
  const dbPath = path.join(dir, 'qrtz.db');
  const db = openDatabase(dbPath);
  const llm = {
    fail: !!o.llmFail,
    status: o.llmStatus ?? 503,
    calls: 0,
    inflight: 0,
    maxInflight: 0,
  };
  const fetch: FetchLike = async () => {
    llm.calls++;
    llm.inflight++;
    llm.maxInflight = Math.max(llm.maxInflight, llm.inflight);
    if (o.llmDelayMs) await new Promise((res) => setTimeout(res, o.llmDelayMs));
    llm.inflight--;
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
  const writes: Rig['writes'] = [];
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
        ...(o.revoke ? { revoke: o.revoke } : {}),
        ...(o.failRecordingFile ? { failRecordingFile: true } : {}),
        ...(o.wedgeStop ? { wedgeStop: o.wedgeStop } : {}),
        ...(o.silence ? { silence: o.silence } : {}),
      });
      mocks.push(m);
      const write = m.write.bind(m);
      m.write = (line: string) => {
        const msg = JSON.parse(line) as Record<string, unknown>;
        const type = msg.type as string;
        writes.push({ helper: mocks.indexOf(m), type, msg });
        o.beforeWrite?.(type, m, note, msg);
        write(line);
      };
      return m;
    },
  });
  const bus = new EventBus();
  const base = defaultConfig();
  const config: Config = {
    ...base,
    note: {
      ...base.note,
      llmProvider: 'openrouter',
      autoRecord: false,
      ...o.noteConfig,
    },
  } as Config;
  const noAudio: string[] = [];
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
    onNoAudio: (id) => noAudio.push(id),
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
    if (o.dropLine?.(line)) return;
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
    noAudio,
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
    // H7: the reconnect gap is silence, so the second part starts at its offset in the stitched file.
    const pad = gapPadBytes(offset, 3200, { byteRate: 32000, blockAlign: 2 });
    expect(pad).toBe(
      Math.max(0, Math.floor((offset * 32000) / 2000) * 2 - 3200),
    );
    expect(fs.statSync(parts[0]!).size).toBe(44 + 3200 + pad + 3200);
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

  it('H5: TCC revoked mid-recording degrades live (screen → mic-only, then mic → audio-unavailable) and keeps recording', async () => {
    const r = rig({
      revoke: [
        { permission: 'screen-recording', afterMs: 20 },
        { permission: 'microphone', afterMs: 60 },
      ],
    });
    await detected(r);
    await r.note.start();
    expect(r.note.view().degraded).toEqual([]);
    await until(() => r.note.view().degraded.includes('mic-only'));
    await until(() => r.note.view().degraded.includes('audio-unavailable'));
    expect(r.note.view().phase).toBe('recording');
    expect(r.note.getPermissions()).toMatchObject({
      'screen-recording': 'denied',
      microphone: 'denied',
    });
    await r.note.stop();
    await until(
      () => r.note.view().phase === 'idle' || r.note.view().phase === 'done',
    );
  });

  it('H5: mic revoked mid-recording with system audio still granted flags no-mic', async () => {
    const r = rig({ revoke: [{ permission: 'microphone', afterMs: 20 }] });
    await detected(r);
    await r.note.start();
    await until(() => r.note.view().degraded.includes('no-mic'));
    expect(r.note.view().degraded).not.toContain('audio-unavailable');
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

  // ---- cross-model review fixes (docs/architecture-review.md, C-findings) ----

  it('C1: finals the helper flushes after recording.stop (phase stopping) are kept and persisted', async () => {
    const r = rig({
      beforeWrite: (type, _m, note) => {
        if (type === 'recording.stop')
          // The recogniser's tail arrives while main is already in `stopping`.
          queueMicrotask(() =>
            note.handle({
              type: 'asr',
              event: {
                kind: 'final',
                utteranceId: 'tail',
                startMs: 90_000,
                endMs: 91_000,
                text: 'Last words before stop.',
              },
            }),
          );
      },
    });
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    const m = await r.h['note.get']({ id: r.note.view().meetingId! });
    expect(m.segments.map((x) => x.text)).toContain('Last words before stop.');
  });

  it('C2: a manual finalize retry that succeeds takes the session from error to done', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    r.failWorker.add('note.stepFinalize');
    await r.note.stop();
    await until(() => r.note.view().phase === 'error');
    r.failWorker.delete('note.stepFinalize');
    await r.note.retryStep(r.note.view().meetingId!, 'finalize');
    await until(() => r.note.view().phase === 'done');
    expect(r.note.view().errorInfo).toBeUndefined();
  });

  it('C3: stop is idempotent (double stop resolves, one pipeline)', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    const [a, b] = await Promise.all([r.note.stop(), r.note.stop()]);
    expect(a.meetingId).toBe(b.meetingId);
    await expect(r.note.stop()).resolves.toMatchObject({
      meetingId: a.meetingId,
    });
    expect(r.writes.filter((w) => w.type === 'recording.stop')).toHaveLength(1);
  });

  it('C4: helper acks are correlated by audio path; a stale recording.started is stopped, not trusted', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    const stopsBefore = r.writes.filter(
      (w) => w.type === 'recording.stop',
    ).length;
    r.note.handle({
      type: 'recording.started',
      audioPath: '/elsewhere/stale.wav',
    });
    expect(r.writes.filter((w) => w.type === 'recording.stop').length).toBe(
      stopsBefore + 1,
    );
    expect(r.note.view().phase).toBe('recording'); // our own capture is untouched
    // The real ack for our own path is not treated as stale.
    r.note.handle({
      type: 'recording.started',
      audioPath: r.note.machine.state.audioParts[0]!,
    });
    expect(r.writes.filter((w) => w.type === 'recording.stop').length).toBe(
      stopsBefore + 1,
    );
  });

  it('H6: a start the helper answers with recording.busy fails fast and only stops its own file', async () => {
    let sent = false;
    const r = rig({
      beforeWrite: (type, _m, note, msg) => {
        if (type !== 'recording.start' || sent) return;
        sent = true;
        note.handle({
          type: 'error',
          code: 'recording.busy',
          message: 'a recording is already running',
          fatal: false,
          audioPath: msg.audioPath as string,
          activeAudioPath: '/elsewhere/orphan.wav',
        });
      },
    });
    await detected(r);
    const t0 = Date.now();
    await expect(r.note.start()).rejects.toThrow(
      /still busy with another recording/,
    );
    expect(Date.now() - t0).toBeLessThan(1000);
    expect(r.note.view().phase).not.toBe('recording');
    const own = r.writes.find((w) => w.type === 'recording.start')!.msg
      .audioPath;
    const stops = r.writes
      .filter((w) => w.type === 'recording.stop')
      .map((w) => w.msg.audioPath);
    // The orphan named by activeAudioPath first, then this attempt's own file; never a bare stop.
    expect(stops).toEqual(['/elsewhere/orphan.wav', own]);
  });

  it('busy re-review: an orphaned recording A is stopped by its path, so the retry starts', async () => {
    const orphan = path.join(os.tmpdir(), `qrtz-orphan-${Date.now()}.wav`);
    // The host never sees recording A's ack (it was started by a previous host process).
    const r = rig({
      dropLine: (l) => l.includes('recording.started') && l.includes(orphan),
    });
    await detected(r);
    r.mocks[0]!.write(
      encodeHostMessage({ type: 'recording.start', audioPath: orphan }),
    );
    await expect(r.note.start()).rejects.toThrow(
      /still busy with another recording/,
    );
    expect(
      r.writes
        .filter((w) => w.type === 'recording.stop')
        .map((w) => w.msg.audioPath),
    ).toContain(orphan);
    // Before the fix the orphan ran forever and every retry got helper-busy.
    await r.note.start();
    expect(r.note.view().phase).toBe('recording');
    await r.note.stop();
    fs.rmSync(orphan, { force: true });
  });

  it('busy re-review: a busy for another file (or with no file) never fails the start in flight', async () => {
    let sent = false;
    const r = rig({
      beforeWrite: (type, _m, note) => {
        if (type !== 'recording.start' || sent) return;
        sent = true;
        const busy = (o: { audioPath?: string }) =>
          note.handle({
            type: 'error',
            code: 'recording.busy',
            message: 'a recording is already running',
            fatal: false,
            activeAudioPath: '/elsewhere/a.wav',
            ...o,
          });
        busy({ audioPath: '/elsewhere/older-attempt.wav' });
        busy({});
      },
    });
    await detected(r);
    await r.note.start();
    expect(r.note.view().phase).toBe('recording');
    expect(r.writes.filter((w) => w.type === 'recording.stop')).toEqual([]);
    await r.note.stop();
  });

  it('file rule: a start whose file the helper cannot create fails fast (recording-file), no helper restart', async () => {
    const r = rig({ failRecordingFile: true });
    await detected(r);
    const t0 = Date.now();
    await expect(r.note.start()).rejects.toThrow(
      /recording file could not be created/,
    );
    expect(Date.now() - t0).toBeLessThan(1000);
    const v = r.note.view();
    expect(v.phase).toBe('detected'); // back where it was before the start
    expect(v.errorInfo?.code).toBe('recording-file');
    const own = r.writes.find((w) => w.type === 'recording.start')!.msg
      .audioPath;
    // Only this attempt's (already torn down) file; never a pathless stop, never a restart.
    expect(
      r.writes
        .filter((w) => w.type === 'recording.stop')
        .map((w) => w.msg.audioPath),
    ).toEqual([own]);
    expect(r.mocks).toHaveLength(1);
    // The helper is idle again: the next start is admitted (not answered with a busy).
    await expect(r.note.start()).rejects.toThrow(
      /recording file could not be created/,
    );
    expect(r.mocks).toHaveLength(1);
  });

  it('file rule watchdog: a busy naming no active file is a broken invariant, so the helper is restarted', async () => {
    let sent = false;
    const r = rig({
      beforeWrite: (type, _m, note, msg) => {
        if (type !== 'recording.start' || sent) return;
        sent = true;
        note.handle({
          type: 'error',
          code: 'recording.busy',
          message: 'a recording is already running',
          fatal: false,
          audioPath: msg.audioPath as string,
        });
      },
    });
    await detected(r);
    await expect(r.note.start()).rejects.toThrow(
      /still busy with another recording/,
    );
    // No stop at all: the unnamed recording cannot be stopped by path, and a pathless stop is never sent.
    expect(r.writes.filter((w) => w.type === 'recording.stop')).toEqual([]);
    await until(() => r.mocks.length === 2);
    // The restart happens after the failed start, so it never looks like a crash mid-recording.
    expect(r.note.view().phase).toBe('detected');
    await until(() => r.helper.status.state === 'running');
    await r.note.start();
    expect(r.note.view().phase).toBe('recording');
    await r.note.stop();
  });

  it(
    'N2: a missed stop ack aborts just that recording by its file; the helper (and a dictation on it) survives',
    { timeout: 20_000 },
    async () => {
      const r = rig({ wedgeStop: { abort: 'ok' } });
      const lines: string[] = [];
      r.helper.onLine((l) => lines.push(l));
      await detected(r);
      await r.note.start();
      await until(() => finals(r) >= 1);
      // A dictation shares the helper with the recording.
      r.mocks[0]!.write(
        encodeHostMessage({ type: 'dictation.start', id: 'd1' }),
      );
      await until(() => lines.some((l) => l.includes('"dictation.started"')));
      await r.note.stop();
      const own = r.writes.find((w) => w.type === 'recording.start')!.msg
        .audioPath;
      expect(
        r.writes
          .filter((w) => w.type === 'recording.abort')
          .map((w) => w.msg.audioPath),
      ).toEqual([own]);
      expect(r.mocks).toHaveLength(1); // no restart
      await until(() => r.note.view().phase === 'done');
      // The dictation is still alive on the same helper: its stop is answered.
      r.mocks[0]!.write(
        encodeHostMessage({ type: 'dictation.stop', id: 'd1' }),
      );
      await until(() =>
        lines.some(
          (l) => l.includes('"dictation.stopped"') && l.includes('"d1"'),
        ),
      );
      expect(lines.some((l) => l.includes('helper-lost'))).toBe(false);
    },
  );

  it(
    'N2: when the abort is not acked either (hard deadline), the helper is restarted',
    { timeout: 40_000 },
    async () => {
      const r = rig({ wedgeStop: { abort: 'wedged' } });
      await detected(r);
      await r.note.start();
      await until(() => finals(r) >= 1);
      await r.note.stop();
      expect(r.writes.filter((w) => w.type === 'recording.abort')).toHaveLength(
        1,
      );
      // The abort deadline plus a helper restart: generous waits, this runs under full-suite load.
      await until(() => r.mocks.length === 2, 15_000);
      // The meeting still completes. Use the session events, not the live view: the restarted helper re-detects the
      // meeting right after, which moves the view on to `detected`.
      await until(() => r.sessions.some((v) => v.phase === 'done'), 15_000);
    },
  );

  it('the meeting being recorded cannot be deleted (also not deferred); once processing it can', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    const id = r.note.view().meetingId!;
    const refused = await r.note.deleteMeeting(id).catch((e) => e);
    expect(refused).toBeInstanceOf(AppError);
    expect(refused).toMatchObject({
      code: 'E_VALIDATION',
      message: DELETE_WHILE_RECORDING,
    });
    expect(() => r.note.assertDeletable(id)).toThrow(DELETE_WHILE_RECORDING); // the deferMs (undo) path
    expect(() => r.note.assertDeletable('another-meeting')).not.toThrow();
    // Nothing was removed and the recording goes on.
    expect(r.note.view().phase).toBe('recording');
    expect(await r.h['note.get']({ id })).toBeTruthy();
    expect(r.writes.filter((w) => w.type === 'recording.stop')).toEqual([]);
    await r.note.stop();
    expect(() => r.note.assertDeletable(id)).not.toThrow();
    await until(() => r.note.view().phase === 'done');
    await r.note.deleteMeeting(id);
    expect(r.h['note.list']({})).toHaveLength(0);
  });

  it('C5: a step never runs if its start could not be persisted (durability barrier), then retries', async () => {
    const r = rig();
    await detected(r);
    await r.note.start();
    await until(() => finals(r) >= 1);
    let saves = 0;
    const orig = r.h['pipeline.save'];
    (r.h as Record<string, unknown>)['pipeline.save'] = (
      p: Parameters<typeof orig>[0],
    ) => {
      // Fail exactly the save that marks summary running.
      if (p.state.steps.summary.status === 'running' && saves++ === 0)
        throw new Error('disk full');
      return orig(p);
    };
    await r.note.stop();
    await until(() => r.note.view().phase === 'done');
    const s = r.note.view().pipeline!.steps.summary;
    expect(s.status).toBe('done');
    expect(s.attempts).toBe(2);
    expect(r.llm.calls).toBe(1); // the unpersisted first attempt never reached the LLM
  });

  it('C6: summaries across meetings never call the LLM concurrently (no retry storms)', async () => {
    const a = rig({ llmDelayMs: 60 });
    await detected(a);
    await a.note.start();
    await until(() => finals(a) >= 1);
    await a.note.stop();
    await a.note.start('second');
    await until(() => finals(a) >= 1);
    await a.note.stop();
    await until(() => a.llm.calls >= 2 && a.llm.inflight === 0, 8000);
    expect(a.llm.maxInflight).toBe(1);
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

describe('NoteService no-audio hook (N29)', () => {
  it('fires once when every source goes silent, not for one silent source', async () => {
    const r = rig({
      silence: [
        { source: 'mic', afterMs: 40, audibleAfterMs: 500, sinceMs: 0 },
        { source: 'system', afterMs: 120, audibleAfterMs: 400, sinceMs: 0 },
      ],
    });
    await detected(r);
    await r.note.start();
    const id = r.note.view().meetingId!;
    await until(() => r.sessions.some((v) => v.noAudio));
    const firstSilent = r.sessions.findIndex((v) => v.silent?.length);
    const firstNoAudio = r.sessions.findIndex((v) => v.noAudio);
    expect(firstSilent).toBeLessThan(firstNoAudio);
    expect(r.noAudio).toEqual([id]);
    await until(
      () =>
        r.note.view().phase === 'recording' && !r.note.view().silent?.length,
    );
    expect(r.noAudio).toEqual([id]);
    await r.note.stop();
  });

  it('note.notifyNoAudio off: noAudio still shows in the session, the hook never fires', async () => {
    const r = rig({
      noteConfig: { notifyNoAudio: false },
      silence: [
        { source: 'mic', afterMs: 20, sinceMs: 0 },
        { source: 'system', afterMs: 20, sinceMs: 0 },
      ],
    });
    await detected(r);
    await r.note.start();
    await until(() => !!r.note.view().noAudio);
    expect(r.noAudio).toEqual([]);
    await r.note.stop();
  });
});
