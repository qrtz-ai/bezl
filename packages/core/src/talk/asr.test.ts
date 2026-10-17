import { describe, expect, it, vi } from 'vitest';
import { EngineArbiter, elevenLabsStream, type WsLike } from './asr';

function fakeWs() {
  const sent: (string | Uint8Array)[] = [];
  let url = '';
  let init: { protocols?: string[]; headers?: Record<string, string> } = {};
  const ws: WsLike & { open(): void; msg(o: unknown): void } = {
    readyState: 0,
    onopen: null,
    onmessage: null,
    onerror: null,
    onclose: null,
    send: (d) => void sent.push(d),
    close: () => {},
    open() {
      (ws as { readyState: number }).readyState = 1;
      ws.onopen?.({});
    },
    msg: (o) => ws.onmessage?.({ data: JSON.stringify(o) }),
  };
  return {
    ws,
    sent,
    factory: (u: string, i: typeof init) => ((url = u), (init = i), ws),
    get url() {
      return url;
    },
    get init() {
      return init;
    },
  };
}

describe('cloud streaming engines', () => {
  it('elevenlabs: header auth, base64 chunks, partial/committed, commit flush, errors', async () => {
    const f = fakeWs();
    const onError = vi.fn();
    const texts: string[] = [];
    const s = elevenLabsStream({
      ws: f.factory,
      apiKey: 'xi',
      cb: { onText: (c, p) => texts.push(`${c}|${p}`), onError },
    });
    expect(f.init.headers).toEqual({ 'xi-api-key': 'xi' });
    expect(f.url).toContain('model_id=scribe_v2_realtime');
    f.ws.open();
    s.push(new Uint8Array([0, 1]));
    expect(JSON.parse(f.sent[0] as string)).toMatchObject({
      message_type: 'input_audio_chunk',
      audio_base_64: 'AAE=',
      commit: false,
    });
    f.ws.msg({ message_type: 'partial_transcript', text: 'hel' });
    const done = s.finish();
    expect(JSON.parse(f.sent.at(-1) as string)).toMatchObject({ commit: true });
    f.ws.msg({ message_type: 'committed_transcript', text: 'Hello.' });
    expect(await done).toBe('Hello.');
    expect(texts).toEqual(['|hel', 'Hello.|']);
    f.ws.msg({ message_type: 'auth_error', error: 'bad key' });
    expect(onError).toHaveBeenCalledWith('bad key');
  });
});

describe('EngineArbiter (cloud primary, on-device fallback)', () => {
  const cloud = (final: Promise<string>) => ({
    engine: 'elevenlabs' as const,
    push: () => {},
    finish: () => final,
    close: vi.fn(),
  });

  it('uses cloud text while healthy', async () => {
    const a = new EngineArbiter({
      cloud: cloud(Promise.resolve('cloud words')),
      stallMs: 1000,
      finalMs: 1000,
    });
    a.localPartial('local words');
    a.cloudUpdate('', 'cloud');
    expect(a.view()).toEqual({ committed: '', partial: 'cloud' });
    expect(await a.finish('local words')).toEqual({
      text: 'cloud words',
      engine: 'elevenlabs',
      failover: false,
    });
  });

  it('fails over on socket error, stall, late final, or empty cloud text', async () => {
    const e = new EngineArbiter({
      cloud: cloud(Promise.resolve('x')),
      stallMs: 1000,
      finalMs: 1000,
    });
    e.cloudError('boom');
    expect(e.engine).toBe('on-device');
    expect(await e.finish('local')).toMatchObject({
      text: 'local',
      engine: 'on-device',
      failover: true,
      reason: 'boom',
    });

    let now = 0;
    const s = new EngineArbiter({
      cloud: cloud(Promise.resolve('x')),
      stallMs: 500,
      finalMs: 1000,
      now: () => now,
    });
    s.localPartial('hi');
    now = 600;
    s.check();
    expect(s.failReason).toBe('cloud stream stalled');

    const late = new EngineArbiter({
      cloud: cloud(new Promise(() => {})),
      stallMs: 1e9,
      finalMs: 20,
    });
    late.cloudUpdate('a', '');
    expect(await late.finish('local text')).toMatchObject({
      engine: 'on-device',
      reason: 'cloud final timed out',
    });

    const empty = new EngineArbiter({
      cloud: cloud(Promise.resolve('')),
      stallMs: 1e9,
      finalMs: 100,
    });
    expect(await empty.finish('heard this')).toMatchObject({
      text: 'heard this',
      reason: 'cloud returned no text',
    });
  });

  it('without a cloud engine it is on-device and joins finals + partial', async () => {
    const a = new EngineArbiter({ cloud: null, stallMs: 1, finalMs: 1 });
    a.localFinal('hello');
    a.localPartial('wor');
    expect(a.view()).toEqual({ committed: 'hello', partial: 'wor' });
    expect(await a.finish('')).toEqual({
      text: 'hello wor',
      engine: 'on-device',
      failover: false,
    });
  });
});
