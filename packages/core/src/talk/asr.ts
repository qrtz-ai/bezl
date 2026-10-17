import type { DictionaryEntry, TalkEngineId } from './contract';

/**
 * Cloud streaming ASR for qrtz talk. Both engines are true real-time WebSocket streams with incremental partials;
 * batch-only APIs are not eligible (docs/research-talk.md). The on-device Parakeet stream in the helper always runs
 * alongside as the offline fallback, so failover never has to re-send audio.
 */
export interface WsLike {
  readonly readyState: number;
  send(data: string | Uint8Array): void;
  close(code?: number): void;
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null;
}
export type WsFactory = (
  url: string,
  init: { protocols?: string[]; headers?: Record<string, string> },
) => WsLike;

export interface CloudStream {
  readonly engine: Exclude<TalkEngineId, 'on-device'>;
  /** PCM16 LE mono 16 kHz. Buffered until the socket opens. */
  push(pcm: Uint8Array): void;
  /** Flush and resolve with the full final text. */
  finish(): Promise<string>;
  close(): void;
}

export interface CloudCallbacks {
  /** committed: text that will not change; partial: the current hypothesis after it. */
  onText(committed: string, partial: string): void;
  onError(err: string): void;
}

const OPEN = 1;
const join = (a: string, b: string) => (a && b ? `${a} ${b}` : a || b).trim();
const b64 = (u: Uint8Array) => {
  let s = '';
  for (let i = 0; i < u.length; i += 0x8000)
    s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s);
};

function socketBase(ws: WsLike, cb: CloudCallbacks) {
  const queue: (string | Uint8Array)[] = [];
  let failed = false;
  const fail = (e: string) => {
    if (failed) return;
    failed = true;
    cb.onError(e);
  };
  ws.onopen = () => {
    for (const q of queue.splice(0)) ws.send(q);
  };
  ws.onerror = () => fail('websocket error');
  const send = (d: string | Uint8Array) =>
    ws.readyState === OPEN ? ws.send(d) : void queue.push(d);
  return { send, fail, isFailed: () => failed };
}

/** Deepgram streaming (/v1/listen, Nova-3 by default): interim_results partials, Finalize to flush. */
export function deepgramStream(o: {
  ws: WsFactory;
  apiKey: string;
  model?: string;
  dictionary?: readonly DictionaryEntry[];
  cb: CloudCallbacks;
}): CloudStream {
  const q = new URLSearchParams({
    model: o.model || 'nova-3',
    encoding: 'linear16',
    sample_rate: '16000',
    channels: '1',
    interim_results: 'true',
    smart_format: 'true',
    punctuate: 'true',
    endpointing: '300',
  });
  for (const d of o.dictionary ?? []) q.append('keyterm', d.term);
  // Browser-style WebSocket can't set headers; Deepgram accepts the key as the "token" subprotocol.
  const ws = o.ws(`wss://api.deepgram.com/v1/listen?${q}`, {
    protocols: ['token', o.apiKey],
  });
  const base = socketBase(ws, o.cb);
  let committed = '';
  let finishing: ((t: string) => void) | null = null;
  ws.onmessage = (ev) => {
    let m: {
      type?: string;
      is_final?: boolean;
      from_finalize?: boolean;
      channel?: { alternatives?: { transcript?: string }[] };
    };
    try {
      m = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    if (m.type !== 'Results') return;
    const t = m.channel?.alternatives?.[0]?.transcript ?? '';
    if (m.is_final) {
      committed = join(committed, t);
      o.cb.onText(committed, '');
      if (m.from_finalize && finishing) finishing(committed);
    } else o.cb.onText(committed, t);
  };
  ws.onclose = (ev) => {
    if (finishing) finishing(committed);
    else if (ev.code !== 1000)
      base.fail(`closed ${ev.code ?? ''} ${ev.reason ?? ''}`.trim());
  };
  return {
    engine: 'deepgram',
    push: (pcm) => base.send(pcm),
    finish: () =>
      new Promise<string>((resolve) => {
        finishing = (t) => {
          finishing = null;
          resolve(t);
          base.send(JSON.stringify({ type: 'CloseStream' }));
        };
        base.send(JSON.stringify({ type: 'Finalize' }));
      }),
    close: () => ws.close(1000),
  };
}

/** ElevenLabs Scribe v2 Realtime: partial_transcript / committed_transcript; commit flushes. */
export function elevenLabsStream(o: {
  ws: WsFactory;
  apiKey: string;
  model?: string;
  cb: CloudCallbacks;
}): CloudStream {
  const q = new URLSearchParams({
    model_id: o.model || 'scribe_v2_realtime',
    audio_format: 'pcm_16000',
    commit_strategy: 'manual',
  });
  const ws = o.ws(`wss://api.elevenlabs.io/v1/speech-to-text/realtime?${q}`, {
    headers: { 'xi-api-key': o.apiKey },
  });
  const base = socketBase(ws, o.cb);
  let committed = '';
  let finishing: ((t: string) => void) | null = null;
  ws.onmessage = (ev) => {
    let m: {
      message_type?: string;
      text?: string;
      error?: string;
      message?: string;
    };
    try {
      m = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    const type = m.message_type ?? '';
    if (type === 'partial_transcript') o.cb.onText(committed, m.text ?? '');
    else if (type.startsWith('committed_transcript')) {
      committed = join(committed, m.text ?? '');
      o.cb.onText(committed, '');
      finishing?.(committed);
    } else if (/error/i.test(type)) base.fail(m.error ?? m.message ?? type);
  };
  ws.onclose = (ev) => {
    if (finishing) finishing(committed);
    else if (ev.code !== 1000)
      base.fail(`closed ${ev.code ?? ''} ${ev.reason ?? ''}`.trim());
  };
  const chunk = (pcm: Uint8Array, commit: boolean) =>
    JSON.stringify({
      message_type: 'input_audio_chunk',
      audio_base_64: b64(pcm),
      commit,
      sample_rate: 16000,
    });
  return {
    engine: 'elevenlabs',
    push: (pcm) => base.send(chunk(pcm, false)),
    finish: () =>
      new Promise<string>((resolve) => {
        finishing = (t) => {
          finishing = null;
          resolve(t);
        };
        base.send(chunk(new Uint8Array(0), true));
      }),
    close: () => ws.close(1000),
  };
}

export interface ArbiterResult {
  text: string;
  engine: TalkEngineId;
  failover: boolean;
  reason?: string;
}

/**
 * Picks which engine's text to show and insert. Cloud wins while healthy; it fails over to the on-device stream on a
 * socket error, when it stays silent while on-device already has words (stallMs), or when its final is late (finalMs).
 */
export class EngineArbiter {
  private cloudText = { committed: '', partial: '' };
  private local = { finals: [] as string[], partial: '' };
  private cloudFailed: string | null = null;
  private cloudHeard = false;
  private localSince: number | null = null;
  constructor(
    private o: {
      cloud: CloudStream | null;
      stallMs: number;
      finalMs: number;
      now?: () => number;
      onChange?: () => void;
    },
  ) {}

  private get now() {
    return this.o.now ?? Date.now;
  }

  /** The socket connects while the mic is already streaming on-device; attach it once it exists. */
  attachCloud(cloud: CloudStream) {
    this.o = { ...this.o, cloud };
    this.o.onChange?.();
  }

  cloudUpdate(committed: string, partial: string) {
    this.cloudHeard = true;
    this.cloudText = { committed, partial };
    this.o.onChange?.();
  }
  cloudError(err: string) {
    if (!this.cloudFailed) this.cloudFailed = err;
    this.o.onChange?.();
  }
  localPartial(text: string) {
    this.local.partial = text;
    this.localSince ??= this.now();
    this.check();
    this.o.onChange?.();
  }
  localFinal(text: string) {
    this.local.finals.push(text);
    this.local.partial = '';
    this.localSince ??= this.now();
    this.check();
    this.o.onChange?.();
  }

  /** Call periodically: detects a silent cloud stream. */
  check() {
    if (
      this.o.cloud &&
      !this.cloudFailed &&
      !this.cloudHeard &&
      this.localSince !== null &&
      this.now() - this.localSince >= this.o.stallMs
    ) {
      this.cloudFailed = 'cloud stream stalled';
      this.o.onChange?.();
    }
  }

  get usingCloud() {
    return !!this.o.cloud && !this.cloudFailed;
  }
  get failover() {
    return !!this.o.cloud && !!this.cloudFailed;
  }
  get failReason() {
    return this.cloudFailed ?? undefined;
  }
  get engine(): TalkEngineId {
    return this.usingCloud ? this.o.cloud!.engine : 'on-device';
  }
  private localText() {
    return {
      committed: this.local.finals.join(' ').trim(),
      partial: this.local.partial,
    };
  }
  view() {
    return this.usingCloud ? this.cloudText : this.localText();
  }

  /** After the helper's dictation.stopped (localFinalText = full on-device text). */
  async finish(localFinalText: string): Promise<ArbiterResult> {
    const local =
      localFinalText.trim() ||
      join(this.localText().committed, this.localText().partial);
    if (this.usingCloud) {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const r = await Promise.race([
        this.o.cloud!.finish().then(
          (t) => ({ t }),
          (e: unknown) => ({ err: String(e) }),
        ),
        new Promise<{ err: string }>((res) => {
          timer = setTimeout(
            () => res({ err: 'cloud final timed out' }),
            this.o.finalMs,
          );
        }),
      ]);
      clearTimeout(timer);
      this.o.cloud!.close();
      if ('t' in r && !this.cloudFailed) {
        const text =
          r.t.trim() || join(this.cloudText.committed, this.cloudText.partial);
        // An empty cloud result while on-device heard words is a failure, not silence.
        if (text || !local)
          return { text, engine: this.o.cloud!.engine, failover: false };
        this.cloudFailed = 'cloud returned no text';
      } else this.cloudFailed ??= 'err' in r ? r.err : 'cloud failed';
    } else this.o.cloud?.close();
    return {
      text: local,
      engine: 'on-device',
      failover: !!this.o.cloud,
      ...(this.cloudFailed ? { reason: this.cloudFailed } : {}),
    };
  }
}
