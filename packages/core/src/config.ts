import { z } from 'zod';

/** App configuration (non-secret). Every field has a default so old files keep loading. */
export const ConfigSchema = z.object({
  schemaVersion: z.literal(1).default(1),
  theme: z.enum(['dark', 'light', 'system']).default('dark'),
  logLevel: z.enum(['debug', 'info', 'warn', 'error']).default('info'),
  app: z
    .object({
      /** Show the qrtz item in the macOS menu bar. */
      menuBarIcon: z.boolean().default(true),
      /** Open qrtz when you log in (macOS/Windows login item). */
      launchAtLogin: z.boolean().default(false),
      /** Check for updates in the background (packaged builds). */
      autoUpdateCheck: z.boolean().default(true),
    })
    .prefault({}),
  telemetry: z
    .object({ crashReports: z.boolean().default(false) })
    .prefault({}),
  note: z
    .object({
      autoDetectMeetings: z.boolean().default(true),
      /** Start recording as soon as a meeting is detected (no prompt). Off = show the "Record this meeting?" prompt. */
      autoRecord: z.boolean().default(false),
      /** Show the "Record this meeting?" prompt and notification when a meeting is detected. */
      detectPrompt: z.boolean().default(true),
      /** OS notification when every capture source goes silent (session `noAudio`) while the window is unfocused. */
      notifyNoAudio: z.boolean().default(true),
      /** Summarize automatically when a meeting ends (off = Summarize button in the meeting). */
      autoSummarize: z.boolean().default(true),
      /** Build semantic-search embeddings in the background after meetings. */
      backgroundIndexing: z.boolean().default(true),
      /** Download the on-device search model when it is missing (off = full-text search only). */
      searchModelDownload: z.boolean().default(true),
      llmProvider: z
        .enum(['openai', 'anthropic', 'google', 'openrouter', 'ollama', 'none'])
        .default('openrouter'),
      llmModel: z.string().default(''),
      /** Show the floating always-on-top mini HUD while recording. */
      floatingHud: z.boolean().default(true),
      /** Delete recordings (audio only; transcript and summary stay) this many days after the meeting. 0 = keep. */
      retainAudioDays: z.number().int().min(0).max(3650).default(0),
      /** Summary template for new meetings (per-meeting override in meeting detail). */
      defaultTemplate: z
        .enum(['general', 'one-on-one', 'standup', 'sales', 'interview'])
        .default('general'),
    })
    .prefault({}),
  talk: z
    .object({
      /** Global dictation (push-to-talk + toggle hotkeys, floating pill). */
      enabled: z.boolean().default(true),
      /** Hold-to-talk modifier key watched by the native helper ('' = none). */
      pushKey: z
        .enum(['fn', 'right-option', 'right-command', 'right-control', ''])
        .default('fn'),
      /** Press once to start, again to stop (Electron accelerator; '' = none). */
      toggleHotkey: z.string().max(60).default('CommandOrControl+Shift+Space'),
      /** Keep the mic open between dictations so the first word isn't clipped (mic indicator stays on). */
      warmMic: z.boolean().default(false),
      /** Audio kept from just before the key press (warm mic only). */
      preRollMs: z.number().int().min(0).max(2000).default(300),
      /** auto: best cloud engine with a key, else on-device. */
      engine: z
        .enum(['auto', 'deepgram', 'elevenlabs', 'on-device'])
        .default('auto'),
      /** Switch to the on-device stream automatically when the cloud stream fails. */
      autoFailover: z.boolean().default(true),
      /** Clean up dictated text (fillers, punctuation, formatting). */
      autoCleanup: z.boolean().default(true),
      /** Use the LLM for cleanup (OpenRouter); off = on-device rules only. */
      aiCleanup: z.boolean().default(true),
      cleanupModel: z.string().max(120).default(''),
      cleanupTimeoutMs: z.number().int().min(100).max(10_000).default(900),
      /** Type the text into the focused app when you let go. */
      autoInsert: z.boolean().default(true),
      /** Put your clipboard back after a paste-based insert. */
      restoreClipboard: z.boolean().default(true),
      /** Floating pill with the level and live text while dictating. */
      showPill: z.boolean().default(true),
      saveHistory: z.boolean().default(true),
    })
    .prefault({}),
  /** qrtz speech (ADR 0046). Model-agnostic: the engine and its models live inside the speech module. */
  speech: z
    .object({
      /** Speaker labelling trade-off: fast (about 1 s behind) or accurate (about 3 s behind). */
      speakerMode: z.enum(['fast', 'accurate']).default('fast'),
      /** Optional cloud words (host-side ElevenLabs adapter); speakers stay on-device. */
      cloud: z.enum(['off', 'elevenlabs']).default('off'),
      /** Download and install signed speech updates in the background (never during a session). */
      autoUpdate: z.boolean().default(true),
    })
    .prefault({}),
  /** First-run setup (welcome, permissions, speech download, optional OpenRouter key). */
  onboarding: z.object({ done: z.boolean().default(false) }).prefault({}),
  privacy: z
    .object({
      /** Never send audio, transcripts or notes to a cloud service: summaries/chat use the on-device fallback (or a local Ollama). */
      localOnly: z.boolean().default(false),
      /** Meetings detected in these apps/platforms (e.g. "Safari", "zoom", "google-meet") are ignored. */
      excludedApps: z
        .array(z.string().trim().min(1).max(80))
        .max(50)
        .default([]),
      /** Global shortcut that pauses/resumes capture while recording (Electron accelerator; "" = none). */
      pauseHotkey: z.string().max(60).default('CommandOrControl+Shift+P'),
    })
    .prefault({}),
});
export type Config = z.infer<typeof ConfigSchema>;

export const defaultConfig = (): Config => ConfigSchema.parse({});

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * Renamed settings keys. The Note sub app was called Memo, so stored files have `memo: {...}`; it moves
 * to `note` (keys already under `note` win). Returns the input unchanged when nothing needs migrating.
 */
export function migrateLegacyConfig(raw: unknown): {
  raw: unknown;
  migrated: boolean;
} {
  if (!isObj(raw) || !('memo' in raw)) return { raw, migrated: false };
  const { memo, ...rest } = raw;
  const note = {
    ...(isObj(memo) ? memo : {}),
    ...(isObj(rest.note) ? rest.note : {}),
  };
  return { raw: { ...rest, note }, migrated: true };
}

/** Parse untrusted stored config; falls back to defaults field-by-field rather than failing. */
export function loadConfig(input: unknown): Config {
  const { raw } = migrateLegacyConfig(input);
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
