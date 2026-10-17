import type { Config } from './config';

/** Cloud LLM providers; Ollama runs on this machine and stays allowed in local-only mode. */
const LOCAL_PROVIDERS = new Set(['none', 'ollama']);

/** The provider to actually use: local-only forces the on-device path for anything cloud-hosted. */
export function effectiveLlmProvider(
  cfg: Config,
): Config['memo']['llmProvider'] {
  const p = cfg.memo.llmProvider;
  return cfg.privacy.localOnly && !LOCAL_PROVIDERS.has(p) ? 'none' : p;
}

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
