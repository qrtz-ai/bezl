import type { QrtzTransport } from '@qrtz/core';

type Perms = Record<string, string>;

/** A fake host for the onboarding tests: config, permissions and the calls it saw. */
export function fakeHost(opts: { done?: boolean; perms?: Perms } = {}) {
  const state = {
    done: opts.done ?? false,
    perms: {
      microphone: 'not-determined',
      'screen-recording': 'not-determined',
      accessibility: 'denied',
      ...opts.perms,
    } as Perms,
    calls: [] as { channel: string; payload: unknown }[],
    listeners: new Map<string, Set<(p: unknown) => void>>(),
    /** What the next permissions:request grants (default: the asked one). */
    grantOnRequest: true,
  };
  const data = (channel: string, payload: unknown): unknown => {
    switch (channel) {
      case 'config:get':
        return { onboarding: { done: state.done } };
      case 'permissions:get':
        return { ...state.perms };
      case 'permissions:request': {
        const p = (payload as { permission: string }).permission;
        if (state.grantOnRequest) state.perms[p] = 'granted';
        return { ...state.perms };
      }
      case 'permissions:open-settings':
        return {
          opened: true,
          restart: (payload as { pane: string }).pane === 'screen-recording',
        };
      case 'app:restart':
        return { restarting: true };
      case 'onboarding:finish':
        state.done = true;
        return { done: true };
      default:
        throw new Error(`unexpected ${channel}`);
    }
  };
  const transport: QrtzTransport = {
    invoke: async (e) => {
      state.calls.push({ channel: e.channel, payload: e.payload });
      try {
        return { ok: true, id: e.id, data: data(e.channel, e.payload) };
      } catch (err) {
        return {
          ok: false,
          id: e.id,
          error: {
            name: 'AppError',
            code: 'E_INTERNAL',
            message: (err as Error).message,
            retryable: false,
          },
        };
      }
    },
    on: (_sub, event, cb) => {
      let s = state.listeners.get(event);
      if (!s) state.listeners.set(event, (s = new Set()));
      s.add(cb);
      return () => s.delete(cb);
    },
  };
  const emit = (event: string, payload: unknown) => {
    for (const cb of state.listeners.get(event) ?? []) cb(payload);
  };
  const seen = (ch: string) => state.calls.filter((c) => c.channel === ch);
  return { state, transport, emit, seen };
}
