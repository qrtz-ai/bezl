import { z } from 'zod';

/** App configuration (non-secret). Every field has a default so old files keep loading. */
export const ConfigSchema = z.object({
  schemaVersion: z.literal(1).default(1),
  theme: z.enum(['dark', 'system']).default('dark'),
  logLevel: z.enum(['debug', 'info', 'warn', 'error']).default('info'),
  telemetry: z
    .object({ crashReports: z.boolean().default(false) })
    .prefault({}),
  memo: z
    .object({
      autoDetectMeetings: z.boolean().default(true),
      liveAsr: z
        .enum(['parakeet-eou-160ms', 'parakeet-unified-320ms', 'apple-speech'])
        .default('parakeet-unified-320ms'),
      llmProvider: z
        .enum(['openai', 'anthropic', 'google', 'openrouter', 'ollama', 'none'])
        .default('openrouter'),
      llmModel: z.string().default(''),
    })
    .prefault({}),
});
export type Config = z.infer<typeof ConfigSchema>;

export const defaultConfig = (): Config => ConfigSchema.parse({});

/** Parse untrusted stored config; falls back to defaults field-by-field rather than failing. */
export function loadConfig(raw: unknown): Config {
  const r = ConfigSchema.safeParse(raw ?? {});
  if (r.success) return r.data;
  // Drop invalid top-level keys and retry so one bad value doesn't reset everything.
  const obj =
    typeof raw === 'object' && raw
      ? { ...(raw as Record<string, unknown>) }
      : {};
  for (const issue of r.error.issues) {
    const k = issue.path[0];
    if (typeof k === 'string') delete obj[k];
  }
  return ConfigSchema.safeParse(obj).data ?? defaultConfig();
}

export function deepMerge<T>(base: T, patch: unknown): T {
  if (typeof patch !== 'object' || patch === null || Array.isArray(patch))
    return patch as T;
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(patch)) {
    if (k === '__proto__' || k === 'constructor' || k === 'prototype') continue;
    out[k] =
      typeof v === 'object' && v !== null && !Array.isArray(v)
        ? deepMerge(out[k] ?? {}, v)
        : v;
  }
  return out as T;
}

/** Apply a patch and validate strictly (writes must be valid). */
export function applyConfigPatch(cfg: Config, patch: unknown): Config {
  return ConfigSchema.parse(deepMerge(cfg, patch));
}
