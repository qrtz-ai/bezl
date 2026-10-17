import { describe, expect, it } from 'vitest';
import { createClient } from './client';
import {
  TAURI_EVENT,
  TAURI_INVOKE_CMD,
  TAURI_SUBSCRIBE_CMD,
  createTauriTransport,
  type TauriBridge,
} from './tauri-transport';

const flush = () => new Promise((r) => setTimeout(r, 0));

function fakeBridge(opts: { subscribeOk?: boolean; fail?: boolean } = {}) {
  const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
  let emit: (payload: unknown) => void = () => {};
  let listens = 0;
  const bridge: TauriBridge = {
    async invoke(cmd, args) {
      calls.push({ cmd, args });
      if (opts.fail) throw new Error('backend gone');
      if (cmd === TAURI_SUBSCRIBE_CMD) return { ok: opts.subscribeOk ?? true };
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
    t.on('note', 'note:session', (p) => got.push(`note:${String(p)}`));
    const off = t.on('shell', 'note:session', (p) =>
      got.push(`shell:${String(p)}`),
    );
    await flush();
    expect(f.listens()).toBe(1);
    expect(f.calls.filter((c) => c.cmd === TAURI_SUBSCRIBE_CMD)).toHaveLength(
      2,
    );
    f.emit({ event: 'note:session', payload: 'a', subApps: ['note'] });
    f.emit({ event: 'note:session', payload: 'b', subApps: ['note', 'shell'] });
    f.emit({ event: 'note:pipeline', payload: 'c', subApps: ['note'] });
    f.emit('garbage');
    off();
    f.emit({ event: 'note:session', payload: 'd', subApps: ['note', 'shell'] });
    expect(got).toEqual(['note:a', 'note:b', 'shell:b', 'note:d']);
  });

  it('drops the listener when the backend refuses the subscription', async () => {
    const f = fakeBridge({ subscribeOk: false });
    const t = createTauriTransport(f.bridge);
    const got: unknown[] = [];
    t.on('gallery', 'note:session', (p) => got.push(p));
    await flush();
    f.emit({ event: 'note:session', payload: 1, subApps: ['gallery'] });
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
