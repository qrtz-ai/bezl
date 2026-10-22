// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { QrtzClient } from '@qrtz/core';
import { defaultConfig } from '@qrtz/core';
import { QrtzClientProvider } from '@qrtz/core/react';
import { ipcSamples } from '@qrtz/core/ipc/samples';
import { primaryButtons, ThemeProvider } from '@qrtz/ui';
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
      'speech-auto-update',
      'speech-warm-after-download',
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
      // The qrtz page is the app manager now: no apps section and no link to it.
      'extensions-link',
      'open-marketplace',
      'modules-note',
      // Each app's models are managed in its detail on the qrtz page: no speech section here.
      'speech-settings',
      'speech-setup',
      'speech-download',
      'speech-delete',
      'speech-pause',
    ])
      expect(q(id), id).toBeNull();
    expect(el.textContent).not.toContain('apps page');
    // The opts page: its own sidebar row, so a plain "opts" title and no close button.
    expect(q('settings')?.querySelector('h1')?.textContent).toBe('qrtz');
    expect(q('settings-close')).toBeNull();
    root.unmount();
    el.remove();
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
    expect(primaryButtons(el)).toBeLessThanOrEqual(1);
    expect(calls.some(([c]) => c === 'packages:check-update')).toBe(false);
    await act(async () => {
      q('diagnostics-export')?.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(calls).toContainEqual(['logs:export', { diagnostics: true }]);
    expect(q('logs-result')?.textContent).toBe(
      'saved 2 log files and 3 crash reports',
    );
    // ADR 0181: "check for updates" is qrtz's own updater now (settings → qrtz).
    expect(q('about-qrtz')).not.toBeNull();
    expect(calls.some(([c]) => c === 'app-update:check')).toBe(false);
    await act(async () => {
      q('update-check')?.click();
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(calls.filter(([c]) => c === 'app-update:check')).toHaveLength(1);
    expect(calls.some(([c]) => c === 'packages:check-update')).toBe(false);
    root.unmount();
  });
});

describe('permissions note (ADR 0184 amendment)', () => {
  it('names system audio only while nano is installed, live on config:changed', async () => {
    const withNano = (installed: boolean) => {
      const c = defaultConfig();
      return {
        ...c,
        modules: { ...c.modules, nano: { ...c.modules.nano, installed } },
      };
    };
    const listeners: ((p: unknown) => void)[] = [];
    const client = {
      subApp: 'settings',
      invoke: async (channel: string) =>
        channel === 'config:get'
          ? withNano(false)
          : (ipcSamples as Record<string, { response: unknown }>)[channel]
              ?.response,
      on: (event: string, cb: (p: unknown) => void) => {
        if (event === 'config:changed') listeners.push(cb);
        return () => {};
      },
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
    const note = () =>
      el.querySelector('[data-testid="permissions-note"]')?.textContent ?? '';
    expect(note()).toContain('microphone and accessibility');
    expect(note()).not.toContain('system audio');
    await act(async () => {
      for (const cb of listeners) cb(withNano(true));
    });
    expect(note()).toContain('system audio');
    root.unmount();
    el.remove();
  });
});

describe('open-source licenses (audit P0-4)', () => {
  it('shows the notices the build carries, fetched only when asked', async () => {
    const calls: string[] = [];
    const client = {
      subApp: 'settings',
      invoke: async (channel: string) => {
        calls.push(channel);
        if (channel === 'config:get') return defaultConfig();
        if (channel === 'app:licenses')
          return {
            text: 'Licensed by NVIDIA Corporation under the NVIDIA Open Model License',
          };
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
      document.body.querySelector(`[data-testid="${id}"]`);
    expect(q('licenses-text')).toBeNull();
    expect(calls).not.toContain('app:licenses');
    await act(async () => (q('licenses-open') as HTMLElement).click());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(q('licenses-text')?.textContent).toContain(
      'NVIDIA Open Model License',
    );
    root.unmount();
    el.remove();
  });

  it('a combined load error names what failed; the raw reasons sit only under details (QRT-311)', async () => {
    const raws: Record<string, string> = {
      'app:get-info': 'no such file or directory: /Users/x/qrtz/info.json',
      'helper:status': 'connection refused (os error 61)',
    };
    const client = {
      subApp: 'settings',
      invoke: async (channel: string) => {
        if (raws[channel]) throw new Error(raws[channel]);
        if (channel === 'config:get') return defaultConfig();
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
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    const box = el.querySelector('[data-testid="settings-error"]')!;
    expect(box).not.toBeNull();
    const shown = box.cloneNode(true) as Element;
    shown.querySelectorAll('details').forEach((d) => d.remove());
    expect(shown.textContent).toContain("couldn't load app info, helper");
    expect(shown.textContent).toContain('retry in a moment');
    for (const raw of Object.values(raws))
      expect(shown.textContent).not.toContain(raw);
    expect(shown.textContent).not.toMatch(/\/Users|os error/);
    const details = el.querySelector(
      '[data-testid="settings-error-details"]',
    )!.textContent;
    expect(details).toContain(`app info: ${raws['app:get-info']}`);
    expect(details).toContain(`helper: ${raws['helper:status']}`);
    root.unmount();
    el.remove();
  });
});
