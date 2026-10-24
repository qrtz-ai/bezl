// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import { AppModels } from './AppModels';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

/** Renders echo's models, presses delete and confirms, with `speech:delete` failing with `raw`. */
async function deleteFailing(raw: string) {
  const client = {
    subApp: 'marketplace',
    invoke: async (channel: string) => {
      if (channel === 'speech:delete') throw new Error(raw);
      return (ipcSamples as Record<string, { response: unknown }>)[channel]
        ?.response;
    },
    on: () => () => {},
  } as unknown as QrtzClient;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const root = createRoot(el);
  await act(async () =>
    root.render(
      <ThemeProvider mode="light">
        <QrtzClientProvider client={client}>
          <AppModels app="echo" on={['echo']} />
        </QrtzClientProvider>
      </ThemeProvider>,
    ),
  );
  await act(async () => {
    await new Promise((r) => setTimeout(r, 10));
  });
  const q = (id: string) =>
    document.querySelector(`[data-testid="${id}"]`) as HTMLElement | null;
  await act(async () => {
    q('models-echo-panel-delete')?.click();
  });
  await act(async () => {
    q('models-echo-confirm-delete')?.click();
    await new Promise((r) => setTimeout(r, 10));
  });
  const box = q('models-echo-panel-error');
  const shown = box?.cloneNode(true) as Element | undefined;
  shown?.querySelectorAll('details').forEach((d) => d.remove());
  const result = {
    shown: shown?.textContent ?? null,
    details: q('models-echo-panel-error-details')?.textContent ?? null,
  };
  act(() => root.unmount());
  el.remove();
  return result;
}

describe("an app's models: delete errors (QRT-311)", () => {
  it('the shared-bundle refusal reads as plain language, with no details', async () => {
    const r = await deleteFailing(
      'SPEECH_SHARED: the speech module is shared (remote source)',
    );
    expect(r.shown).toContain("couldn't delete the models");
    expect(r.shown).toContain(
      "these models are part of bezl's shared speech, so they can't be deleted for one app.",
    );
    expect(r.shown).not.toContain('SPEECH_SHARED');
    expect(r.details).toBeNull();
  });

  it('any other failure keeps its raw reason under details only', async () => {
    const raw = 'permission denied (os error 13): /Users/x/models/parakeet';
    const r = await deleteFailing(raw);
    expect(r.shown).toContain("couldn't delete the models");
    expect(r.shown).toContain('try again in a moment.');
    expect(r.shown).not.toMatch(/os error|\/Users/);
    expect(r.details).toContain(raw);
  });
});
