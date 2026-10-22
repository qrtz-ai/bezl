import { z } from 'zod';
import { MODULE_IDS } from './modules';
import { PillMotionSchema } from './pill-motion';

/**
 * App configuration (non-secret). Every field has a default so old files keep loading, and removed fields are
 * stripped on load. qrtz is entirely offline (ADR 0076): the cloud settings (`note.llmProvider`, `note.llmModel`,
 * `talk.engine`, `talk.autoFailover`, `speech.cloud`, `privacy.localOnly`) are gone; an old file that has them
 * still loads, without them.
 */
/**
 * One module's switch. `installed` (ADR 0169) is written only by the host, from qrtz.lock: qrtz ships empty and a
 * module is installed from the marketplace. A renderer's `config:set` of it is refused. It defaults to true so a
 * config the host never synced (the mock backend, tests) keeps every module.
 */
const ModuleSwitch = z
  .object({
    enabled: z.boolean().default(true),
    installed: z.boolean().default(true),
    /**
     * A bundled module (echo, ADR 0174) runs the copy inside qrtz.app because its store version couldn't start
     * (written only by the host, like `installed`). Echo's view says so. Stub: no host writes it yet (the
     * self-update work will); absent means no fallback.
     */
    bundledFallback: z.boolean().optional(),
  })
  .prefault({});

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
    })
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
      /** Show the floating always-on-top mini HUD while recording. */
      floatingHud: z.boolean().default(true),
      /** Delete recordings (audio only; transcript and summary stay) this many days after the meeting. 0 = keep. */
      retainAudioDays: z.number().int().min(0).max(3650).default(0),
      /** Summary template for new meetings (per-meeting override in meeting detail). */
      defaultTemplate: z
        .enum(['general', 'one-on-one', 'standup', 'sales', 'interview'])
        .default('general'),
      /**
       * ADR 0217: Apple Calendar on this Mac (read-only, offline). Off until the user turns it on in nano's settings
       * or presses 'connect calendar' (that asks macOS for access). `calendars`: the chosen calendar ids, null = all.
       */
      calendar: z
        .object({
          enabled: z.boolean().default(false),
          calendars: z
            .array(z.string().min(1).max(512))
            .max(200)
            .nullable()
            .default(null),
        })
        .prefault({}),
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
      /** With fn as the talk key, stop macOS from also running its fn/globe action (input source, emoji). */
      blockGlobeKey: z.boolean().default(true),
      /** Press once to start, again to stop (Electron accelerator; '' = none). */
      toggleHotkey: z.string().max(60).default('CommandOrControl+Shift+Space'),
      /** Keep the mic open between dictations so the first word isn't clipped (mic indicator stays on). */
      warmMic: z.boolean().default(false),
      /** Audio kept from just before the key press (warm mic only). */
      preRollMs: z.number().int().min(0).max(2000).default(300),
      /** Clean up dictated text on the device (fillers, punctuation, the dictionary). There is no LLM cleanup:
       * the text goes in as soon as the key is released (an old `aiCleanup` / `cleanupModel` /
       * `cleanupTimeoutMs` is dropped on load). */
      autoCleanup: z.boolean().default(true),
      /** Type the text into the focused app when you let go. */
      autoInsert: z.boolean().default(true),
      /** Put your clipboard back after a paste-based insert. */
      restoreClipboard: z.boolean().default(true),
      /** Floating pill with the level and live text while dictating. */
      showPill: z.boolean().default(true),
      /** ADR 0097: while dictating with the speakers playing (not headphones), hold the mic down when it only hears
       * the speakers. Uses a short system-audio tap under the screen & system audio grant (macOS may show its
       * recording indicator). */
      echoGate: z.boolean().default(true),
      saveHistory: z.boolean().default(true),
      /** ADR 0221, temporary for tuning: the pill's springs, fades and corners (echo's dev opts). */
      pillMotion: PillMotionSchema,
    })
    .prefault({}),
  /** qrtz speech (ADR 0046). Model-agnostic: the engine and its models live inside the speech module. */
  speech: z
    .object({
      /** Speaker labelling trade-off: fast (about 1 s behind) or accurate (about 3 s behind). */
      speakerMode: z.enum(['fast', 'accurate']).default('fast'),
      /** Download and install signed speech updates in the background (never during a session). */
      autoUpdate: z.boolean().default(true),
      /** Mac: right after the speech models download, load each once so the first recording starts with speaker
       * labels instead of waiting for the system to prepare the models (ADR 0079). */
      warmAfterDownload: z.boolean().default(true),
    })
    .prefault({}),
  /**
   * Global module switches (ADR 0146): the global settings control every module; each module's own settings live
   * with the module. Off stops the module and hides its view. Everything defaults to on, so M0 changes nothing.
   */
  modules: z
    .object({
      /** Every module at once. */
      enabled: z.boolean().default(true),
      echo: ModuleSwitch,
      nano: ModuleSwitch,
      ctrl: ModuleSwitch,
      snap: ModuleSwitch,
    })
    .prefault({}),
  /**
   * ADR 0184: the first-run permissions screen (microphone, system audio, accessibility). `done` is written only by
   * the host, from `onboarding:finish` (continue or skip); a renderer's `config:set` of it is refused, like
   * `modules.<id>.installed`. Absent (a fresh install, or a config from before 0184) shows the screen once.
   */
  onboarding: z.object({ done: z.boolean().default(false) }).prefault({}),
  privacy: z
    .object({
      /** Meetings detected in these apps/platforms (e.g. "Safari", "zoom", "google-meet") are ignored. */
      excludedApps: z
        .array(z.string().trim().min(1).max(80))
        .max(50)
        .default([]),
      /** Global shortcut that pauses/resumes capture while recording (Electron accelerator; "" = none). */
      pauseHotkey: z.string().max(60).default('CommandOrControl+Shift+P'),
    })
    .prefault({}),
  /**
   * qrtz snap (ADR 0159), stored in snap's own settings file. `shortcuts` holds only the user's changes to
   * Spectacle's defaults (action → "ctrl+alt+right"; "" clears one); `disabledApps` are bundle ids snap leaves alone;
   * `statusItem` shows snap's menu bar item with every action; `summon` is "summon agent" (off by default).
   */
  snap: z
    .object({
      shortcuts: z.record(z.string().max(32), z.string().max(60)).default({}),
      disabledApps: z
        .array(z.string().trim().min(1).max(255))
        .max(100)
        .default([]),
      statusItem: z.boolean().default(true),
      /** Summon agent: hold left ⌘ + right ⌘ to bring this app forward (again: back to where you were). */
      summon: z
        .object({
          enabled: z.boolean().default(false),
          bundleId: z.string().trim().max(255).default(''),
          name: z.string().max(255).default(''),
        })
        .prefault({}),
    })
    .prefault({}),
});
export type Config = z.infer<typeof ConfigSchema>;

export const defaultConfig = (): Config => ConfigSchema.parse({});

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * Legacy settings (crates/qrtz-app `migrate_legacy` does the same). The Note sub app was called Memo, so stored
 * files have `memo: {...}`; it moves to `note` (keys already under `note` win). Removed settings (an old
 * `talk.engine`, Deepgram's included) need nothing: the schema strips them. Returns the input unchanged when
 * nothing needs migrating.
 */
export function migrateLegacyConfig(raw: unknown): {
  raw: unknown;
  migrated: boolean;
} {
  if (!isObj(raw)) return { raw, migrated: false };
  let out: Record<string, unknown> = raw;
  let migrated = false;
  if ('memo' in out) {
    const { memo, ...rest } = out;
    const note = {
      ...(isObj(memo) ? memo : {}),
      ...(isObj(rest.note) ? rest.note : {}),
    };
    out = { ...rest, note };
    migrated = true;
  }
  return { raw: migrated ? out : raw, migrated };
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

/**
 * True if a renderer's `config:set` patch touches a key only the host writes (crates/qrtz-app `installed.rs`
 * `patch_touches_installed` / `patch_touches_bundled_fallback` and `onboarding.rs` `patch_touches_onboarding`):
 * `modules.<id>.installed` (ADR 0169), `modules.<id>.bundledFallback` (ADR 0181) and `onboarding` (ADR 0184). The
 * mock backend refuses these the same way the host does.
 *
 * P3.5 (91cdf32): replacing `modules`, or a module's own entry, with something that isn't an object
 * (`{modules: null}`, `{modules: {echo: null}}`) counts as touching them too, since `deepMerge` would apply it as a
 * replacement and wipe the host-written keys below it. `modules.enabled` (the global switch) is a legitimate scalar.
 */
export function patchTouchesHostOnly(patch: unknown): boolean {
  if (!isPlainObject(patch)) return false;
  if ('onboarding' in patch) return true;
  if (!('modules' in patch)) return false;
  const m = patch.modules;
  if (!isPlainObject(m)) return true;
  return Object.entries(m).some(([id, v]) =>
    isPlainObject(v)
      ? 'installed' in v || 'bundledFallback' in v
      : (MODULE_IDS as readonly string[]).includes(id),
  );
}

/** A `config:set` patch is an object: anything else would replace the whole config (`deepMerge`; host `patch_is_object`). */
export function patchIsObject(patch: unknown): boolean {
  return isPlainObject(patch);
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
