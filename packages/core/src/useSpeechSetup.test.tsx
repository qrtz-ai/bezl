// @vitest-environment jsdom
// QRT-277: a scoped useSpeechSetup reads only its modules' models and fails closed.
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient, SpeechSetupStatus } from '@qrtz/core';
import { QrtzClientProvider, useSpeechSetup } from '@qrtz/core/react';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const none: SpeechSetupStatus = {
  configured: true,
  installed: null,
  bytes: 0,
  latest: '1.0.0',
  latestBytes: 1,
  waiting: null,
  listExpired: false,
  download: { state: 'idle', done: 0, total: 0 },
};
const installed: SpeechSetupStatus = { ...none, installed: '1.0.0', bytes: 1 };

type Answer = (req: { extensions?: string[] }) => Promise<SpeechSetupStatus>;

async function mount(answer: Answer, extensions: readonly string[]) {
  const calls: unknown[] = [];
  const listeners = new Set<(p: unknown) => void>();
  const client = {
    subApp: 'test',
    invoke: async (channel: string, req: { extensions?: string[] }) => {
      calls.push(req);
      if (channel !== 'speech:status') throw new Error(channel);
      return answer(req);
    },
    on: (_e: string, fn: (p: unknown) => void) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  } as unknown as QrtzClient;
  let out: ReturnType<typeof useSpeechSetup> | null = null;
  function Probe() {
    out = useSpeechSetup({
      extensions: extensions as ('echo' | 'nano' | 'ctrl')[],
    });
    return null;
  }
  const root = createRoot(document.createElement('div'));
  const settle = () =>
    act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
  await act(async () =>
    root.render(
      <QrtzClientProvider client={client}>
        <Probe />
      </QrtzClientProvider>,
    ),
  );
  await settle();
  const emit = async (p: unknown) => {
    await act(async () => listeners.forEach((fn) => fn(p)));
    await settle();
  };
  return { calls, get: () => out!, emit, unmount: () => root.unmount() };
}

describe('useSpeechSetup scoped to a module (QRT-277)', () => {
  it("echo's model missing while nano's are installed: echo is not ready", async () => {
    const v = await mount(
      async (r) =>
        r.extensions?.length === 1 && r.extensions[0] === 'nano'
          ? installed
          : r.extensions
            ? none
            : installed,
      ['echo'],
    );
    expect(v.calls).toEqual([{ extensions: ['echo'] }]);
    expect(v.get().speech?.installed).toBeNull();
    // A global "installed" event doesn't make echo ready.
    await v.emit(installed);
    expect(v.get().speech?.installed).toBeNull();
    v.unmount();
  });

  it.each([[[]], [['']], [['bogus']], [['echo', '']]])(
    'an empty or unknown scope %j is never sent and shows not ready',
    async (scope) => {
      const v = await mount(async () => installed, scope);
      expect(v.calls).toEqual([]);
      expect(v.get().speech?.installed).toBeNull();
      expect(v.get().speech?.configured).toBe(true);
      expect(v.get().error).not.toBeNull();
      v.unmount();
    },
  );

  it("a failed re-read clears a stale 'installed'", async () => {
    let fail = false;
    const v = await mount(async () => {
      if (fail) throw new Error('host gone');
      return installed;
    }, ['echo']);
    expect(v.get().speech?.installed).toBe('1.0.0');
    fail = true;
    await v.emit(none);
    expect(v.get().speech?.installed).toBeNull();
    expect(v.get().error).toBe('host gone');
    v.unmount();
  });
});
