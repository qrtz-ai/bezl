import { describe, expect, it } from 'vitest';
import { createRouter, ipcContract, type IpcChannel } from '@qrtz/core';
import { manifests, resolveManifest } from './manifests';

// IPC integration: real manifests × real contract × router capability enforcement.
const router = createRouter({
  resolveSubApp: resolveManifest,
  handlers: Object.fromEntries(
    Object.keys(ipcContract).map((c) => [c, () => undefined]),
  ) as never,
  validateResponses: false,
});
const call = (subApp: string, channel: IpcChannel, payload?: unknown) =>
  router.dispatch({
    v: 1,
    id: 'x',
    channel,
    version: ipcContract[channel].version,
    subApp,
    payload,
  });

describe('sub-app capability matrix', () => {
  it.each([
    ['memo', 'memo:list', { query: 'x' }, true],
    ['memo', 'memo:start', {}, true],
    ['memo', 'permissions:request', { permission: 'microphone' }, true],
    ['memo', 'secrets:set', { name: 'llm.openrouter', value: 'k' }, false],
    ['memo', 'config:set', { patch: {} }, false],
    ['home', 'memo:list', { query: '' }, false],
    ['home', 'memo:start', {}, false],
    ['settings', 'secrets:set', { name: 'llm.openrouter', value: 'k' }, true],
    ['settings', 'memo:delete', { id: 'm' }, false],
  ] as const)(
    '%s → %s allowed=%s',
    async (subApp, channel, payload, allowed) => {
      const r = await call(subApp, channel, payload);
      if (allowed) expect(r.ok).toBe(true);
      else expect(r.ok === false && r.error.code).toBe('E_FORBIDDEN');
    },
  );
  it('every channel is reachable by at least one sub app (no dead capabilities)', () => {
    for (const [c, def] of Object.entries(ipcContract)) {
      expect(
        manifests.some((m) => m.capabilities.includes(def.capability)),
        c,
      ).toBe(true);
    }
  });
  it('validates memo payloads', async () => {
    const r = await call('memo', 'memo:get', { id: 'x'.repeat(100) });
    expect(r.ok === false && r.error.code).toBe('E_VALIDATION');
  });
});
