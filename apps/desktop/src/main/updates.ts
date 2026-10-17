import type { Logger } from '@qrtz/core';

export interface UpdatePolicy {
  isPackaged: boolean;
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
}

/**
 * Auto-update scaffold (update-electron-app → update.electronjs.org, GitHub releases).
 * Squirrel.Mac refuses unsigned builds, so this stays off until a Developer ID-signed,
 * notarized release exists. Opt in with QRTZ_AUTO_UPDATE=1 once signing is in place.
 */
export function shouldEnableUpdates(p: UpdatePolicy): {
  enabled: boolean;
  reason: string;
} {
  if (!p.isPackaged) return { enabled: false, reason: 'dev build' };
  if (p.platform !== 'darwin' && p.platform !== 'win32')
    return { enabled: false, reason: `unsupported platform ${p.platform}` };
  if (p.env.QRTZ_AUTO_UPDATE !== '1')
    return {
      enabled: false,
      reason: 'disabled until signed releases (set QRTZ_AUTO_UPDATE=1)',
    };
  return { enabled: true, reason: 'enabled' };
}

export async function initAutoUpdates(p: UpdatePolicy, log: Logger) {
  const d = shouldEnableUpdates(p);
  log.info('auto-update', d);
  if (!d.enabled) return d;
  try {
    const { updateElectronApp, UpdateSourceType } =
      await import('update-electron-app');
    updateElectronApp({
      updateSource: {
        type: UpdateSourceType.ElectronPublicUpdateService,
        repo: 'qrtz-ai/qrtz',
      },
      updateInterval: '1 hour',
      notifyUser: true,
    });
  } catch (e) {
    log.warn('auto-update init failed', { err: String(e) });
    return { enabled: false, reason: 'init failed' };
  }
  return d;
}
