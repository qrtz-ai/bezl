// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { defaultConfig } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { ThemeProvider } from '@qrtz/ui';
import SettingsView from './SettingsView';

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

describe('global settings (ADR 0146 M1.4)', () => {
  it('keep only the global sections: no module switches and no module settings (ADR 0161)', async () => {
    const client = {
      subApp: 'settings',
      invoke: async (channel: string) =>
        channel === 'config:get'
          ? defaultConfig()
          : (ipcSamples as Record<string, { response: unknown }>)[channel]
              ?.response,
      on: () => () => {},
    } as unknown as QrtzClient;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const root = createRoot(el);
    await act(async () =>
      root.render(
        <ThemeProvider mode="light">
          <QrtzClientProvider client={client}>
            <SettingsView />
          </QrtzClientProvider>
        </ThemeProvider>,
      ),
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const q = (id: string) => el.querySelector(`[data-testid="${id}"]`);
    for (const id of [
      'open-marketplace',
      'speech-settings',
      'offline-note',
      'theme',
      'menu-bar-icon',
      'launch-at-login',
      'backup-card',
      'logs-export',
    ])
      expect(q(id), id).not.toBeNull();
    // echo's, nano's and ctrl's own settings live in their views now.
    // The module list and its switches moved to the marketplace's module manager.
    for (const id of [
      'modules-settings',
      'modules-all',
      'module-echo',
      'module-nano',
      'module-ctrl',
      'talk-enabled',
      'talk-show-pill',
      'talk-push-key',
      'auto-record',
      'auto-detect',
      'auto-summarize',
      'default-template',
      'excluded-apps',
      'pause-hotkey',
      'speaker-mode',
    ])
      expect(q(id), id).toBeNull();
    expect(q('modules-note')?.textContent).toContain('behind the gear');
    root.unmount();
  });

  it('export diagnostics and check for updates only when pressed', async () => {
    const calls: [string, unknown][] = [];
    const client = {
      subApp: 'settings',
      invoke: async (channel: string, req?: unknown) => {
        calls.push([channel, req]);
        if (channel === 'config:get') return defaultConfig();
        if (channel === 'logs:export')
          return { savedTo: '/tmp/d.txt', files: 2, bytes: 9, crashReports: 3 };
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
            <SettingsView />
          </QrtzClientProvider>
        </ThemeProvider>,
      ),
    );
    const q = (id: string) =>
      el.querySelector(`[data-testid="${id}"]`) as HTMLElement | null;
    expect(calls.some(([c]) => c === 'packages:check-update')).toBe(false);
    await act(async () => {
      q('diagnostics-export')?.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(calls).toContainEqual(['logs:export', { diagnostics: true }]);
    expect(q('logs-result')?.textContent).toBe(
      'saved 2 log files and 3 crash reports',
    );
    await act(async () => {
      q('update-check')?.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(calls.filter(([c]) => c === 'packages:check-update')).toHaveLength(
      1,
    );
    expect(q('update-result')?.textContent).toBe("updates aren't set up yet");
    root.unmount();
  });
});
