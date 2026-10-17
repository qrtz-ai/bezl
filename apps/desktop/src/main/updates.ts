import type { Logger } from '@qrtz/core';

export interface UpdatePolicy {
  isPackaged: boolean;
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  arch?: string;
  /** Values baked in at build time by scripts/release-mac.sh (vite define); runtime env can't redirect a packaged app. */
  build?: { feedUrl?: string; autoUpdate?: boolean };
}

export type UpdateSource =
  | { kind: 'static'; baseUrl: string }
  | { kind: 'public-service'; repo: string };

/**
 * Feed choice. A self-hosted static feed (R2/S3 bucket or a GitHub Releases "latest/download" URL holding
 * RELEASES.json + the zip) wins when QRTZ_UPDATE_FEED_URL was set at build. `{platform}` and `{arch}`
 * placeholders are substituted, e.g. https://updates.example.com/qrtz/{platform}/{arch}. Without it,
 * update.electronjs.org (public repos only). Non-HTTPS feeds are rejected.
 */
export function resolveUpdateSource(
  p: Pick<UpdatePolicy, 'platform' | 'arch' | 'build'>,
): UpdateSource | { kind: 'invalid'; reason: string } {
  const raw = p.build?.feedUrl?.trim();
  if (!raw) return { kind: 'public-service', repo: 'qrtz-ai/qrtz' };
  const url = raw
    .replaceAll('{platform}', p.platform)
    .replaceAll('{arch}', p.arch ?? process.arch)
    .replace(/\/+$/, '');
  try {
    if (new URL(url).protocol !== 'https:')
      return { kind: 'invalid', reason: 'update feed must be https' };
  } catch {
    return { kind: 'invalid', reason: 'update feed is not a URL' };
  }
  return { kind: 'static', baseUrl: url };
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
  if (p.env.QRTZ_AUTO_UPDATE !== '1' && !p.build?.autoUpdate)
    return {
      enabled: false,
      reason: 'disabled until signed releases (set QRTZ_AUTO_UPDATE=1)',
    };
  const src = resolveUpdateSource(p);
  if (src.kind === 'invalid') return { enabled: false, reason: src.reason };
  return { enabled: true, reason: 'enabled' };
}

export async function initAutoUpdates(p: UpdatePolicy, log: Logger) {
  const d = shouldEnableUpdates(p);
  log.info('auto-update', d);
  if (!d.enabled) return d;
  const src = resolveUpdateSource(p);
  if (src.kind === 'invalid') return { enabled: false, reason: src.reason };
  log.info('auto-update source', src);
  try {
    const { updateElectronApp, UpdateSourceType } =
      await import('update-electron-app');
    updateElectronApp({
      updateSource:
        src.kind === 'static'
          ? { type: UpdateSourceType.StaticStorage, baseUrl: src.baseUrl }
          : {
              type: UpdateSourceType.ElectronPublicUpdateService,
              repo: src.repo,
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
