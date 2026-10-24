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
    ['note', 'note:list', { query: 'x' }, true],
    ['note', 'note:start', {}, true],
    ['note', 'permissions:request', { permission: 'microphone' }, true],
    ['dash', 'config:set', { patch: {} }, false],
    ['unknown', 'note:list', { query: '' }, false],
    ['settings', 'config:set', { patch: {} }, true],
    ['settings', 'note:delete', { id: 'm' }, false],
    // ADR 0161: the package manager is the marketplace's, not settings'.
    ['marketplace', 'packages:list', undefined, true],
    ['marketplace', 'packages:plan', { op: 'install', names: ['echo'] }, true],
    [
      'marketplace',
      'packages:set-enabled',
      { name: 'ctrl', enabled: false },
      true,
    ],
    ['marketplace', 'config:get', undefined, true],
    ['marketplace', 'config:set', { patch: {} }, false],
    ['settings', 'packages:plan', { op: 'install', names: ['echo'] }, false],
    ['talk', 'packages:set-enabled', { name: 'ctrl', enabled: false }, false],
    // Audit P1: "check for updates" lives in settings → system; it installs nothing.
    ['settings', 'packages:check-update', undefined, true],
    ['talk', 'packages:check-update', undefined, false],
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
  it('validates note payloads', async () => {
    const r = await call('note', 'note:get', { id: 'x'.repeat(100) });
    expect(r.ok === false && r.error.code).toBe('E_VALIDATION');
  });
});
