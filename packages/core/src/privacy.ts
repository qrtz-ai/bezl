import type { Config } from './config';

/** True when a detected meeting's app/platform matches an excluded entry (case-insensitive). */
export function isExcludedApp(
  cfg: Config,
  m: { app: string; platform: string },
): boolean {
  const app = m.app.toLowerCase();
  const platform = m.platform.toLowerCase();
  return cfg.privacy.excludedApps.some((raw) => {
    const x = raw.trim().toLowerCase();
    return !!x && (app === x || app.includes(x) || platform === x);
  });
}
