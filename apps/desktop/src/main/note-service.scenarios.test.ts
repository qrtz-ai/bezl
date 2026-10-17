import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  AppError,
  EventBus,
  NoteRepo,
  defaultConfig,
  parseHelperLine,
  type Config,
  type PipelineState,
  type PipelineView,
  type SessionView,
  type StepId,
} from '@qrtz/core';
import { openDatabase } from '../worker/db';
import { createWorkerHandlers } from '../worker/handlers';
import { NoteService } from './note-service';
import type { WorkerClient } from './worker-client';

// Tier-2 runtime scenarios (docs/rust-core-api.md §5): spec/scenarios/lifecycle/*.json drive the real
// NoteService + real worker handlers on a file DB with fake timers, a scripted helper and scripted
// summary/index steps. The trace is the oracle for crates/qrtz-lifecycle/tests/scenarios.rs.
// QRTZ_VECTORS_UPDATE=1 rewrites each file's `expect` from this run.

const ids = vi.hoisted(() => ({ n: 0 }));
vi.mock('node:crypto', async (orig) => ({
  ...(await orig<typeof import('node:crypto')>()),
  randomUUID: () => `id${++ids.n}`,
}));

const T0 = 1_760_000_000_000;
const DIR = path.resolve(__dirname, '../../../../spec/scenarios/lifecycle');
const UPDATE = !!process.env.QRTZ_VECTORS_UPDATE;
const nullLog = {
  debug() {},
  info() {},
  warn() {},
  error() {},
  child() {
    return nullLog;
  },
} as never;

/** `slow-ok` / `slow-fail`: the step takes SLOW_MS of (fake) time, then succeeds / fails retryably (N27). */
type Outcome =
  | 'ok'
  | 'fail'
  | 'fatal'
  | 'hang'
  | 'skip'
  | 'slow-ok'
  | 'slow-fail';
const SLOW_MS = 5000;
type Op = { op: string; [k: string]: unknown };
interface Scenario {
  name: string;
  about: string;
  config?: Record<string, unknown>;
  excludedApps?: string[];
  random?: number;
  retryScale?: number;
  faultStep?: StepId;
  script: Op[];
  expect?: unknown;
}

/**
 * N29/H8: the pipeline's capture record, compact ("" when absent, so older scenarios keep their trace):
 * `mic:0-80000` silent spans (`?` = no end: crash mid-stretch), `fb:from>to@atMs` fallbacks (`>-` = to null).
 */
const compactCapture = (c: PipelineView['capture']) =>
  c
    ? [
        ...(c.silentSpans ?? []).map(
          (x) => `${x.source}:${x.startMs}-${x.endMs ?? '?'}`,
        ),
        ...(c.fallbacks ?? []).map(
          (f) => `fb:${f.from}>${f.to ?? '-'}@${f.atMs}`,
        ),
        // N35: device failures, apart from permission revokes (which are not on the meeting record).
        ...(c.deviceFailures ?? []).map(
          (d) => `dev:${d.degraded.join('+')}@${d.atMs}`,
        ),
        // N39: stretches in which a source was lost (revoke, device failure, no source left).
        ...(c.lostSpans ?? []).map(
          (x) => `lost:${x.source}:${x.startMs}-${x.endMs ?? '?'}`,
        ),
      ].join(' ')
    : '';

/** N29: the live silent list in the session trace ("" when none): `s:mic@0,system@900` and `!` for noAudio. */
const compactSilent = (v: SessionView) =>
  v.silent?.length
    ? `s:${v.silent.map((x) => `${x.source}@${x.sinceMs}`).join(',')}${v.noAudio ? '!' : ''}`
    : '';

const compactSteps = (p: { steps: PipelineView['steps'] }) =>
  (['finalize', 'summary', 'index'] as const)
    .map((k) => {
      const s = p.steps[k];
      return `${k}:${s.status}/${s.attempts}${s.fallback ? '+fb' : ''}${s.error ? `!${s.error.code}` : ''}`;
    })
    .join(' ');

function run(sc: Scenario) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'qrtz-sc-'));
  const rec = path.join(dir, 'recordings');
  const rel = (p: unknown) =>
    typeof p === 'string' ? p.split(rec).join('$REC') : (p ?? null);
  const db = openDatabase(path.join(dir, 'qrtz.db'));
  const repo = new NoteRepo(db as DatabaseSync);
  const h = createWorkerHandlers(db, {
    fetch: async () => new Response('{}', { status: 500 }),
  }) as unknown as Record<string, (p: unknown) => unknown>;
  const outcomes: Record<string, Outcome[]> = {
    finalize: [],
    summary: [],
    index: [],
  };
  const failing = new Set<string>();
  /** Undo tombstones (`pendingDelete` op): deletes the user confirmed that have not run yet. */
  const tombstones = new Set<string>();
  const base = defaultConfig();
  const config = {
    ...base,
    note: { ...base.note, retainAudioDays: 0, ...sc.config },
    privacy: { ...base.privacy, excludedApps: sc.excludedApps ?? [] },
  } as Config;
  const trace = {
    session: [] as string[],
    pipelines: {} as Record<string, string[]>,
    helper: [] as string[],
    transcript: [] as unknown[],
    changed: [] as string[],
    permissions: [] as string[],
    prompts: [] as string[],
  };
  const results: Record<string, unknown> = {};
  const snapshots: Record<string, unknown> = {};
  let up = true;

  const never = new Promise<never>(() => {});
  const stepError = (method: string, o: Outcome) =>
    o === 'fatal'
      ? new AppError('E_INTERNAL', `${method} failed`, { retryable: false })
      : new AppError('E_UNAVAILABLE', `${method} unavailable`, {
          retryable: true,
        });
  const scripted = async (
    method: string,
    p: Record<string, unknown>,
  ): Promise<unknown> => {
    const step = method.replace('note.step', '').toLowerCase();
    if (method === 'note.stepSummary' && (p.skip || p.fallback)) {
      if (p.skip) return h[method]!(p);
      const id = p.meetingId as string;
      if (!repo.getMeeting(id))
        throw new AppError('E_NOT_FOUND', `meeting ${id} not found`);
      const s = {
        summary: 'FALLBACK summary',
        actionItems: [],
        qa: [],
        provider: 'extractive',
        llmError: (p.fallback as { error: string }).error.slice(0, 500),
      };
      repo.saveSummary(id, s);
      repo.setStatus(id, 'done');
      return s;
    }
    let o = outcomes[step]!.shift() ?? 'ok';
    if (o === 'hang') return never;
    if (o === 'slow-ok' || o === 'slow-fail') {
      await new Promise((r) => setTimeout(r, SLOW_MS));
      o = o === 'slow-ok' ? 'ok' : 'fail';
    }
    if (o === 'fail' || o === 'fatal') throw stepError(method, o);
    const id = p.meetingId as string;
    if (method === 'note.stepFinalize') return h[method]!(p);
    if (!repo.getMeeting(id))
      throw new AppError('E_NOT_FOUND', `meeting ${id} not found`);
    if (method === 'note.stepIndex')
      return { indexed: 0, skipped: o === 'skip' };
    const s = {
      summary: 'STUB summary',
      actionItems: [],
      qa: [],
      provider: 'stub',
    };
    repo.saveSummary(id, s);
    repo.setStatus(id, 'done');
    return s;
  };

  interface Life {
    note: NoteService;
    dead: boolean;
  }
  const boot = (): Life => {
    const life: Life = { note: undefined as never, dead: false };
    const worker = {
      async call(method: string, params: unknown) {
        if (life.dead) return never;
        if (failing.has(method))
          throw new AppError('E_UNAVAILABLE', `${method} unavailable`, {
            retryable: true,
          });
        const p =
          params === undefined ? undefined : JSON.parse(JSON.stringify(params));
        const r = method.startsWith('note.step')
          ? await scripted(method, p)
          : await h[method]!(p);
        if (life.dead) return never;
        return r === undefined ? r : JSON.parse(JSON.stringify(r));
      },
    } as unknown as WorkerClient;
    const helper = {
      send(line: string) {
        if (life.dead || !up) return false;
        const m = JSON.parse(line) as Record<string, unknown>;
        trace.helper.push(
          [m.type, m.audioPath ?? m.permission]
            .filter(Boolean)
            .map(rel)
            .join(' '),
        );
        return true;
      },
      stop() {
        if (!life.dead) trace.helper.push('restart');
      },
      start() {},
    };
    const bus = new EventBus();
    const last = { session: '' };
    bus.on('note:session', (v: SessionView) => {
      if (life.dead) return;
      const k = [
        v.phase,
        v.meetingId ?? '-',
        v.degraded.join(',') || '-',
        v.errorInfo?.code ?? '-',
        v.reconnect ? `r${v.reconnect.attempt}` : '-',
        v.paused ? 'paused' : '-',
        v.helper,
        ...(compactSilent(v) ? [compactSilent(v)] : []),
        ...(v.deviceLost?.length ? [`d:${v.deviceLost.join(',')}`] : []),
      ].join('|');
      if (k !== last.session) trace.session.push((last.session = k));
    });
    bus.on('note:pipeline', (v: PipelineView) => {
      if (life.dead) return;
      const l = (trace.pipelines[v.meetingId] ??= []);
      const cap = compactCapture(v.capture);
      const k = `${v.stopReason} ${compactSteps(v)}${cap ? ` [${cap}]` : ''}`;
      if (l.at(-1) !== k) l.push(k);
    });
    bus.on('note:transcript', (d) => {
      if (life.dead) return;
      trace.transcript.push([
        d.baseRev,
        d.rev,
        d.append.map((a) => `${a.id}${a.speaker ? `@${a.speaker}` : ''}`),
        d.partial?.text ?? null,
      ]);
    });
    bus.on('note:changed', (v) => {
      if (!life.dead) trace.changed.push(v.id);
    });
    bus.on('permissions:changed', (p) => {
      if (!life.dead)
        trace.permissions.push(
          `${p.accessibility}|${p['screen-recording']}|${p.microphone}`,
        );
    });
    life.note = new NoteService({
      helper: helper as never,
      worker,
      bus,
      log: nullLog,
      secrets: {
        get: async () => undefined,
        set: async () => {},
        delete: async () => {},
      } as never,
      config: () => config,
      recordingsDir: rec,
      onDetectedPrompt: (title, platform) =>
        trace.prompts.push(`${title}|${platform}`),
      retryScale: sc.retryScale ?? 1,
      ...(sc.faultStep ? { faultStep: sc.faultStep } : {}),
      pendingDelete: (id) => tombstones.has(id),
    });
    void life.note.recover();
    return life;
  };

  const settle = async () => {
    for (let i = 0; i < 25; i++) await vi.advanceTimersByTimeAsync(0);
  };

  const viewOf = (v: SessionView) => ({
    phase: v.phase,
    meetingId: v.meetingId ?? null,
    startedAt: v.startedAt ?? null,
    elapsedMs: v.elapsedMs,
    stopReason: v.stopReason ?? null,
    degraded: v.degraded,
    error: v.errorInfo ? [v.errorInfo.code, v.errorInfo.retryable] : null,
    paused: v.paused,
    progress: v.progress ?? null,
    reconnect: v.reconnect ?? null,
    helper: v.helper,
    participants: v.participants,
    activeSpeaker: v.activeSpeaker ?? null,
    lines: v.lines.map((l) => [l.id, l.speaker ?? null, l.text, l.startMs]),
    partial: v.partial,
    pipeline: v.pipeline ? compactSteps(v.pipeline) : null,
    ...(v.silent?.length
      ? {
          silent: v.silent.map((x) => [x.source, x.sinceMs]),
          noAudio: !!v.noAudio,
        }
      : {}),
    ...(v.deviceLost?.length ? { deviceLost: v.deviceLost } : {}),
  });

  const snapshot = (life: Life) => {
    const all = (sql: string) =>
      db.prepare(sql).all() as Record<string, unknown>[];
    const pipelines = all(
      'SELECT meeting_id, json, settled FROM pipeline_state ORDER BY meeting_id',
    ).map((r) => {
      const p = JSON.parse(r.json as string) as PipelineState;
      return [
        p.meetingId,
        r.settled,
        p.stopReason,
        compactSteps(p),
        p.ctx.durationMs,
        p.ctx.helperCrashed,
        p.ctx.audioParts.map(rel),
        p.ctx.active.length,
        ...(p.capture ? [compactCapture(p.capture)] : []),
      ];
    });
    const session = all(
      "SELECT json FROM lifecycle_state WHERE key='session'",
    ).map((r) => {
      const s = JSON.parse(r.json as string);
      return [s.phase, s.meetingId ?? null];
    })[0];
    const lt = life.note.liveTranscript();
    return {
      view: viewOf(life.note.view()),
      live: lt
        ? {
            meetingId: lt.meetingId,
            rev: lt.rev,
            segments: lt.segments.map((s) => [
              s.id,
              s.speaker ?? null,
              s.startMs,
              s.endMs,
            ]),
            partial: lt.partial?.text ?? null,
          }
        : null,
      db: {
        meetings: all(
          'SELECT id, title, platform, status, started_at, ended_at, duration_ms, audio_path, template FROM meetings ORDER BY id',
        ).map((m) => [
          m.id,
          m.title,
          m.platform,
          m.status,
          m.started_at,
          m.ended_at ?? null,
          m.duration_ms ?? null,
          rel(m.audio_path),
          m.template,
        ]),
        segments: all(
          'SELECT meeting_id, id, source, start_ms, end_ms, speaker_name, text FROM segments ORDER BY meeting_id, source, start_ms, id',
        ).map((s) => [
          s.meeting_id,
          s.id,
          s.source,
          s.start_ms,
          s.end_ms,
          s.speaker_name ?? null,
          s.text,
        ]),
        summaries: all(
          'SELECT meeting_id, provider, json FROM summaries ORDER BY meeting_id',
        ).map((s) => {
          const j = JSON.parse(s.json as string);
          return [s.meeting_id, s.provider, j.summary, j.llmError ?? null];
        }),
        notes: all(
          'SELECT id, meeting_id, at_ms, text FROM notes ORDER BY meeting_id, at_ms, id',
        ).map((n) => [n.id, n.meeting_id, n.at_ms, n.text]),
        pipelines,
        session: session ?? null,
      },
      files: fs.existsSync(rec)
        ? fs
            .readdirSync(rec)
            .sort()
            .map((f) => [f, fs.statSync(path.join(rec, f)).size])
        : [],
    };
  };

  const record = (
    label: string,
    p: Promise<unknown>,
    map: (v: never) => unknown,
  ) => {
    results[label] = 'pending';
    p.then(
      (v) => (results[label] = { ok: map(v as never) }),
      (e) =>
        (results[label] = {
          err: [AppError.from(e).code, AppError.from(e).message],
        }),
    );
  };

  const writeWav = (file: string, ms: number, broken: boolean) => {
    fs.mkdirSync(rec, { recursive: true });
    const data = Math.round(ms * 32);
    const b = Buffer.alloc(44 + data);
    b.write('RIFF', 0);
    b.writeUInt32LE(broken ? 36 : 36 + data, 4);
    b.write('WAVE', 8);
    b.write('fmt ', 12);
    b.writeUInt32LE(16, 16);
    b.writeUInt16LE(1, 20);
    b.writeUInt16LE(1, 22);
    b.writeUInt32LE(16000, 24);
    b.writeUInt32LE(32000, 28);
    b.writeUInt16LE(2, 32);
    b.writeUInt16LE(16, 34);
    b.write('data', 36);
    b.writeUInt32LE(broken ? 0 : data, 40);
    fs.writeFileSync(path.join(rec, file), b);
  };

  return {
    async exec() {
      let life = boot();
      await settle();
      let n = 0;
      for (const op of sc.script) {
        const label = (op.label as string) ?? `${op.op}#${++n}`;
        const note = life.note;
        switch (op.op) {
          case 'helper': {
            const r = parseHelperLine(
              JSON.stringify(op.msg).split('$REC').join(rec),
            );
            if (!r.ok)
              throw new Error(
                `invalid helper message in ${sc.name}: ${JSON.stringify(op.msg)}`,
              );
            note.handle(r.msg);
            break;
          }
          case 'helperBatch': {
            // N37: several lines from one stdout chunk. The line splitter hands them to NoteService in one
            // synchronous loop, so no continuation (a start's `await acked`) runs between them.
            for (const msg of op.msgs as unknown[]) {
              const r = parseHelperLine(
                JSON.stringify(msg).split('$REC').join(rec),
              );
              if (!r.ok)
                throw new Error(
                  `invalid helper message in ${sc.name}: ${JSON.stringify(msg)}`,
                );
              note.handle(r.msg);
            }
            break;
          }
          case 'helperInvalid': {
            // A line the host's parser must refuse (as index.ts drops it): it never reaches NoteService.
            const r = parseHelperLine(JSON.stringify(op.msg));
            if (r.ok)
              throw new Error(
                `helperInvalid accepted in ${sc.name}: ${JSON.stringify(op.msg)}`,
              );
            break;
          }
          case 'call': {
            const a = (op.args ?? {}) as Record<string, unknown>;
            const view = (v: SessionView) => ({
              phase: v.phase,
              meetingId: v.meetingId ?? null,
            });
            const sync = (f: () => unknown, map: (v: never) => unknown) =>
              record(label, (async () => f())(), map);
            switch (op.fn) {
              case 'start':
                record(label, note.start(a.title as string | undefined), view);
                break;
              case 'stop':
                record(label, note.stop((a.reason as never) ?? 'manual'), view);
                break;
              case 'toggle':
                record(label, note.toggle(), view);
                break;
              case 'dismiss':
                sync(() => note.dismiss(), view);
                break;
              case 'pause':
                sync(() => note.pause(a.want as boolean | undefined), view);
                break;
              case 'retryStep':
                record(
                  label,
                  note.retryStep(a.id as string, a.step as StepId),
                  compactSteps,
                );
                break;
              case 'deleteMeeting':
                record(
                  label,
                  note.deleteMeeting(a.id as string),
                  (r: { removed: string[] }) =>
                    r.removed.map((f) => path.basename(f)).sort(),
                );
                break;
              case 'pipeline':
                record(
                  label,
                  note.pipeline(a.id as string),
                  (p: PipelineView | null) => (p ? compactSteps(p) : null),
                );
                break;
              case 'addNote':
                record(
                  label,
                  note.addNote(a as never),
                  (r: Record<string, unknown>) => [
                    r.id,
                    r.meetingId,
                    r.atMs,
                    r.text,
                  ],
                );
                break;
              case 'resummarize':
                record(
                  label,
                  note.resummarize(a.id as string),
                  (s: { summary?: string } | null) => s?.summary ?? null,
                );
                break;
              case 'requestPermission':
                record(
                  label,
                  note.requestPermission(a.permission as never),
                  (p: Record<string, string>) =>
                    `${p.accessibility}|${p['screen-recording']}|${p.microphone}`,
                );
                break;
              default:
                throw new Error(`unknown call ${String(op.fn)}`);
            }
            break;
          }
          case 'advance':
            await vi.advanceTimersByTimeAsync(op.ms as number);
            break;
          case 'clockJump':
            // A wall-clock step (NTP, VM resume, manual change): Date moves, pending timers do not.
            vi.setSystemTime(Date.now() + (op.ms as number));
            break;
          case 'helperDown':
            up = false;
            note.onHelperDown((op.reason as string) ?? 'exited');
            break;
          case 'helperUp':
            up = true;
            break;
          case 'helperFailed':
            note.onHelperFailed('failed');
            break;
          case 'helperHealth':
            note.setHelperHealth(
              op.health as never,
              op.detail as string | undefined,
            );
            break;
          case 'steps':
            for (const k of ['finalize', 'summary', 'index'])
              outcomes[k]!.push(...(((op[k] as Outcome[]) ?? []) as Outcome[]));
            break;
          case 'pendingDelete':
            tombstones.add(op.id as string);
            break;
          case 'store':
            for (const m of (op.fail as string[]) ?? []) failing.add(m);
            for (const m of (op.heal as string[]) ?? []) failing.delete(m);
            break;
          case 'wav':
            writeWav(op.file as string, op.ms as number, !!op.broken);
            break;
          case 'db':
            if (op.deleteMeeting) h['note.delete']!({ id: op.deleteMeeting });
            if (op.seedMeeting)
              repo.upsertMeeting(
                JSON.parse(
                  JSON.stringify(op.seedMeeting).split('$REC').join(rec),
                ),
              );
            break;
          case 'restart': {
            // The process dies: every pending timer goes with it (clearAllTimers also resets the fake clock).
            const now = Date.now();
            life.dead = true;
            vi.clearAllTimers();
            vi.setSystemTime(now);
            up = true;
            life = boot();
            break;
          }
          case 'snapshot':
            snapshots[label] = snapshot(life);
            break;
          default:
            throw new Error(`unknown op ${op.op}`);
        }
        await settle();
      }
      snapshots.end = snapshot(life);
      life.dead = true;
      db.close();
      fs.rmSync(dir, { recursive: true, force: true });
      return { trace, results, snapshots };
    },
  };
}

const files = fs.existsSync(DIR)
  ? fs
      .readdirSync(DIR)
      .filter((f) => f.endsWith('.json'))
      .sort()
  : [];

describe('tier-2 lifecycle scenarios (spec/scenarios/lifecycle)', () => {
  beforeEach(() => {
    ids.n = 0;
    vi.useFakeTimers({
      now: T0,
      // N45: fake `performance` too so meeting-relative offsets advance with `advance` and ignore `clockJump`
      // (which only moves Date / the wall clock).
      toFake: [
        'setTimeout',
        'clearTimeout',
        'setInterval',
        'clearInterval',
        'Date',
        'performance',
      ],
    });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('has scenarios', () => expect(files.length).toBeGreaterThan(0));

  for (const f of files) {
    it(f, async () => {
      const file = path.join(DIR, f);
      const sc = JSON.parse(fs.readFileSync(file, 'utf8')) as Scenario;
      vi.spyOn(Math, 'random').mockReturnValue(sc.random ?? 0.5);
      const got = await run(sc).exec();
      if (UPDATE) {
        fs.writeFileSync(
          file,
          `${JSON.stringify({ ...sc, expect: got }, null, 2)}\n`,
        );
        return;
      }
      expect(got).toEqual(sc.expect);
    });
  }
});
