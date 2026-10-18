import { describe, expect, it } from 'vitest';
import { createClient } from './client';
import {
  TAURI_EVENT,
  TAURI_INVOKE_CMD,
  TAURI_STREAM_CLOSE_CMD,
  TAURI_STREAM_JOIN_CMD,
  TAURI_STREAM_OPEN_CMD,
  TAURI_SUBSCRIBE_CMD,
  createTauriTransport,
  type TauriBridge,
} from './tauri-transport';

const flush = () => new Promise((r) => setTimeout(r, 0));

function fakeBridge(
  opts: {
    subscribeOk?: boolean;
    fail?: boolean;
    /** Sub apps the backend lets onto a stream (default: all). */
    streamAllow?: (subApp: string) => boolean;
  } = {},
) {
  const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
  let emit: (payload: unknown) => void = () => {};
  let listens = 0;
  // Open streams as the backend holds them: id → channel callback.
  const streams = new Map<number, (p: unknown) => void>();
  let nextId = 1;
  const allow = opts.streamAllow ?? (() => true);
  const bridge: TauriBridge = {
    channel: (onmessage) => ({ onmessage }),
    async invoke(cmd, args) {
      calls.push({ cmd, args });
      if (opts.fail) throw new Error('backend gone');
      if (cmd === TAURI_SUBSCRIBE_CMD) return { ok: opts.subscribeOk ?? true };
      if (cmd === TAURI_STREAM_OPEN_CMD) {
        if (!allow(String(args!.subApp))) return { ok: false };
        const id = nextId++;
        streams.set(
          id,
          (args!.channel as { onmessage: (p: unknown) => void }).onmessage,
        );
        return { ok: true, id };
      }
      if (cmd === TAURI_STREAM_JOIN_CMD)
        return {
          ok: streams.has(Number(args!.id)) && allow(String(args!.subApp)),
        };
      if (cmd === TAURI_STREAM_CLOSE_CMD) {
        streams.delete(Number(args!.id));
        return { ok: true };
      }
      const env = args!.envelope as { id: string; payload: unknown };
      return { ok: true, id: env.id, data: { echoed: env.payload } };
    },
    async listen(event, cb) {
      expect(event).toBe(TAURI_EVENT);
      listens++;
      emit = (payload) => cb({ payload });
      return () => {};
    },
  };
  return {
    bridge,
    calls,
    emit: (p: unknown) => emit(p),
    listens: () => listens,
    /** The backend sends on every open stream (one window here). */
    send: (p: unknown) => {
      for (const cb of streams.values()) cb(p);
    },
    streams: () => streams.size,
  };
}

describe('tauri transport', () => {
  it('sends the envelope to qrtz_invoke and unwraps through createClient', async () => {
    const f = fakeBridge();
    const client = createClient(createTauriTransport(f.bridge), 'note');
    const res = await client.invoke('note:list', { query: 'x' });
    expect(res).toEqual({ echoed: { query: 'x' } });
    const env = f.calls[0]!.args!.envelope as Record<string, unknown>;
    expect(f.calls[0]!.cmd).toBe(TAURI_INVOKE_CMD);
    expect(env).toMatchObject({
      v: 1,
      channel: 'note:list',
      version: 1,
      subApp: 'note',
    });
  });

  it('a rejected tauri invoke becomes a retryable E_UNAVAILABLE AppError', async () => {
    const f = fakeBridge({ fail: true });
    const client = createClient(createTauriTransport(f.bridge), 'note');
    await expect(client.invoke('note:session')).rejects.toMatchObject({
      code: 'E_UNAVAILABLE',
      retryable: true,
    });
  });

  it('delivers events only to subscribed sub apps named by the backend, with one listener', async () => {
    const f = fakeBridge();
    const t = createTauriTransport(f.bridge);
    const got: string[] = [];
    t.on('note', 'note:pipeline', (p) => got.push(`note:${String(p)}`));
    const off = t.on('shell', 'note:pipeline', (p) =>
      got.push(`shell:${String(p)}`),
    );
    await flush();
    expect(f.listens()).toBe(1);
    expect(f.calls.filter((c) => c.cmd === TAURI_SUBSCRIBE_CMD)).toHaveLength(
      2,
    );
    f.emit({ event: 'note:pipeline', payload: 'a', subApps: ['note'] });
    f.emit({
      event: 'note:pipeline',
      payload: 'b',
      subApps: ['note', 'shell'],
    });
    f.emit({ event: 'note:changed', payload: 'c', subApps: ['note'] });
    f.emit('garbage');
    off();
    f.emit({
      event: 'note:pipeline',
      payload: 'd',
      subApps: ['note', 'shell'],
    });
    expect(got).toEqual(['note:a', 'note:b', 'shell:b', 'note:d']);
  });

  it('drops the listener when the backend refuses the subscription', async () => {
    const f = fakeBridge({ subscribeOk: false });
    const t = createTauriTransport(f.bridge);
    const got: unknown[] = [];
    t.on('gallery', 'note:pipeline', (p) => got.push(p));
    await flush();
    f.emit({ event: 'note:pipeline', payload: 1, subApps: ['gallery'] });
    expect(got).toEqual([]);
  });

  it('isolates a throwing listener', async () => {
    const f = fakeBridge();
    const t = createTauriTransport(f.bridge);
    const got: unknown[] = [];
    t.on('note', 'note:changed', () => {
      throw new Error('boom');
    });
    t.on('note', 'note:changed', (p) => got.push(p));
    await flush();
    f.emit({ event: 'note:changed', payload: { id: 'm1' }, subApps: ['note'] });
    expect(got).toEqual([{ id: 'm1' }]);
  });
});

describe('tauri transport streams (ADR 0095)', () => {
  it('high-rate events use one channel per window per event, never the broadcast event or qrtz_subscribe', async () => {
    const f = fakeBridge();
    const t = createTauriTransport(f.bridge);
    const got: string[] = [];
    const offNote = t.on('note', 'note:session', (p) =>
      got.push(`note:${String(p)}`),
    );
    const offShell = t.on('shell', 'note:session', (p) =>
      got.push(`shell:${String(p)}`),
    );
    await flush();
    await flush();
    const cmds = f.calls.map((c) => c.cmd);
    expect(cmds.filter((c) => c === TAURI_STREAM_OPEN_CMD)).toHaveLength(1);
    expect(cmds.filter((c) => c === TAURI_STREAM_JOIN_CMD)).toHaveLength(1);
    expect(cmds).not.toContain(TAURI_SUBSCRIBE_CMD);
    expect(f.listens()).toBe(0);
    expect(f.streams()).toBe(1);
    f.send('a');
    offNote();
    f.send('b');
    expect(got).toEqual(['note:a', 'shell:a', 'shell:b']);
    // The window's last listener leaves: the stream closes.
    offShell();
    await flush();
    expect(f.streams()).toBe(0);
    f.send('c');
    expect(got).toHaveLength(3);
  });

  it('a sub app the backend refuses never hears the stream, and does not block one that is allowed', async () => {
    const f = fakeBridge({ streamAllow: (s) => s !== 'gallery' });
    const t = createTauriTransport(f.bridge);
    const got: string[] = [];
    t.on('gallery', 'note:transcript', (p) => got.push(`gallery:${String(p)}`));
    t.on('note', 'note:transcript', (p) => got.push(`note:${String(p)}`));
    await flush();
    await flush();
    await flush();
    expect(f.streams()).toBe(1);
    f.send('x');
    expect(got).toEqual(['note:x']);
    // A refused join after the stream exists: still nothing for it.
    t.on('gallery', 'note:transcript', (p) =>
      got.push(`gallery2:${String(p)}`),
    );
    await flush();
    await flush();
    f.send('y');
    expect(got).toEqual(['note:x', 'note:y']);
  });
});
