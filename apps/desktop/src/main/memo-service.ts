import { randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import {
  AppError,
  SessionMachine,
  TranscriptAssembler,
  encodeHostMessage,
  type ActiveSpeakerEvent,
  type Config,
  type EventBus,
  type HelperToHost,
  type HostToHelper,
  type Logger,
  type PermissionsStatus,
  type ProviderId,
  type SecretStore,
  type SessionView,
  type Supervisor,
  PROVIDER_DEFAULTS,
} from '@qrtz/core';
import type { WorkerClient } from './worker-client';

/** Orchestrates a meeting: helper events → session state → worker persistence → summary. */
export class MemoService {
  readonly machine = new SessionMachine();
  private asr = new TranscriptAssembler();
  private active: ActiveSpeakerEvent[] = [];
  private participants: string[] = [];
  private level = 0;
  private progress: string | undefined;
  private permissions: PermissionsStatus = {
    accessibility: 'unknown',
    'screen-recording': 'unknown',
    microphone: 'unknown',
  };
  private waiters: { type: string; resolve: (m: HelperToHost) => void }[] = [];
  private emitTimer: NodeJS.Timeout | null = null;
  private audioPath = '';
  private persisted = new Set<string>();

  constructor(
    private readonly deps: {
      helper: Supervisor;
      worker: WorkerClient;
      bus: EventBus;
      log: Logger;
      secrets: SecretStore;
      config: () => Config;
      recordingsDir: string;
    },
  ) {
    this.machine.onChange(() => this.emitSoon(0));
    fs.mkdirSync(deps.recordingsDir, { recursive: true });
  }

  private send(m: HostToHelper) {
    if (!this.deps.helper.send(encodeHostMessage(m)))
      throw new AppError('E_UNAVAILABLE', 'native helper is not running');
  }

  private waitFor<T extends HelperToHost['type']>(
    type: T,
    ms: number,
  ): Promise<Extract<HelperToHost, { type: T }> | null> {
    return new Promise((resolve) => {
      const w = { type, resolve: resolve as (m: HelperToHost) => void };
      this.waiters.push(w);
      setTimeout(() => {
        const i = this.waiters.indexOf(w);
        if (i >= 0) {
          this.waiters.splice(i, 1);
          resolve(null);
        }
      }, ms);
    });
  }

  /** Feed every parsed helper message here. */
  handle(m: HelperToHost) {
    for (const w of this.waiters.slice()) {
      if (w.type === m.type) {
        this.waiters.splice(this.waiters.indexOf(w), 1);
        w.resolve(m);
      }
    }
    const s = this.machine.state;
    switch (m.type) {
      case 'hello':
        this.send({ type: 'permissions.check' });
        break;
      case 'permissions.status':
        this.permissions = { ...this.permissions, ...m.status };
        this.deps.bus.emit('permissions:changed', this.permissions);
        break;
      case 'ax.participants':
        this.participants = m.names;
        this.emitSoon();
        break;
      case 'meeting.detected':
        if (
          this.deps.config().memo.autoDetectMeetings &&
          this.machine.send({ type: 'detected', meeting: m })
        ) {
          this.deps.log.info('meeting detected', {
            platform: m.platform,
            app: m.app,
          });
        }
        break;
      case 'meeting.ended':
        if (s.phase === 'recording' && s.detected?.key === m.key)
          void this.stop().catch(() => {});
        else this.machine.send({ type: 'meeting-ended', key: m.key });
        break;
      case 'ax.activeSpeaker':
        if (s.phase === 'recording') {
          this.active.push({ atMs: m.atMs, name: m.name });
          this.emitSoon();
        }
        break;
      case 'audio.level':
        this.level = m.rms;
        this.emitSoon(150);
        break;
      case 'asr':
        if (s.phase !== 'recording' || !s.meetingId) break;
        this.asr.push(m.event);
        if (m.event.kind === 'final') {
          const seg = this.asr
            .segments()
            .find((x) => x.id === m.event.utteranceId);
          const speaker = this.active
            .filter((a) => a.atMs <= m.event.startMs + 1500)
            .at(-1)?.name;
          if (seg && !this.persisted.has(seg.id)) {
            this.persisted.add(seg.id);
            void this.deps.worker
              .call('memo.appendLive', {
                meetingId: s.meetingId,
                segment: {
                  ...seg,
                  ...(speaker ? { speakerName: speaker } : {}),
                },
              })
              .catch((e) =>
                this.deps.log.warn('appendLive failed', { err: String(e) }),
              );
          }
        }
        this.emitSoon();
        break;
      case 'error':
        this.deps.log.warn('helper error', {
          code: m.code,
          message: m.message,
        });
        break;
    }
  }

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
    const lines = this.asr
      .segments()
      .slice(-6)
      .map((seg) => ({
        id: seg.id,
        text: seg.text,
        startMs: seg.startMs,
        ...(this.speakerAt(seg.startMs)
          ? { speaker: this.speakerAt(seg.startMs)! }
          : {}),
      }));
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
      partial: this.asr.livePartials().at(-1)?.text ?? '',
      ...(this.progress && s.phase === 'processing'
        ? { progress: this.progress }
        : {}),
      ...(s.error ? { error: s.error } : {}),
    };
  }

  private speakerAt(ms: number) {
    return this.active.filter((a) => a.atMs <= ms + 1500).at(-1)?.name;
  }

  private emitSoon(delay = 50) {
    if (this.emitTimer) return;
    this.emitTimer = setTimeout(() => {
      this.emitTimer = null;
      this.deps.bus.emit('memo:session', this.view());
    }, delay);
  }

  async start(title?: string): Promise<SessionView> {
    const s = this.machine.state;
    if (s.phase === 'recording' || s.phase === 'processing')
      throw new AppError('E_VALIDATION', 'already recording');
    const id = randomUUID();
    this.asr = new TranscriptAssembler();
    this.active = [];
    this.persisted.clear();
    this.progress = undefined;
    this.audioPath = path.join(this.deps.recordingsDir, `${id}.wav`);
    const now = Date.now();
    await this.deps.worker.call('memo.create', {
      meeting: {
        id,
        title:
          title ||
          s.detected?.title ||
          `Recording ${new Date(now).toLocaleString()}`,
        platform: s.detected?.platform ?? 'other',
        startedAt: now,
        status: 'recording',
        participants: this.participants,
        audioPath: this.audioPath,
      },
    });
    this.send({ type: 'recording.start', audioPath: this.audioPath });
    this.machine.send({ type: 'start', meetingId: id, at: now });
    this.deps.bus.emit('memo:changed', { id });
    this.deps.log.info('recording started', { id });
    return this.view();
  }

  async stop(): Promise<SessionView> {
    const s = this.machine.state;
    if (s.phase !== 'recording' || !s.meetingId)
      throw new AppError('E_VALIDATION', 'not recording');
    const meetingId = s.meetingId;
    const startedAt = s.startedAt ?? Date.now();
    const stopped = this.waitFor('recording.stopped', 10_000);
    this.send({ type: 'recording.stop' });
    this.machine.send({ type: 'stop', at: Date.now() });
    void this.postProcess(meetingId, startedAt, stopped);
    return this.view();
  }

  dismiss(): SessionView {
    this.machine.send({ type: 'reset' });
    return this.view();
  }

  private async llmParams() {
    const cfg = this.deps.config().memo;
    const provider = cfg.llmProvider as ProviderId | 'none';
    const secretName =
      provider === 'none' ? undefined : PROVIDER_DEFAULTS[provider].secret;
    const apiKey = secretName
      ? await this.deps.secrets.get(secretName)
      : undefined;
    return {
      provider,
      ...(cfg.llmModel ? { model: cfg.llmModel } : {}),
      ...(apiKey ? { apiKey } : {}),
    };
  }

  /** M5.1: finish meetings a crash left in recording/processing. Call once at startup. */
  async recover(): Promise<void> {
    try {
      const r = await this.deps.worker.call(
        'memo.recover',
        { llm: await this.llmParams() },
        300_000,
      );
      for (const id of [...r.recovered, ...r.failed])
        this.deps.bus.emit('memo:changed', { id });
      if (r.recovered.length || r.failed.length)
        this.deps.log.info('recovered meetings', r);
    } catch (e) {
      this.deps.log.warn('recovery failed', { err: String(e) });
    }
  }

  private async postProcess(
    meetingId: string,
    startedAt: number,
    stoppedP: Promise<Extract<
      HelperToHost,
      { type: 'recording.stopped' }
    > | null>,
  ) {
    try {
      const stopped = await stoppedP;
      const durationMs = stopped?.durationMs || Date.now() - startedAt;
      this.progress = 'Summarizing';
      this.emitSoon(0);
      const llm = await this.llmParams();
      await this.deps.worker.call(
        'memo.finalize',
        {
          meetingId,
          endedAt: Date.now(),
          durationMs,
          audioPath: this.audioPath,
          active: this.active,
          llm,
        },
        120_000,
      );
      this.machine.send({ type: 'processed' });
      this.deps.log.info('meeting processed', { id: meetingId });
    } catch (e) {
      this.deps.log.error('post-processing failed', { err: e });
      await this.deps.worker.call('memo.fail', { meetingId }).catch(() => {});
      this.machine.send({ type: 'failed', error: AppError.from(e).message });
    } finally {
      this.deps.bus.emit('memo:changed', { id: meetingId });
      this.emitSoon(0);
    }
  }
}
