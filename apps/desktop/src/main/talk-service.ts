import { randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import {
  EngineArbiter,
  TalkEntry,
  DictionaryEntry,
  cleanupDictation,
  deepgramStream,
  elevenLabsStream,
  idleTalkView,
  type CleanupLlm,
  type CloudStream,
  type Config,
  type EventBus,
  type HelperToHost,
  type HostToHelper,
  type Logger,
  type SecretStore,
  type TalkEngineId,
  type TalkView,
  type WsFactory,
} from '@qrtz/core';

type InsertResult = Extract<HelperToHost, { type: 'insert.result' }>;

export interface TalkDeps {
  send: (m: HostToHelper) => void;
  bus: EventBus;
  log: Logger;
  config: () => Config;
  secrets: SecretStore;
  /** JSON file holding history + dictionary. */
  storePath: string;
  ws: WsFactory;
  /** Builds the cleanup LLM call (null = none configured). */
  llm: () => Promise<CleanupLlm | null>;
  writeClipboard: (text: string) => void;
  now?: () => number;
  /** Timeouts (ms); tests shrink them. */
  timeouts?: Partial<{
    started: number;
    stopped: number;
    insert: number;
    cloudStall: number;
    cloudFinal: number;
  }>;
}

const CLOUD_ENGINES = ['elevenlabs', 'deepgram'] as const;
type CloudEngine = (typeof CLOUD_ENGINES)[number];
const SECRET: Record<CloudEngine, string> = {
  deepgram: 'asr.deepgram',
  elevenlabs: 'asr.elevenlabs',
};
const MAX_HISTORY = 500;

/**
 * qrtz talk: push-to-talk dictation. idle → listening → finalizing → cleaning → inserting → idle (or error).
 * Audio and on-device ASR come from the helper; a cloud WebSocket engine runs alongside when configured, and the
 * EngineArbiter fails over to on-device text when it breaks. Every step has a timeout so a dictation always ends.
 */
export class TalkService {
  private view: TalkView = idleTalkView();
  private arbiter: EngineArbiter | null = null;
  private cloud: CloudStream | null = null;
  private keyDownAt = 0;
  private keyUpAt = 0;
  private firstPartialAt: number | undefined;
  private viaPushKey = false;
  /** Audio that arrived while the cloud socket was being created. */
  private pendingAudio: Buffer[] = [];
  private stallTimer: ReturnType<typeof setInterval> | null = null;
  private waiters = new Map<string, (m: HelperToHost | null) => void>();
  private store: { history: TalkEntry[]; dictionary: DictionaryEntry[] };
  private cloudKeys = new Set<CloudEngine>();
  private t: Required<NonNullable<TalkDeps['timeouts']>>;

  constructor(private readonly deps: TalkDeps) {
    this.t = {
      started: 3000,
      stopped: 3000,
      insert: 2500,
      cloudStall: 1500,
      cloudFinal: 1500,
      ...deps.timeouts,
    };
    this.store = this.load();
  }

  private get now() {
    return this.deps.now ?? Date.now;
  }

  private load() {
    try {
      const raw = JSON.parse(fs.readFileSync(this.deps.storePath, 'utf8')) as {
        history?: unknown[];
        dictionary?: unknown[];
      };
      return {
        history: (raw.history ?? []).flatMap((h) => {
          const r = TalkEntry.safeParse(h);
          return r.success ? [r.data] : [];
        }),
        dictionary: (raw.dictionary ?? []).flatMap((d) => {
          const r = DictionaryEntry.safeParse(d);
          return r.success ? [r.data] : [];
        }),
      };
    } catch {
      return { history: [], dictionary: [] };
    }
  }

  private save() {
    try {
      fs.mkdirSync(path.dirname(this.deps.storePath), { recursive: true });
      const tmp = `${this.deps.storePath}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify(this.store));
      fs.renameSync(tmp, this.deps.storePath);
    } catch (e) {
      this.deps.log.warn('talk store write failed', { err: String(e) });
    }
  }

  state(): TalkView {
    return this.view;
  }
  history(): TalkEntry[] {
    return this.store.history;
  }
  dictionary(): DictionaryEntry[] {
    return this.store.dictionary;
  }
  setDictionary(entries: DictionaryEntry[]) {
    const seen = new Set<string>();
    this.store.dictionary = entries.filter((e) => {
      const k = e.term.toLowerCase();
      return seen.has(k) ? false : (seen.add(k), true);
    });
    this.save();
    return this.store.dictionary;
  }
  deleteEntry(id: string) {
    const n = this.store.history.length;
    this.store.history = this.store.history.filter((h) => h.id !== id);
    this.save();
    this.deps.bus.emit('talk:history-changed', { id });
    return { ok: this.store.history.length !== n };
  }
  clearHistory() {
    this.store.history = [];
    this.save();
    this.deps.bus.emit('talk:history-changed', { id: '*' });
    return { ok: true };
  }
  copy(id: string) {
    const e = this.store.history.find((h) => h.id === id);
    if (e) this.deps.writeClipboard(e.text);
    return { ok: !!e };
  }

  private set(patch: Partial<TalkView>) {
    this.view = { ...this.view, ...patch };
    this.deps.bus.emit('talk:state', this.view);
  }

  /** Which engine a new dictation uses. Local-only privacy and missing keys fall back to on-device. */
  resolveEngine(cfg = this.deps.config()): TalkEngineId {
    if (cfg.privacy.localOnly || cfg.talk.engine === 'on-device')
      return 'on-device';
    if (cfg.talk.engine !== 'auto')
      return this.cloudKeys.has(cfg.talk.engine)
        ? cfg.talk.engine
        : 'on-device';
    return CLOUD_ENGINES.find((e) => this.cloudKeys.has(e)) ?? 'on-device';
  }

  /** Re-read which cloud keys exist and push settings to the helper. Call on start, helper hello and config change. */
  async configure() {
    const keys = new Set<CloudEngine>();
    for (const e of CLOUD_ENGINES)
      if (await this.deps.secrets.get(SECRET[e])) keys.add(e);
    if (process.env.QRTZ_TALK_CLOUD) keys.add('deepgram'); // dev/e2e fake cloud engine
    this.cloudKeys = keys;
    const c = this.deps.config().talk;
    this.deps.send({
      type: 'dictation.configure',
      warmMic: c.enabled && c.warmMic,
      preRollMs: c.preRollMs,
      streamAudio: this.resolveEngine() !== 'on-device',
      pushKey: c.enabled ? c.pushKey : '',
    });
  }

  handle(m: HelperToHost) {
    switch (m.type) {
      case 'hello':
        void this.configure();
        return;
      case 'hotkey':
        if (!this.deps.config().talk.enabled) return;
        if (m.action === 'down' && !this.active()) {
          this.viaPushKey = true;
          void this.start();
        } else if (m.action === 'up' && this.viaPushKey) {
          this.viaPushKey = false;
          void this.stop();
        }
        return;
      case 'dictation.started':
        this.waiters.get(`started:${m.id}`)?.(m);
        return;
      case 'dictation.level':
        if (this.view.phase === 'listening') this.set({ level: m.rms });
        return;
      case 'dictation.audio':
        if (m.id !== this.view.id) return;
        if (this.cloud) this.cloud.push(Buffer.from(m.pcm16, 'base64'));
        else if (
          this.view.engine !== 'on-device' &&
          this.pendingAudio.length < 300
        )
          this.pendingAudio.push(Buffer.from(m.pcm16, 'base64'));
        return;
      case 'dictation.asr':
        if (m.id !== this.view.id || !this.arbiter) return;
        if (m.event.kind === 'partial') this.arbiter.localPartial(m.event.text);
        else this.arbiter.localFinal(m.event.text);
        return;
      case 'dictation.stopped': {
        const w = this.waiters.get(`stopped:${m.id}`);
        if (w) return w(m);
        // H5: the helper ended the dictation itself (microphone revoked). Stop gracefully: keep and insert what
        // was heard so far, then show why it stopped.
        if (
          m.reason &&
          this.view.id === m.id &&
          this.view.phase === 'listening'
        )
          void this.endedByHelper(m);
        return;
      }
      case 'permission.revoked':
        if (m.permission === 'microphone' && this.active())
          this.deps.log.warn('microphone revoked mid-dictation');
        return;
      case 'insert.result':
        this.waiters.get(`insert:${m.id}`)?.(m);
        return;
      case 'error':
        if (m.code.startsWith('dictation.')) {
          this.deps.log.warn('dictation error', {
            code: m.code,
            message: m.message,
          });
          if (m.code === 'dictation.mic' && this.active()) this.fail(m.message);
          // Our start was ignored because another dictation still owns the helper: fail now, and release any
          // waiter so a stop for this id can never hang on a dictation.stopped that will not come.
          if (m.code === 'dictation.busy' && this.active() && this.view.id) {
            const id = this.view.id;
            this.waiters.get(`started:${id}`)?.(null);
            this.waiters.get(`stopped:${id}`)?.(null);
            this.fail('another dictation is still running');
          }
        }
        return;
    }
  }

  private active() {
    return !['idle', 'error'].includes(this.view.phase);
  }

  private wait<T extends HelperToHost>(
    key: string,
    ms: number,
  ): Promise<T | null> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => done(null), ms);
      const done = (m: HelperToHost | null) => {
        clearTimeout(timer);
        this.waiters.delete(key);
        resolve(m as T | null);
      };
      this.waiters.set(key, done);
    });
  }

  private refreshText() {
    if (!this.arbiter) return;
    const v = this.arbiter.view();
    if ((v.committed || v.partial) && this.firstPartialAt === undefined)
      this.firstPartialAt = this.now();
    this.set({
      text: v.committed,
      partial: v.partial,
      engine: this.arbiter.engine,
      failover: this.arbiter.failover,
    });
  }

  private makeCloud(
    engine: TalkEngineId,
    onText: (c: string, p: string) => void,
    onError: (e: string) => void,
  ) {
    if (engine === 'on-device') return null;
    return (async () => {
      const key = process.env.QRTZ_TALK_CLOUD
        ? 'fake'
        : await this.deps.secrets.get(SECRET[engine]);
      if (!key) return null;
      const cb = { onText, onError };
      return engine === 'deepgram'
        ? deepgramStream({
            ws: this.deps.ws,
            apiKey: key,
            dictionary: this.store.dictionary,
            cb,
          })
        : elevenLabsStream({ ws: this.deps.ws, apiKey: key, cb });
    })();
  }

  async start(): Promise<TalkView> {
    if (this.active()) return this.view;
    const id = randomUUID();
    const engine = this.resolveEngine();
    this.keyDownAt = this.now();
    this.firstPartialAt = undefined;
    this.view = {
      ...idleTalkView(),
      phase: 'listening',
      id,
      startedAt: this.keyDownAt,
      engine,
    };
    const startedP = this.wait(`started:${id}`, this.t.started);
    // Ask the helper first: the mic must open as early as possible; the socket connects in parallel.
    this.deps.send({ type: 'dictation.start', id });
    this.set({});
    const autoFailover = this.deps.config().talk.autoFailover;
    const arbiter = new EngineArbiter({
      cloud: null,
      // Auto-failover off: never give up on a silent cloud stream early (errors and timeouts still fall back).
      stallMs: autoFailover ? this.t.cloudStall : Number.POSITIVE_INFINITY,
      finalMs: this.t.cloudFinal,
      now: this.now,
      onChange: () => this.refreshText(),
    });
    this.arbiter = arbiter;
    try {
      const cloud = await this.makeCloud(
        engine,
        (c, p) => arbiter.cloudUpdate(c, p),
        (e) => {
          this.deps.log.warn('cloud asr failed', { engine, err: e });
          arbiter.cloudError(e);
        },
      );
      if (cloud && this.arbiter === arbiter) {
        this.cloud = cloud;
        for (const a of this.pendingAudio.splice(0)) cloud.push(a);
        arbiter.attachCloud(cloud);
      } else cloud?.close();
    } catch (e) {
      this.deps.log.warn('cloud asr unavailable', { engine, err: String(e) });
    }
    this.stallTimer = setInterval(() => this.arbiter?.check(), 250);
    const started = await startedP;
    if (!started && this.view.id === id && this.view.phase === 'listening')
      this.fail('Microphone did not start');
    return this.view;
  }

  async stop(): Promise<TalkView> {
    if (this.view.phase !== 'listening' || !this.view.id) return this.view;
    const id = this.view.id;
    this.keyUpAt = this.now();
    this.set({ phase: 'finalizing', level: 0 });
    const stoppedP = this.wait<
      Extract<HelperToHost, { type: 'dictation.stopped' }>
    >(`stopped:${id}`, this.t.stopped);
    this.deps.send({ type: 'dictation.stop', id });
    const stopped = await stoppedP;
    void this.finish(
      id,
      stopped?.text ?? '',
      stopped?.durationMs ?? this.keyUpAt - this.keyDownAt,
    );
    return this.view;
  }

  async cancel(): Promise<TalkView> {
    if (!this.active() || !this.view.id) return this.view;
    this.deps.send({ type: 'dictation.cancel', id: this.view.id });
    this.cleanupSession();
    this.set({
      ...idleTalkView(),
      ...(this.view.last ? { last: this.view.last } : {}),
    });
    return this.view;
  }

  private cleanupSession() {
    if (this.stallTimer) clearInterval(this.stallTimer);
    this.stallTimer = null;
    this.cloud?.close();
    this.cloud = null;
    this.arbiter = null;
    this.pendingAudio = [];
  }

  private async endedByHelper(
    m: Extract<HelperToHost, { type: 'dictation.stopped' }>,
  ) {
    const id = m.id;
    this.keyUpAt = this.now();
    this.set({ phase: 'finalizing', level: 0 });
    await this.finish(
      id,
      m.text,
      m.durationMs || this.keyUpAt - this.keyDownAt,
    );
    this.set({
      phase: 'error',
      error:
        m.reason === 'permission-revoked'
          ? 'microphone access was turned off, so dictation stopped'
          : 'the microphone went away, so dictation stopped',
      level: 0,
    });
  }

  private fail(error: string) {
    if (this.view.id)
      this.deps.send({ type: 'dictation.cancel', id: this.view.id });
    this.cleanupSession();
    this.set({ phase: 'error', error, level: 0 });
  }

  private async finish(id: string, localText: string, durationMs: number) {
    const arbiter = this.arbiter;
    if (!arbiter || this.view.id !== id) return;
    const asr = await arbiter.finish(localText);
    const finalAt = this.now();
    this.cleanupSession();
    if (!asr.text.trim()) {
      this.set({
        ...idleTalkView(),
        ...(this.view.last ? { last: this.view.last } : {}),
      });
      return;
    }
    const cfg = this.deps.config();
    this.set({
      phase: 'cleaning',
      text: asr.text,
      partial: '',
      engine: asr.engine,
      failover: asr.failover,
    });
    let llm: CleanupLlm | null = null;
    if (cfg.talk.autoCleanup && cfg.talk.aiCleanup && !cfg.privacy.localOnly) {
      try {
        llm = await this.deps.llm();
      } catch {
        llm = null;
      }
    }
    const cleaned = await cleanupDictation({
      raw: asr.text,
      dictionary: this.store.dictionary,
      enabled: cfg.talk.autoCleanup,
      llm,
      timeoutMs: cfg.talk.cleanupTimeoutMs,
      now: this.now,
    });
    if (cleaned.fallbackReason)
      this.deps.log.info('talk cleanup fell back to rules', {
        reason: cleaned.fallbackReason,
      });
    let insert: InsertResult | null = null;
    let error: string | undefined;
    if (cfg.talk.autoInsert) {
      this.set({ phase: 'inserting', text: cleaned.text });
      const p = this.wait<InsertResult>(`insert:${id}`, this.t.insert);
      this.deps.send({
        type: 'insert.text',
        id,
        text: cleaned.text,
        restoreClipboard: cfg.talk.restoreClipboard,
      });
      insert = await p;
      if (!insert?.ok) {
        // Never lose the words: leave them on the clipboard to paste by hand.
        this.deps.writeClipboard(cleaned.text);
        error = `${insert?.error ?? 'insert timed out'}; copied to clipboard`;
      }
    }
    const insertedAt = this.now();
    const entry: TalkEntry = {
      id,
      at: this.keyDownAt,
      raw: asr.text,
      text: cleaned.text,
      cleanup: cleaned.method,
      engine: asr.engine,
      failover: asr.failover,
      insert: insert?.method ?? 'none',
      inserted: !!insert?.ok,
      ...(insert?.app ? { app: insert.app } : {}),
      durationMs,
      latency: {
        ...(this.firstPartialAt !== undefined
          ? { firstPartialMs: this.firstPartialAt - this.keyDownAt }
          : {}),
        finalMs: finalAt - this.keyUpAt,
        cleanupMs: cleaned.ms,
        ...(insert?.ok ? { insertedMs: insertedAt - this.keyUpAt } : {}),
      },
      ...(error ? { error } : {}),
    };
    this.deps.log.info('talk latency', {
      ...entry.latency,
      engine: entry.engine,
      insert: entry.insert,
      cleanup: entry.cleanup,
    });
    if (cfg.talk.saveHistory) {
      this.store.history = [entry, ...this.store.history].slice(0, MAX_HISTORY);
      this.save();
      this.deps.bus.emit('talk:history-changed', { id });
    }
    this.set({ ...idleTalkView(), last: entry });
  }
}
