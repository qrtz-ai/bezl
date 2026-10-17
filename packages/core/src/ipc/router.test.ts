import { describe, expect, it } from 'vitest';
import { createRouter } from './router';
import { createClient, type QrtzTransport } from './client';
import { EventBus } from './events';
import { SubAppManifest } from '../subapp';
import { AppError } from '../errors';

const apps = {
  shell: SubAppManifest.parse({
    id: 'shell',
    name: 's',
    icon: 's',
    capabilities: ['app.info'],
  }),
  plain: SubAppManifest.parse({ id: 'plain', name: 'p', icon: 'p' }),
};
const info = {
  name: 'qrtz',
  version: '1',
  platform: 'darwin',
  arch: 'arm64',
  electron: '44',
};
const router = createRouter({
  resolveSubApp: (id) => apps[id as keyof typeof apps],
  handlers: { 'app:get-info': () => info },
});
const base = {
  v: 1,
  id: 'r1',
  channel: 'app:get-info',
  version: 1,
  subApp: 'shell',
  payload: undefined,
};

describe('IPC router', () => {
  it('dispatches valid calls', async () => {
    expect(await router.dispatch(base)).toEqual({
      ok: true,
      id: 'r1',
      data: info,
    });
  });
  it.each([
    [{ ...base, v: 2 }, 'E_VALIDATION'],
    ['garbage', 'E_VALIDATION'],
    [{ ...base, channel: 'nope' }, 'E_UNKNOWN_CHANNEL'],
    [{ ...base, version: 9 }, 'E_VERSION'],
    [{ ...base, subApp: 'ghost' }, 'E_FORBIDDEN'],
    [{ ...base, subApp: 'plain' }, 'E_FORBIDDEN'],
    [{ ...base, channel: 'db:health', subApp: 'shell' }, 'E_FORBIDDEN'],
  ])('rejects %j with %s', async (env, code) => {
    const r = await router.dispatch(env);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe(code);
  });
  it('validates request payloads', async () => {
    const r2 = createRouter({
      resolveSubApp: () =>
        SubAppManifest.parse({
          id: 'x',
          name: 'x',
          icon: 'x',
          capabilities: ['config.write'],
        }),
      handlers: { 'config:set': () => ({}) },
    });
    const r = await r2.dispatch({
      ...base,
      channel: 'config:set',
      subApp: 'x',
      payload: { patch: 5 },
    });
    expect(r.ok === false && r.error.code).toBe('E_VALIDATION');
  });
  it('rejects invalid handler output and hides internals', async () => {
    const bad = createRouter({
      resolveSubApp: () => apps.shell,
      handlers: { 'app:get-info': () => ({ nope: 1 }) as never },
    });
    const r = await bad.dispatch(base);
    expect(r.ok === false && r.error.code).toBe('E_INTERNAL');
  });
  it('typed client round-trips and throws AppError', async () => {
    const transport: QrtzTransport = {
      invoke: (e) => router.dispatch(e),
      on: () => () => {},
    };
    expect(
      await createClient(transport, 'shell').invoke('app:get-info'),
    ).toEqual(info);
    await expect(
      createClient(transport, 'plain').invoke('app:get-info'),
    ).rejects.toBeInstanceOf(AppError);
  });
});

describe('EventBus', () => {
  it('validates payloads and isolates listener failures', () => {
    const bus = new EventBus();
    const got: unknown[] = [];
    bus.on('helper:status', () => {
      throw new Error('boom');
    });
    const off = bus.on('helper:status', (p) => got.push(p.state));
    bus.emit('helper:status', { state: 'running', restarts: 0 });
    off();
    bus.emit('helper:status', { state: 'stopped', restarts: 0 });
    expect(got).toEqual(['running']);
    expect(() =>
      bus.emit('helper:status', { state: 'weird' } as never),
    ).toThrow();
  });
});
