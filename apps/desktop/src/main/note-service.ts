import { randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import {
  degradationsForRevocation,
  AppError,
  LifecycleMachine,
  RETRY_POLICIES,
  TranscriptAssembler,
  encodeHostMessage,
  isCapturing,
  canStart,
  newPipeline,
  nextRunnableStep,
  pipelineSettled,
  pipelineStep,
  pipelineUsable,
  retryDelay,
  maxRetryDelay,
  toPipelineView,
  resumeAfterRestart,
  type ActiveSpeakerEvent,
  type Config,
  type Degradation,
  type EventBus,
  type HelperToHost,
  type HostToHelper,
  type LifecycleError,
  type LiveSegment,
  type LiveTranscript,
  type Logger,
  type PermissionsStatus,
  type PipelineState,
  type PipelineView,
  type ProviderId,
  type RecordingState,
  type SecretStore,
  type SessionView,
  type StepId,
  type StopReason,
  type Supervisor,
  PROVIDER_DEFAULTS,
  effectiveLlmProvider,
  isExcludedApp,
  type ChatTurn,
} from '@qrtz/core';
import type { WorkerClient } from './worker-client';

interface Waiter {
  type: string;
  resolve: (m: HelperToHost | null) => void;
  match?: (m: HelperToHost) => boolean;
}
/** Host RPC timeout for one summary attempt, and the worker's LLM deadline inside it (ADR 0005). */
const SUMMARY_RPC_MS = 120_000;
const SUMMARY_DEADLINE_MS = 110_000;
/** Index: worker deadline inside the RPC timeout, like the summary (N3, ADR 0007). */
const INDEX_RPC_MS = 300_000;
const INDEX_DEADLINE_MS = 280_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const toLifecycleError = (
  e: unknown,
  fallbackCode = 'E_INTERNAL',
): LifecycleError => {
  const a = AppError.from(e);
  return {
    code: a.code || fallbackCode,
    message: a.message,
    retryable: a.retryable ?? true,
  };
};

/**
 * Runtime owner of the recording lifecycle (docs/recording-lifecycle.md).
 *
 * - Every transition goes through the pure `lifecycleReducer` (@qrtz/core) via `LifecycleMachine`.
 * - Every accepted transition is persisted (worker SQLite `lifecycle_state`) and broadcast as `note:session`.
 * - Post-processing is a per-meeting pipeline (finalize → summary → index) with retry/backoff, persisted after
 *   every step transition (`pipeline_state`) and broadcast as `note:pipeline`; restarts resume it.
 * - The live transcript is streamed as `note:transcript` deltas with a monotonically increasing `rev`.
 */
export class NoteService {
  readonly machine = new LifecycleMachine();
  private asr = new TranscriptAssembler();
  private active: ActiveSpeakerEvent[] = [];
  private participants: string[] = [];
  private level = 0;
  private permissions: PermissionsStatus = {
    accessibility: 'unknown',
    'screen-recording': 'unknown',
    microphone: 'unknown',
  };
  private waiters: Waiter[] = [];
  /** Audio path of the recording.start in flight (start or resume); acks for any other path are stale. */
  private pendingStartPath: string | null = null;
  /**
   * The helper answered a start with error{recording.busy, audioPath}: another recording still owns it. Keyed by
   * the refused attempt's audioPath, so a busy only ever fails the start it names.
   */
  /**
   * Starts the helper refused, by this attempt's file: `busy` (another recording owns the helper), `file` (the
   * helper could not create this file and tore the recording down), `invariant` (a busy that names no active
   * file: the helper broke "a running recording always has a file"; last-resort watchdog restarts it).
   */
  private busyRefusals = new Map<
    string,
    { kind: 'busy' | 'file' | 'invariant'; activeAudioPath?: string }
  >();
  private emitTimer: NodeJS.Timeout | null = null;
  private persisted = new Set<string>();
  private paused = false;
  private helperCrashed = false;
  private reconnectTimer: NodeJS.Timeout | null = null;
  /** Live transcript stream state. */
  private tRev = 0;
  private tSent = new Set<string>();
  private pipelines = new Map<string, PipelineState>();
  private pipelineTimers = new Map<string, NodeJS.Timeout>();
  /**
   * Per meeting, the `${id}:${step}:${attempts}` whose backoff timer already fired (N12: never wait on the wall
   * clock twice). One entry per meeting at most, dropped with the pipeline (N14).
   */
  private backoffServed = new Map<string, string>();
  private pipelineRunning = new Set<string>();
  private readyResolve!: () => void;
  /** Resolves once the persisted session was loaded and crash recovery queued (start waits on it). */
  readonly ready = new Promise<void>((r) => (this.readyResolve = r));
  private saveChain: Promise<unknown> = Promise.resolve();
  private readonly scale: number;

  constructor(
    private readonly deps: {
      helper: Supervisor;
      worker: WorkerClient;
      bus: EventBus;
      log: Logger;
      secrets: SecretStore;
      config: () => Config;
      recordingsDir: string;
      /** A meeting was detected and auto-record is off: prompt the user (OS notification when unfocused). */
      onDetectedPrompt?: (title: string, platform: string) => void;
      /** Multiplies every retry delay/timeout (tests: QRTZ_RETRY_SCALE=0.01). */
      retryScale?: number;
      /** Fault injection for e2e (unpackaged builds only): fail the first attempt of this step. */
      faultStep?: StepId;
    },
  ) {
    this.scale = deps.retryScale ?? 1;
    this.machine.onChange((s, e) => {
      this.deps.log.info('lifecycle', { event: e.type, phase: s.phase });
      this.persistSession(s);
      // Transitions are broadcast synchronously so no phase is ever coalesced away (level/ASR are throttled).
      this.deps.bus.emit('note:session', this.view());
    });
    // Owner-only: recordings are raw meeting audio (N18). chmod too, for a directory an older build created.
    fs.mkdirSync(deps.recordingsDir, { recursive: true, mode: 0o700 });
    try {
      fs.chmodSync(deps.recordingsDir, 0o700);
    } catch {
      // Not ours to change (e.g. a mounted volume): keep going, the files are still the user's.
    }
  }

  // ------------------------------------------------------------------ persistence

  private persistSession(s: RecordingState) {
    // Serialized so an older snapshot never lands after a newer one.
    const snapshot: RecordingState = s;
    this.saveChain = this.saveChain
      .then(() =>
        this.deps.worker.call('lifecycle.saveSession', { state: snapshot }),
      )
      .catch((e) =>
        this.deps.log.warn('session persist failed', { err: String(e) }),
      );
  }

  /**
   * Saves the pipeline. `gone`: the worker refused because the meeting was deleted meanwhile; the caller must
   * stop driving it (N16). `failed`: the save itself failed (retryable).
   */
  private persistPipeline(
    p: PipelineState,
  ): Promise<'saved' | 'gone' | 'failed'> {
    this.pipelines.set(p.meetingId, p);
    this.deps.bus.emit('note:pipeline', toPipelineView(p));
    if (p.meetingId === this.machine.state.meetingId) this.emitSoon(0);
    return this.deps.worker
      .call('pipeline.save', { state: p, settled: pipelineSettled(p) })
      .then(
        (r) => (r && r.ok === false ? 'gone' : 'saved'),
        (e) => {
          this.deps.log.warn('pipeline persist failed', { err: String(e) });
          return 'failed' as const;
        },
      );
  }

  /**
   * The meeting was deleted: drop every bit of host state for its pipeline (timer, served backoff, manual
   * flags) so nothing runs or retries for it again (N14, N16). The session lets go of it too.
   */
  private forgetPipeline(id: string) {
    const t = this.pipelineTimers.get(id);
    if (t) clearTimeout(t);
    this.pipelineTimers.delete(id);
    this.pipelines.delete(id);
    this.backoffServed.delete(id);
    for (const k of this.manualSteps)
      if (k.startsWith(`${id}:`)) this.manualSteps.delete(k);
    const s = this.machine.state;
    if (
      s.meetingId === id &&
      (s.phase === 'processing' || s.phase === 'error' || s.phase === 'done')
    ) {
      if (s.phase === 'processing')
        this.machine.send({ type: 'processing-done' });
      this.dismiss();
    }
  }

  /**
   * User delete (immediate, or the deferred one after its undo window): forget the pipeline first so no
   * retry timer can race the delete, drop the rows, then remove the meeting's audio (N15). Only files inside
   * the recordings directory are removed: the paths come from the DB, which a restored backup also wrote.
   */
  async deleteMeeting(id: string): Promise<{ removed: string[] }> {
    this.forgetPipeline(id);
    return this.dropMeeting(id);
  }

  /** Rows, then files (no session or pipeline side effects). */
  private async dropMeeting(id: string): Promise<{ removed: string[] }> {
    const r = await this.deps.worker.call('note.delete', { id });
    const removed: string[] = [];
    for (const f of meetingAudio(this.deps.recordingsDir, id, r.files ?? [])) {
      if (!insideDir(this.deps.recordingsDir, f)) {
        this.deps.log.warn('not removing audio outside recordings', { id });
        continue;
      }
      try {
        fs.rmSync(f, { force: true });
        removed.push(f);
      } catch (e) {
        this.deps.log.warn('audio delete failed', { id, err: String(e) });
      }
    }
    return { removed };
  }

  /** Pipeline sizes for tests and diagnostics (N14: nothing outlives a deleted meeting). */
  pipelineDebug() {
    return {
      pipelines: this.pipelines.size,
      timers: this.pipelineTimers.size,
      backoffServed: this.backoffServed.size,
      manualSteps: this.manualSteps.size,
    };
  }

  // ------------------------------------------------------------------ helper I/O

  private send(m: HostToHelper) {
    if (!this.deps.helper.send(encodeHostMessage(m)))
      throw new AppError('E_UNAVAILABLE', 'native helper is not running', {
        retryable: true,
      });
  }

  /**
   * Wait for a helper message. `match` correlates it to this request (e.g. the audio path of this start/stop),
   * so a late ack from an aborted attempt can never satisfy a newer one. The timer is cleared on resolution.
   */
  private waitFor<T extends HelperToHost['type']>(
    type: T,
    ms: number,
    match?: (m: Extract<HelperToHost, { type: T }>) => boolean,
  ): Promise<Extract<HelperToHost, { type: T }> | null> {
    return new Promise((resolve) => {
      let timer: NodeJS.Timeout | undefined;
      const w: Waiter = {
        type,
        resolve: (m) => {
          if (timer) clearTimeout(timer);
          resolve(m as Extract<HelperToHost, { type: T }>);
        },
        ...(match ? { match: match as (m: HelperToHost) => boolean } : {}),
      };
      timer = setTimeout(() => {
        const i = this.waiters.indexOf(w);
        if (i >= 0) {
          this.waiters.splice(i, 1);
          resolve(null);
        }
      }, ms);
      this.waiters.push(w);
    });
  }

  private degrade(flag: Degradation) {
    if (this.machine.send({ type: 'degraded', flag }))
      this.deps.log.warn('capture degraded', { flag });
  }

  /** Feed every parsed helper message here. */
  handle(m: HelperToHost) {
    for (const w of this.waiters.slice()) {
      if (w.type === m.type && (!w.match || w.match(m))) {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        w.resolve(m);
      }
    }
    const s = this.machine.state;
    switch (m.type) {
      case 'hello':
        this.send({ type: 'permissions.check' });
        if (s.phase === 'reconnecting') void this.resumeAfterRestart();
        break;
      case 'permissions.status':
        this.permissions = { ...this.permissions, ...m.status };
        this.deps.bus.emit('permissions:changed', this.permissions);
        break;
      case 'permission.revoked': {
        // H5: TCC withdrawn mid-capture. Degrade the running recording live (arbiter rules, shared with the
        // helper) and reflect the new state in Settings/onboarding.
        const denied = (
          Object.keys(this.permissions) as (keyof PermissionsStatus)[]
        ).filter((k) => this.permissions[k] === 'denied');
        for (const f of degradationsForRevocation(m.permission, denied))
          this.degrade(f);
        this.permissions = { ...this.permissions, [m.permission]: 'denied' };
        this.deps.bus.emit('permissions:changed', this.permissions);
        this.deps.log.warn('permission revoked mid-capture', {
          permission: m.permission,
        });
        break;
      }
      case 'ax.participants':
        this.participants = m.names;
        this.emitSoon();
        break;
      case 'meeting.detected':
        if (isExcludedApp(this.deps.config(), m)) {
          this.deps.log.info('meeting ignored (excluded app)', {
            platform: m.platform,
          });
          break;
        }
        const mc = this.deps.config().note;
        if (
          mc.autoDetectMeetings &&
          (mc.autoRecord || mc.detectPrompt) &&
          this.machine.send({ type: 'detected', meeting: m })
        ) {
          this.deps.log.info('meeting detected', {
            platform: m.platform,
            app: m.app,
          });
          if (this.deps.config().note.autoRecord) {
            this.deps.log.info('auto-record: starting');
            void this.start().catch((e) =>
              this.deps.log.warn('auto-record failed', { err: String(e) }),
            );
          } else this.deps.onDetectedPrompt?.(m.title, m.platform);
        }
        break;
      case 'meeting.ended':
        if (isCapturing(s.phase) && s.detected?.key === m.key)
          void this.stop('meeting-ended').catch(() => {});
        else this.machine.send({ type: 'meeting-ended', key: m.key });
        break;
      case 'ax.activeSpeaker':
        if (isCapturing(s.phase)) {
          this.active.push({ atMs: m.atMs + s.partOffsetMs, name: m.name });
          this.emitSoon();
        }
        break;
      case 'recording.started':
        // A late ack for a start we already abandoned (timeout, give-up): never leave that capture running.
        if (
          m.audioPath !== this.pendingStartPath &&
          !(isCapturing(s.phase) && s.audioParts.includes(m.audioPath))
        ) {
          this.deps.log.warn('stale recording.started; stopping it');
          try {
            // With the path, so a late started{A} can never stop the recording that replaced it.
            this.send({ type: 'recording.stop', audioPath: m.audioPath });
          } catch {
            /* helper gone */
          }
        }
        break;
      case 'audio.level':
        this.level = m.rms;
        this.emitSoon(150);
        break;
      case 'asr':
        // `stopping` is accepted: the helper flushes the recogniser's last finals before recording.stopped.
        // Paused: drop anything still in flight from the helper (it stops feeding the recogniser too).
        if (
          (s.phase !== 'recording' && s.phase !== 'stopping') ||
          !s.meetingId ||
          this.paused
        )
          break;
        this.onAsr(s, m.event);
        break;
      case 'error':
        this.deps.log.warn('helper error', {
          code: m.code,
          message: m.message,
        });
        if (m.code === 'recording.busy') this.onBusy(s, m);
        else if (m.code === 'recording.file') this.onFileFailed(m);
        else if (m.code === 'asr.unavailable') this.degrade('asr-unavailable');
        else if (m.code === 'audio.unavailable')
          this.degrade('audio-unavailable');
        break;
    }
  }

  /**
   * error{recording.busy, audioPath, activeAudioPath}: the helper ignored recording.start{audioPath} because the
   * recording writing `activeAudioPath` owns it. Acts only on the start in flight. A busy naming that same file as
   * active is a duplicate start of a recording that is running: treat it as the ack. If the active file is not one
   * of ours (an orphan from before a reload or an abandoned attempt), stop it by its path before failing this
   * attempt, so the retry can start; never stop a recording this service is capturing.
   */
  private onBusy(
    s: RecordingState,
    m: Extract<HelperToHost, { type: 'error' }>,
  ) {
    const path = m.audioPath;
    if (!path || path !== this.pendingStartPath) {
      this.deps.log.warn('recording.busy for another start; ignored', {
        audioPath: path,
      });
      return;
    }
    if (m.activeAudioPath === path) {
      this.resolveWaiters('recording.started', {
        type: 'recording.started',
        audioPath: path,
      });
      return;
    }
    const active = m.activeAudioPath;
    if (!active) {
      // The helper says a recording runs but names no file: it broke its own invariant (a running recording always
      // has a file). Nothing can stop that recording by path, and a pathless stop is never sent (it would stop
      // whatever runs). This is a bug, not a normal path: the start fails and the helper is restarted (watchdog).
      this.deps.log.error(
        'BUG: helper invariant broken: recording.busy names no active recording file; restarting the helper',
        { audioPath: path },
      );
      this.busyRefusals.set(path, { kind: 'invariant' });
      this.resolveWaiters('recording.started', null, (w) =>
        w.match
          ? w.match({ type: 'recording.started', audioPath: path })
          : true,
      );
      return;
    }
    const ours =
      !!active && isCapturing(s.phase) && s.audioParts.includes(active);
    if (active && !ours) {
      this.deps.log.warn('stopping orphaned recording', { audioPath: active });
      try {
        this.send({ type: 'recording.stop', audioPath: active });
      } catch {
        /* helper gone */
      }
    }
    // The start was ignored: fail it now instead of waiting out the ack timeout.
    this.busyRefusals.set(path, { kind: 'busy', activeAudioPath: active });
    this.resolveWaiters('recording.started', null, (w) =>
      w.match ? w.match({ type: 'recording.started', audioPath: path }) : true,
    );
  }

  /**
   * error{recording.file, audioPath}: the helper could not create the file of recording.start{audioPath}. It has
   * already torn that recording down (fail closed: mic and taps released, no recording.started follows), so the
   * start in flight fails now instead of waiting out the ack timeout. Any other recording.file error (e.g. a WAV
   * that would not finalize at stop) only logs.
   */
  private onFileFailed(m: Extract<HelperToHost, { type: 'error' }>) {
    const path = m.audioPath;
    if (!path || path !== this.pendingStartPath) return;
    this.busyRefusals.set(path, { kind: 'file' });
    this.resolveWaiters('recording.started', null, (w) =>
      w.match ? w.match({ type: 'recording.started', audioPath: path }) : true,
    );
  }

  private resolveWaiters(
    type: string,
    m: HelperToHost | null,
    pick: (w: Waiter) => boolean = (w) => (m && w.match ? w.match(m) : true),
  ) {
    for (const w of this.waiters.slice())
      if (w.type === type && pick(w)) {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        w.resolve(m);
      }
  }

  private onAsr(
    s: RecordingState,
    ev: Extract<HelperToHost, { type: 'asr' }>['event'],
  ) {
    // Each helper (re)start restarts its clock and utterance ids: shift into meeting time, namespace ids.
    const part = s.audioParts.length - 1;
    const e = {
      ...ev,
      utteranceId: part > 0 ? `p${part}-${ev.utteranceId}` : ev.utteranceId,
      startMs: ev.startMs + s.partOffsetMs,
      ...(ev.kind === 'final' ? { endMs: ev.endMs + s.partOffsetMs } : {}),
    } as typeof ev;
    this.asr.push(e);
    if (e.kind === 'final') {
      const seg = this.asr.segments().find((x) => x.id === e.utteranceId);
      const speaker = this.speakerAt(e.startMs);
      if (seg && !this.persisted.has(seg.id)) {
        this.persisted.add(seg.id);
        void this.appendLive(s.meetingId!, {
          ...seg,
          ...(speaker ? { speakerName: speaker } : {}),
        });
      }
    }
    this.emitTranscript(s.meetingId!);
    this.emitSoon();
  }

  /** Persist a final segment with retry (idempotent upsert on (meeting, source, id)). */
  private async appendLive(
    meetingId: string,
    segment: ReturnType<TranscriptAssembler['segments']>[number],
  ) {
    const p = RETRY_POLICIES.appendLive;
    for (let attempt = 1; ; attempt++) {
      try {
        await this.deps.worker.call('note.appendLive', { meetingId, segment });
        return;
      } catch (e) {
        if (attempt >= p.maxAttempts) {
          // Not lost: still in the in-memory assembler; finalize re-flushes everything before it runs.
          this.deps.log.error('appendLive gave up; will flush at finalize', {
            err: String(e),
          });
          this.persisted.delete(segment.id);
          return;
        }
        await sleep(retryDelay(p, attempt, Math.random, this.scale));
      }
    }
  }

  // ------------------------------------------------------------------ live transcript stream

  private liveSegments(): LiveSegment[] {
    return this.asr.segments().map((seg) => {
      const speaker = this.speakerAt(seg.startMs);
      return {
        id: seg.id,
        text: seg.text,
        startMs: seg.startMs,
        endMs: seg.endMs,
        ...(speaker ? { speaker } : {}),
      };
    });
  }
  private livePartial(): LiveTranscript['partial'] {
    const p = this.asr.livePartials().at(-1);
    return p ? { id: p.utteranceId, text: p.text, startMs: p.startMs } : null;
  }

  /** Snapshot for the meeting being recorded (or null). Renderer fetches this, then applies deltas. */
  liveTranscript(meetingId?: string): LiveTranscript | null {
    const s = this.machine.state;
    if (!s.meetingId || (meetingId && meetingId !== s.meetingId)) return null;
    return {
      meetingId: s.meetingId,
      rev: this.tRev,
      segments: this.liveSegments(),
      partial: isCapturing(s.phase) ? this.livePartial() : null,
    };
  }

  private emitTranscript(meetingId: string) {
    const append = this.liveSegments().filter((x) => !this.tSent.has(x.id));
    for (const a of append) this.tSent.add(a.id);
    const baseRev = this.tRev++;
    this.deps.bus.emit('note:transcript', {
      meetingId,
      baseRev,
      rev: this.tRev,
      append,
      partial: isCapturing(this.machine.state.phase)
        ? this.livePartial()
        : null,
    });
  }

  // ------------------------------------------------------------------ views

  getPermissions() {
    return this.permissions;
  }
  requestPermission(p: keyof PermissionsStatus) {
    this.send({ type: 'permissions.request', permission: p });
    return this.waitFor('permissions.status', 5000).then(
      () => this.permissions,
    );
  }

  view(): SessionView {
    const s = this.machine.state;
    const lines = this.liveSegments()
      .slice(-6)
      .map(({ endMs: _e, ...l }) => l);
    const pipeline = s.meetingId ? this.pipelines.get(s.meetingId) : undefined;
    const live = isCapturing(s.phase);
    return {
      phase: s.phase,
      ...(s.detected ? { detected: s.detected } : {}),
      ...(s.meetingId ? { meetingId: s.meetingId } : {}),
      ...(s.startedAt ? { startedAt: s.startedAt } : {}),
      elapsedMs: s.startedAt ? (s.stoppedAt ?? Date.now()) - s.startedAt : 0,
      participants: this.participants,
      ...(this.active.at(-1)
        ? { activeSpeaker: this.active.at(-1)!.name }
        : {}),
      level: s.phase === 'recording' ? this.level : 0,
      lines,
      partial: live ? (this.livePartial()?.text ?? '') : '',
      ...(s.phase === 'processing'
        ? { progress: progressLabel(pipeline) }
        : {}),
      ...(s.error ? { error: s.error.message, errorInfo: s.error } : {}),
      helper: this.helperHealth,
      ...(this.helperDetail ? { helperDetail: this.helperDetail } : {}),
      paused: s.phase === 'recording' && this.paused,
      ...(s.stopReason ? { stopReason: s.stopReason } : {}),
      degraded: s.degraded,
      ...(s.reconnect ? { reconnect: s.reconnect } : {}),
      ...(pipeline ? { pipeline: toPipelineView(pipeline) } : {}),
    };
  }

  /** Pipeline of any meeting (live session or a past one). */
  async pipeline(id: string): Promise<PipelineView | null> {
    const p =
      this.pipelines.get(id) ??
      (await this.deps.worker.call('pipeline.get', { id }));
    return p ? toPipelineView(p) : null;
  }

  /** Privacy pause/resume while recording (hotkey, HUD). Omit `want` to toggle. */
  pause(want?: boolean): SessionView {
    const s = this.machine.state;
    if (s.phase !== 'recording')
      throw new AppError('E_VALIDATION', 'not recording');
    const next = want ?? !this.paused;
    if (next !== this.paused) {
      this.send({ type: next ? 'recording.pause' : 'recording.resume' });
      this.paused = next;
      this.deps.log.info(next ? 'capture paused' : 'capture resumed');
    }
    this.deps.bus.emit('note:session', this.view());
    return this.view();
  }

  /** Live note: defaults to the meeting being recorded, stamped at the current recording offset. */
  async addNote(req: { meetingId?: string; text: string; atMs?: number }) {
    const s = this.machine.state;
    const live = !req.meetingId || req.meetingId === s.meetingId;
    const meetingId = req.meetingId ?? s.meetingId;
    if (!meetingId)
      throw new AppError('E_VALIDATION', 'no meeting is being recorded');
    const atMs =
      req.atMs ??
      (live && s.startedAt ? (s.stoppedAt ?? Date.now()) - s.startedAt : 0);
    return this.deps.worker.call('note.addNote', {
      meetingId,
      text: req.text,
      atMs,
    });
  }

  /**
   * Template picker. Routed like `resummarize`: through the pipeline's summary step when there is one (heavy-step
   * queue, worker deadline, retries, fallback), else one deadline-bounded call. It used to be a bare
   * `note.resummarize` on the default 10 s RPC timeout: a real LLM summary outlived it, the UI got E_TIMEOUT
   * while the worker kept going and saved later, outside the queue (cycle 3, N9, ADR 0008).
   */
  async setTemplate(id: string, template: string) {
    const p =
      this.pipelines.get(id) ??
      (await this.deps.worker.call('pipeline.get', { id }));
    if (p && p.steps.finalize.status === 'done') {
      await this.deps.worker.call('note.setTemplate', { id, template });
      return this.resummarize(id);
    }
    const summary = await this.deps.worker.call(
      'note.resummarize',
      {
        id,
        template,
        llm: await this.llmParams(),
        deadlineMs: SUMMARY_DEADLINE_MS,
      },
      SUMMARY_RPC_MS,
    );
    this.deps.bus.emit('note:changed', { id });
    return summary;
  }

  private helperHealth: SessionView['helper'] = 'ok';
  private helperDetail: string | undefined;
  /** Main reports helper availability/supervisor state; surfaced in the HUD, Note and Home. */
  setHelperHealth(h: SessionView['helper'], detail?: string): void {
    if (h === this.helperHealth && detail === this.helperDetail) return;
    this.helperHealth = h;
    this.helperDetail = detail;
    this.deps.bus.emit('note:session', this.view());
  }

  private speakerAt(ms: number) {
    return this.active.filter((a) => a.atMs <= ms + 1500).at(-1)?.name;
  }

  private emitSoon(delay = 50) {
    if (this.emitTimer) return;
    this.emitTimer = setTimeout(() => {
      this.emitTimer = null;
      this.deps.bus.emit('note:session', this.view());
    }, delay);
  }

  // ------------------------------------------------------------------ start

  private startInFlight = false;
  async start(title?: string): Promise<SessionView> {
    // Two starts racing (hotkey double-press, auto-record + click) must not create two meetings.
    if (this.startInFlight)
      throw new AppError('E_VALIDATION', 'already starting');
    this.startInFlight = true;
    try {
      return await this.startInner(title);
    } finally {
      this.startInFlight = false;
    }
  }

  private async startInner(title?: string): Promise<SessionView> {
    await this.ready;
    if (this.helperHealth === 'missing')
      throw new AppError(
        'E_UNAVAILABLE',
        'Recording is unavailable: the native capture helper is missing from this build.',
      );
    const s = this.machine.state;
    if (!canStart(s.phase))
      throw new AppError('E_VALIDATION', 'already recording');
    // Permission gate: with neither system audio nor microphone there is nothing to capture.
    const p = this.permissions;
    if (p['screen-recording'] === 'denied' && p.microphone === 'denied') {
      const error: LifecycleError = {
        code: 'permission-denied',
        message:
          'Recording needs Screen & System Audio Recording or Microphone access. Grant one in System Settings › Privacy & Security, then try again.',
        retryable: true,
      };
      // Surface it as state (not only a rejected promise) so every window renders the same thing.
      this.setIdleError(error);
      throw new AppError('E_FORBIDDEN', error.message);
    }
    const id = randomUUID();
    this.asr = new TranscriptAssembler();
    this.active = [];
    this.persisted.clear();
    this.tSent.clear();
    this.tRev = 0;
    this.paused = false;
    this.helperCrashed = false;
    const audioPath = path.join(this.deps.recordingsDir, `${id}.wav`);
    const now = Date.now();
    await this.deps.worker.call('note.create', {
      meeting: {
        id,
        title:
          title ||
          s.detected?.title ||
          `Recording ${new Date(now).toLocaleString()}`, // check-titles-ignore (sample user content)
        platform: s.detected?.platform ?? 'other',
        startedAt: now,
        status: 'recording',
        participants: this.participants,
        audioPath,
        template: this.deps.config().note.defaultTemplate,
      },
    });
    this.machine.send({
      type: 'start-requested',
      meetingId: id,
      at: now,
      audioPath,
    });
    this.deps.bus.emit('note:changed', { id });
    this.pendingStartPath = audioPath;
    this.busyRefusals.delete(audioPath);
    const acked = this.waitFor(
      'recording.started',
      RETRY_POLICIES.startAckMs * Math.max(this.scale, 0.2),
      (m) => m.audioPath === audioPath,
    );
    try {
      this.send({ type: 'recording.start', audioPath });
    } catch (e) {
      if (this.machine.state.phase === 'starting') {
        await this.abortStart(id, toLifecycleError(e));
        throw e;
      }
      return this.view();
    }
    const ok = await acked;
    if (this.pendingStartPath === audioPath) this.pendingStartPath = null;
    if (!ok) {
      // The helper may have died meanwhile: then `reconnecting` owns the meeting, do not delete it.
      if (
        this.machine.state.meetingId !== id ||
        this.machine.state.phase !== 'starting'
      )
        return this.view();
      // Only a refusal that named this attempt's file counts; one for any other start never relabels it.
      const refusal = this.busyRefusals.get(audioPath);
      this.busyRefusals.delete(audioPath);
      const error: LifecycleError =
        refusal?.kind === 'file'
          ? {
              code: 'recording-file',
              message:
                'The recording file could not be created. Check free disk space, then try again.',
              retryable: true,
            }
          : refusal
            ? {
                code: 'helper-busy',
                message:
                  'The capture helper is still busy with another recording. Try again.',
                retryable: true,
              }
            : {
                code: 'start-timeout',
                message:
                  'The capture helper did not start recording. Try again.',
                retryable: true,
              };
      if (refusal?.kind !== 'invariant')
        try {
          // Only this attempt's file: never the recording that made the helper busy.
          this.send({ type: 'recording.stop', audioPath });
        } catch {
          /* helper gone */
        }
      await this.abortStart(id, error);
      // Last-resort watchdog: a helper that broke the file invariant is restarted (the supervisor brings a fresh one
      // back). After abortStart, so the restart never looks like a crash of this (already failed) start.
      if (refusal?.kind === 'invariant') {
        this.deps.helper.stop();
        this.deps.helper.start();
      }
      throw new AppError(
        refusal ? 'E_UNAVAILABLE' : 'E_TIMEOUT',
        error.message,
        { retryable: true },
      );
    }
    if (
      this.machine.state.meetingId !== id ||
      this.machine.state.phase !== 'starting'
    )
      return this.view(); // helper crashed during start: reconnect path owns it now
    this.machine.send({ type: 'start-acked' });
    if (p['screen-recording'] === 'denied') this.degrade('mic-only');
    if (p.accessibility === 'denied') this.degrade('ax-unavailable');
    this.deps.log.info('recording started', { id });
    return this.view();
  }

  private setIdleError(error: LifecycleError) {
    this.machine.send({ type: 'rejected', error });
  }

  private async abortStart(id: string, error: LifecycleError) {
    this.machine.send({ type: 'start-failed', error });
    // Nothing was captured: drop the empty meeting (and any file the helper began) instead of a broken row.
    await this.dropMeeting(id).catch(() => {});
    this.deps.bus.emit('note:changed', { id });
  }

  // ------------------------------------------------------------------ helper crash / reconnect

  /**
   * The helper went away (crash, kill, heartbeat timeout). While capturing, keep the meeting open in
   * `reconnecting`: the supervisor restarts the helper with backoff and `hello` resumes into a new WAV part.
   * If it is not back within the reconnect window (or the supervisor gives up), stop with `helper-lost` and
   * post-process what was captured.
   */
  onHelperDown(reason: string): void {
    const s = this.machine.state;
    if (!isCapturing(s.phase) || !s.meetingId) return;
    this.helperCrashed = true;
    const now = Date.now();
    const window = RETRY_POLICIES.helperReconnectWindowMs * this.scale;
    this.deps.log.warn('helper went away mid-recording; reconnecting', {
      reason,
    });
    this.machine.send({
      type: 'helper-lost',
      at: now,
      deadlineAt: now + window,
    });
    if (!this.reconnectTimer)
      this.reconnectTimer = setTimeout(() => {
        this.reconnectTimer = null;
        if (this.machine.state.phase === 'reconnecting')
          void this.giveUpReconnect('reconnect window elapsed');
      }, window);
  }

  /** Supervisor circuit breaker opened (too many restarts): stop reconnecting now. */
  onHelperFailed(reason: string): void {
    if (this.machine.state.phase === 'reconnecting')
      void this.giveUpReconnect(reason);
  }

  private async giveUpReconnect(reason: string) {
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    this.deps.log.warn('helper did not come back; salvaging', { reason });
    await this.stop('helper-lost').catch((e) =>
      this.deps.log.error('salvage stop failed', { err: String(e) }),
    );
  }

  private async resumeAfterRestart() {
    const s = this.machine.state;
    if (s.phase !== 'reconnecting' || !s.meetingId) return;
    const part = s.audioParts.length;
    const audioPath = path.join(
      this.deps.recordingsDir,
      `${s.meetingId}.p${part}.wav`,
    );
    this.pendingStartPath = audioPath;
    const acked = this.waitFor(
      'recording.started',
      RETRY_POLICIES.startAckMs * Math.max(this.scale, 0.2),
      (m) => m.audioPath === audioPath,
    );
    // Offset from when capture was asked to begin, not from the ack (ack latency would shift timestamps late).
    const at = Date.now();
    try {
      this.send({ type: 'recording.start', audioPath });
      if (this.paused) this.send({ type: 'recording.pause' });
    } catch {
      return; // went down again; the next hello retries
    }
    const ok = await acked;
    if (this.pendingStartPath === audioPath) this.pendingStartPath = null;
    this.busyRefusals.delete(audioPath);
    if (!ok || this.machine.state.phase !== 'reconnecting') return;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    // A fresh helper starts unpaused: re-apply a privacy pause before anything is transcribed.
    if (this.paused) {
      try {
        this.send({ type: 'recording.pause' });
      } catch {
        /* down again; next hello retries */
      }
    }
    this.machine.send({ type: 'helper-restored', at, audioPath });
    this.deps.log.info('recording resumed after helper restart', { part });
  }

  // ------------------------------------------------------------------ stop

  /**
   * N2: `recording.abort{audioPath}` with a hard deadline. True when the helper answered
   * `recording.aborted{audioPath}` (no writer on that file remains); false on timeout or a dead helper.
   */
  private async abortRecording(audioPath: string): Promise<boolean> {
    const p = this.waitFor(
      'recording.aborted',
      RETRY_POLICIES.abortAckMs * Math.max(this.scale, 0.2),
      (m) => m.audioPath === audioPath,
    );
    try {
      this.send({ type: 'recording.abort', audioPath });
    } catch {
      return false;
    }
    return !!(await p);
  }

  async stop(reason: StopReason = 'manual'): Promise<SessionView> {
    const s = this.machine.state;
    // Idempotent: a second stop (double-click, tray + hotkey, meeting.ended racing a click) is a no-op.
    if (s.meetingId && (s.phase === 'stopping' || s.phase === 'processing'))
      return this.view();
    if (!isCapturing(s.phase) || !s.meetingId)
      throw new AppError('E_VALIDATION', 'not recording');
    const meetingId = s.meetingId;
    const startedAt = s.startedAt ?? Date.now();
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    const wasLive = s.phase !== 'reconnecting';
    this.machine.send({ type: 'stop-requested', reason, at: Date.now() });
    let stopped: Extract<HelperToHost, { type: 'recording.stopped' }> | null =
      null;
    if (wasLive) {
      const part = s.audioParts.at(-1);
      const p = this.waitFor(
        'recording.stopped',
        RETRY_POLICIES.stopAckMs * Math.max(this.scale, 0.2),
        (m) => !part || m.audioPath === part,
      );
      try {
        this.send({ type: 'recording.stop' });
        stopped = await p;
        if (!stopped) {
          // Still flushing after the timeout: it may still be writing the WAV finalize is about to repair. First
          // abort just that recording (scoped by its file, hard deadline), so a dictation sharing the helper
          // survives; only if the abort is not acked either, restart the helper so there is no concurrent writer.
          const aborted = part ? await this.abortRecording(part) : false;
          if (aborted)
            this.deps.log.warn(
              'recording.stop not acked; aborted the recording (helper kept)',
            );
          else {
            this.deps.log.warn(
              'recording.stop and recording.abort not acked; restarting helper',
            );
            this.deps.helper.stop();
            this.deps.helper.start();
          }
        }
      } catch {
        this.helperCrashed = true; // helper down: finish from what's persisted
      }
    } else {
      // Giving up a reconnect: a helper that came back late may have opened a part; never leave it capturing.
      try {
        this.send({ type: 'recording.stop' });
      } catch {
        /* still down */
      }
    }
    this.paused = false;
    const st = this.machine.state;
    // Flush any final segment whose live append gave up.
    // Same retry policy as live appends; whatever still fails is logged loudly (doc: known limits).
    for (const seg of this.asr.segments())
      if (!this.persisted.has(seg.id)) {
        this.persisted.add(seg.id);
        await this.appendLive(meetingId, seg);
      }
    const endedAt = Date.now();
    const pipeline = newPipeline(
      meetingId,
      reason,
      {
        startedAt,
        endedAt,
        durationMs: stopped?.durationMs || endedAt - startedAt,
        audioParts: st.audioParts,
        active: this.active,
        helperCrashed: this.helperCrashed || !stopped,
      },
      endedAt,
    );
    await this.persistPipeline(pipeline);
    this.machine.send({ type: 'stopped', at: endedAt });
    this.emitTranscript(meetingId); // final rev: partial cleared
    void this.runPipeline(meetingId);
    return this.view();
  }

  dismiss(): SessionView {
    if (!this.machine.send({ type: 'reset' }))
      this.machine.send({ type: 'clear-error' });
    return this.view();
  }

  /** Global shortcut: start recording when idle/detected, stop when recording. */
  async toggle(): Promise<SessionView> {
    return isCapturing(this.machine.state.phase) ? this.stop() : this.start();
  }

  // ------------------------------------------------------------------ post-processing pipeline

  /** User action: retry one failed step (and everything after it). */
  async retryStep(id: string, step: StepId): Promise<PipelineView> {
    let p =
      this.pipelines.get(id) ??
      (await this.deps.worker.call('pipeline.get', { id }));
    if (!p)
      throw new AppError('E_NOT_FOUND', `no post-processing record for ${id}`);
    if (this.pipelineRunning.has(id) && p.steps[step].status === 'running')
      return toPipelineView(p);
    const t = this.pipelineTimers.get(id);
    if (t) clearTimeout(t);
    this.pipelineTimers.delete(id);
    this.backoffServed.delete(id);
    p = pipelineStep(p, step, { kind: 'manual-retry', now: Date.now() });
    // A manual run always executes, even when Settings turned the automatic step off.
    this.manualSteps.add(`${id}:${step}`);
    await this.persistPipeline(p);
    void this.runPipeline(id);
    return toPipelineView(p);
  }

  private async runPipeline(id: string): Promise<void> {
    if (this.pipelineRunning.has(id)) return;
    this.pipelineRunning.add(id);
    try {
      for (;;) {
        let p = this.pipelines.get(id);
        if (!p) return;
        const step = nextRunnableStep(p);
        if (!step) break;
        const cur = p.steps[step];
        // Wait out the backoff (timer survives in-memory; a restart resumes immediately). nextRetryAt is wall
        // clock, so the wait is capped at the longest delay this attempt could have drawn, and once that timer
        // has fired the step runs: a clock that jumped back must not stall it for the size of the jump (N12).
        const backoff = `${id}:${step}:${cur.attempts}`;
        const served = this.backoffServed.get(id) === backoff;
        this.backoffServed.delete(id);
        if (
          !served &&
          cur.status === 'retrying' &&
          cur.nextRetryAt &&
          cur.nextRetryAt > Date.now()
        ) {
          const delay = Math.min(
            cur.nextRetryAt - Date.now(),
            maxRetryDelay(RETRY_POLICIES[step], cur.attempts, this.scale),
          );
          this.pipelineTimers.set(
            id,
            setTimeout(() => {
              this.pipelineTimers.delete(id);
              this.backoffServed.set(id, backoff);
              void this.runPipeline(id);
            }, delay),
          );
          return;
        }
        p = pipelineStep(p, step, { kind: 'start', now: Date.now() });
        const started = await this.persistPipeline(p);
        if (started === 'gone') return this.forgetPipeline(id);
        try {
          // Durability barrier: never run a step whose start is not on disk (a crash would re-run it blind).
          if (started !== 'saved')
            throw new AppError(
              'E_UNAVAILABLE',
              'could not persist step start',
              {
                retryable: true,
              },
            );
          const skipped = await this.execStep(p, step);
          // A manual retry re-armed this step while it was in flight: its result is stale, drop it.
          if (this.pipelines.get(id)!.steps[step].status !== 'running')
            continue;
          p = pipelineStep(this.pipelines.get(id)!, step, {
            kind: skipped ? 'skip' : 'success',
            now: Date.now(),
          });
        } catch (e) {
          const err = toLifecycleError(e);
          const latest = this.pipelines.get(id)!;
          if (latest.steps[step].status !== 'running') continue; // superseded by a manual retry
          const policy = RETRY_POLICIES[step];
          p = pipelineStep(latest, step, {
            kind: 'failure',
            now: Date.now(),
            error: err,
            delayMs: retryDelay(
              policy,
              latest.steps[step].attempts,
              Math.random,
              this.scale,
            ),
          });
          this.deps.log.warn('pipeline step failed', {
            id,
            step,
            attempt: latest.steps[step].attempts,
            status: p.steps[step].status,
            err: err.message,
          });
          if (step === 'summary' && p.steps.summary.status === 'failed') {
            // Out of attempts: the meeting still gets the on-device summary; the step stays failed + retryable.
            try {
              await this.deps.worker.call('note.stepSummary', {
                meetingId: id,
                llm: await this.llmParams(),
                fallback: { error: err.message },
              });
              p = {
                ...p,
                steps: {
                  ...p.steps,
                  summary: { ...p.steps.summary, fallback: true },
                },
              };
            } catch (fe) {
              this.deps.log.error('fallback summary failed', {
                err: String(fe),
              });
            }
          }
          if (step === 'finalize' && p.steps.finalize.status === 'failed')
            await this.deps.worker
              .call('note.fail', { meetingId: id })
              .catch(() => {});
        }
        if ((await this.persistPipeline(p)) === 'gone')
          return this.forgetPipeline(id);
        this.deps.bus.emit('note:changed', { id });
        this.syncSessionWithPipeline(p);
      }
    } finally {
      this.pipelineRunning.delete(id);
    }
  }

  /**
   * Network/CPU-heavy steps (summary, index) run one at a time across all meetings, so a recovered backlog or
   * synchronized retry timers can never burst the LLM provider. Finalize is local and runs immediately.
   */
  private heavyChain: Promise<unknown> = Promise.resolve();
  private execStep(p: PipelineState, step: StepId): Promise<boolean> {
    if (step === 'finalize') return this.execStepInner(p, step);
    const run = this.heavyChain.then(() => this.execStepInner(p, step));
    this.heavyChain = run.catch(() => {});
    return run;
  }

  /** Returns true when the step was skipped (nothing to do on this machine). */
  private async execStepInner(
    p: PipelineState,
    step: StepId,
  ): Promise<boolean> {
    if (
      this.deps.faultStep === step &&
      !this.faultsFired.has(`${p.meetingId}:${step}`)
    ) {
      // Fault injection (e2e, unpackaged only): fail this step's first attempt.
      this.faultsFired.add(`${p.meetingId}:${step}`);
      throw new AppError('E_UNAVAILABLE', `injected ${step} failure`, {
        retryable: true,
      });
    }
    const manual = this.manualSteps.delete(`${p.meetingId}:${step}`);
    const mc = this.deps.config().note;
    switch (step) {
      case 'finalize':
        await this.deps.worker.call(
          'note.stepFinalize',
          { meetingId: p.meetingId, ctx: p.ctx },
          60_000,
        );
        return false;
      case 'summary':
        if (!mc.autoSummarize && !manual) {
          // Settings → Summarize after meetings is off: finish without a summary (Summarize button later).
          await this.deps.worker.call('note.stepSummary', {
            meetingId: p.meetingId,
            llm: { provider: 'none' },
            skip: true,
          });
          return true;
        }
        // The worker aborts the LLM call at deadlineMs and saves nothing; our RPC timeout is longer, so a
        // timed-out attempt is really over before the next one (or the fallback) starts (ADR 0005).
        await this.deps.worker.call(
          'note.stepSummary',
          {
            meetingId: p.meetingId,
            llm: await this.llmParams(),
            deadlineMs: SUMMARY_DEADLINE_MS,
          },
          SUMMARY_RPC_MS,
        );
        return false;
      case 'index': {
        if (!mc.backgroundIndexing && !manual) return true; // Settings → Index for search off
        const r = await this.deps.worker.call(
          'note.stepIndex',
          { meetingId: p.meetingId, deadlineMs: INDEX_DEADLINE_MS },
          INDEX_RPC_MS,
        );
        return r.skipped;
      }
    }
  }
  private faultsFired = new Set<string>();
  private manualSteps = new Set<string>();

  /** The session's own meeting: processing → done once usable, → error if finalize failed for good. */
  private syncSessionWithPipeline(p: PipelineState) {
    const s = this.machine.state;
    // `error` too: a manual finalize retry that succeeds must bring the session back to done.
    if (
      s.meetingId !== p.meetingId ||
      (s.phase !== 'processing' && s.phase !== 'error')
    )
      return;
    if (pipelineUsable(p)) this.machine.send({ type: 'processing-done' });
    else if (s.phase === 'processing' && p.steps.finalize.status === 'failed')
      this.machine.send({
        type: 'processing-failed',
        error: p.steps.finalize.error ?? {
          code: 'E_INTERNAL',
          message: 'post-processing failed',
          retryable: true,
        },
      });
  }

  // ------------------------------------------------------------------ restart recovery

  /**
   * Startup: (1) a session persisted mid-capture means the app died while recording → open a pipeline for it
   * (stop reason crash-recovered); (2) resume every unsettled pipeline; (3) adopt orphaned pre-lifecycle
   * meetings stuck in recording/processing. Idempotent: running it twice resumes, never duplicates.
   */
  async recover(): Promise<void> {
    try {
      const saved = await this.deps.worker.call(
        'lifecycle.loadSession',
        undefined as never,
      );
      const now = Date.now();
      if (
        saved &&
        saved.meetingId &&
        (isCapturing(saved.phase) || saved.phase === 'stopping')
      ) {
        const existing = await this.deps.worker.call('pipeline.get', {
          id: saved.meetingId,
        });
        if (!existing) {
          this.deps.log.warn('app died mid-recording; recovering', {
            id: saved.meetingId,
          });
          await this.persistPipeline(
            newPipeline(
              saved.meetingId,
              'crash-recovered',
              {
                startedAt: saved.startedAt ?? now,
                endedAt: saved.stoppedAt ?? now,
                durationMs: 0, // finalize derives it from the repaired audio
                audioParts: saved.audioParts,
                active: [],
                helperCrashed: true,
              },
              now,
            ),
          );
        }
      }
      for (const m of await this.deps.worker.call(
        'pipeline.orphans',
        undefined as never,
      )) {
        await this.persistPipeline(
          newPipeline(
            m.id,
            'crash-recovered',
            {
              startedAt: m.startedAt,
              endedAt: m.endedAt ?? now,
              durationMs: m.durationMs ?? 0,
              audioParts: m.audioPath ? [m.audioPath] : [],
              active: [],
              helperCrashed: true,
            },
            now,
          ),
        );
      }
      // The session itself always restarts idle (nothing is capturing after a restart).
      if (saved && saved.phase !== 'idle')
        this.persistSession(this.machine.state);
      this.deps.bus.emit('note:session', this.view());
      const pending = await this.deps.worker.call(
        'pipeline.listUnsettled',
        undefined as never,
      );
      for (const raw of pending) {
        // A step caught mid-run by the crash is re-run (every step is idempotent); attempts are kept.
        const p = resumeAfterRestart(raw);
        this.pipelines.set(p.meetingId, p);
        this.deps.bus.emit('note:pipeline', toPipelineView(p));
      }
      // One at a time: a backlog after a long outage must not fire N LLM calls at once.
      void (async () => {
        for (const p of pending) await this.runPipeline(p.meetingId);
      })();
      if (pending.length)
        this.deps.log.info('resuming post-processing', {
          count: pending.length,
        });
    } catch (e) {
      this.deps.log.warn('recovery failed', { err: String(e) });
    } finally {
      this.readyResolve();
    }
    try {
      const days = this.deps.config().note.retainAudioDays;
      if (days > 0) {
        const p = await this.deps.worker.call('note.pruneAudio', { days });
        for (const id of p.pruned) this.deps.bus.emit('note:changed', { id });
      }
    } catch (e) {
      this.deps.log.warn('audio prune failed', { err: String(e) });
    }
  }

  // ------------------------------------------------------------------ LLM

  private async llmParams() {
    const full = this.deps.config();
    const cfg = full.note;
    // Privacy "local only": cloud providers are replaced by the on-device path (keys are never read).
    const provider = effectiveLlmProvider(full) as ProviderId | 'none';
    const secretName =
      provider === 'none' ? undefined : PROVIDER_DEFAULTS[provider].secret;
    const apiKey = secretName
      ? await this.deps.secrets.get(secretName)
      : undefined;
    return {
      provider,
      ...(cfg.llmModel ? { model: cfg.llmModel } : {}),
      ...(apiKey ? { apiKey } : {}),
      ...(process.env.QRTZ_LLM_BASE_URL
        ? { baseUrl: process.env.QRTZ_LLM_BASE_URL }
        : {}),
    };
  }

  async chat(req: {
    question: string;
    meetingId?: string;
    history?: ChatTurn[];
  }) {
    return this.deps.worker.call(
      'note.chat',
      { ...req, llm: await this.llmParams() },
      120_000,
    );
  }

  /** Retry the summary for one meeting (after an LLM failure or a provider/key change). */
  async resummarize(id: string) {
    const p =
      this.pipelines.get(id) ??
      (await this.deps.worker.call('pipeline.get', { id }));
    if (p && p.steps.finalize.status === 'done') {
      // Route through the pipeline so retries/backoff/progress are tracked like any other step.
      await this.retryStep(id, 'summary');
      await this.waitSettled(id, 'summary');
      const s = await this.deps.worker.call('note.get', { id });
      return s.summary!;
    }
    const summary = await this.deps.worker.call(
      'note.resummarize',
      { id, llm: await this.llmParams(), deadlineMs: SUMMARY_DEADLINE_MS },
      SUMMARY_RPC_MS,
    );
    this.deps.bus.emit('note:changed', { id });
    return summary;
  }

  private async waitSettled(id: string, step: StepId) {
    for (let i = 0; i < 2400; i++) {
      const st = this.pipelines.get(id)?.steps[step].status;
      if (st === 'done' || st === 'failed' || st === 'skipped') return;
      await sleep(50);
    }
  }
}

function progressLabel(p: PipelineState | undefined): string {
  if (!p) return 'Finishing';
  const step = nextRunnableStep(p);
  const s = step ? p.steps[step] : undefined;
  const name = {
    finalize: 'Saving transcript',
    summary: 'Summarizing',
    index: 'Indexing',
  };
  if (!step || !s) return 'Finishing';
  return s.status === 'retrying' ? `${name[step]} (retrying)` : name[step];
}

/** True when `file` resolves to a path strictly inside `dir` (no `..` escape, no absolute elsewhere). */
export function insideDir(dir: string, file: string): boolean {
  const rel = path.relative(path.resolve(dir), path.resolve(file));
  return !!rel && rel.split(path.sep)[0] !== '..' && !path.isAbsolute(rel);
}

/**
 * The files to remove for a deleted meeting: the ones the DB listed plus any `<id>.wav` / `<id>.p<n>.wav` left in
 * the recordings directory (a part written after the last pipeline save). Deduplicated.
 */
export function meetingAudio(
  dir: string,
  id: string,
  listed: readonly string[],
): string[] {
  const out = new Set(listed.map((f) => path.resolve(f)));
  const own = /^(.+?)(?:\.p\d+)?\.wav$/;
  try {
    for (const name of fs.readdirSync(dir))
      if (own.exec(name)?.[1] === id) out.add(path.resolve(dir, name));
  } catch {
    // No recordings directory yet: nothing of this meeting on disk.
  }
  return [...out];
}
