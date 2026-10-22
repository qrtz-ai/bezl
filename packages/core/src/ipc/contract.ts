import { z } from 'zod';
import type { Capability } from '../capabilities';
import { AppErrorJSON } from '../errors';
import { AppPermission, SettingsPane } from '../capabilities';
import {
  MeetingDetail,
  MeetingListItem,
  PermissionsStatus,
  SessionView,
} from '../note/contract';
import { MeetingSummary } from '../note/summary';
import { LiveTranscript, PipelineView, StepId } from '../note/lifecycle';
import { MeetingNote } from '../note/notes';
import { SummaryTemplateId } from '../note/templates';
import { DictionaryEntry, TalkEntry, TalkView } from '../talk/contract';
import { SpeechSetupStatus } from '../speech/setup';
import {
  ModuleEnabled,
  PackageOp,
  PackagePlan,
  PackagesApplied,
  PackagesPruned,
  PackagesState,
} from '../packages/contract';
import {
  CTRL_TEXT_MAX,
  CtrlRunStatus,
  CtrlRunSummary,
  CtrlRunView,
  CtrlModelState,
} from '../ctrl/contract';
import { SnapStatus } from '../snap/contract';
import { AppUpdateStatus } from '../app-update/contract';
import { DashDelete, DashView } from '../dash/contract';

/** Bump when the envelope shape changes (not when a channel changes). */
export const PROTOCOL_VERSION = 1;

export interface ChannelDef<
  Req extends z.ZodType = z.ZodType,
  Res extends z.ZodType = z.ZodType,
> {
  /** Per-channel version; caller and handler must agree. */
  version: number;
  capability: Capability;
  request: Req;
  response: Res;
}

export const defineChannel = <Req extends z.ZodType, Res extends z.ZodType>(
  def: ChannelDef<Req, Res>,
) => def;

export const AppInfo = z.object({
  name: z.string(),
  version: z.string(),
  platform: z.string(),
  arch: z.string(),
  electron: z.string(),
});

/** ADR 0083: renderer log lines. Never transcript, dictated or summary text, notes or titles: errors only. */
export const LOG_MESSAGE_MAX = 1000;
export const RendererLogLevel = z.enum(['debug', 'info', 'warn', 'error']);
export const LogWrite = z.object({
  level: RendererLogLevel,
  scope: z
    .string()
    .min(1)
    .max(64)
    .regex(/^[a-z0-9._:-]+$/),
  message: z.string().max(LOG_MESSAGE_MAX),
});
export type LogWrite = z.infer<typeof LogWrite>;

export const HelperStatus = z.object({
  state: z.enum(['stopped', 'starting', 'running', 'backoff', 'failed']),
  restarts: z.number().int(),
  lastError: z.string().optional(),
});
export type HelperStatus = z.infer<typeof HelperStatus>;

/** ADR 0175: the speech calls a module view makes name exactly its own models (`extensions: [<id>]`). */
const SpeechScope = z
  .object({
    extensions: z
      .array(z.enum(['echo', 'nano', 'ctrl']))
      .max(8)
      .optional(),
  })
  .optional();

/** The single source of truth for request/response IPC. */
/** ADR 0217: one cached Apple Calendar event (only these fields are kept). */
export const CalendarEvent = z.object({
  id: z.string(),
  calendarId: z.string(),
  title: z.string(),
  startAt: z.number(),
  endAt: z.number(),
  allDay: z.boolean(),
  attendeeCount: z.number().int().min(0),
  meetingUrl: z.string().optional(),
});
export type CalendarEvent = z.infer<typeof CalendarEvent>;
export const CalendarStatus = z.object({
  enabled: z.boolean(),
  access: z.enum([
    'not-determined',
    'granted',
    'denied',
    'restricted',
    'write-only',
    'unsupported',
  ]),
  calendars: z.array(z.object({ id: z.string(), title: z.string() })),
  chosen: z.array(z.string()).nullable(),
  lastSyncAt: z.number().nullable(),
});
export type CalendarStatus = z.infer<typeof CalendarStatus>;
/** ADR 0217 (H1): what an os.permissions holder (the shell's onboarding) may see: the access state only. */
export const CalendarAccess = CalendarStatus.pick({ access: true }).strict();
export type CalendarAccess = z.infer<typeof CalendarAccess>;

/** ADR 0220: the notes vault. */
export const VaultInfo = z.object({
  path: z.string(),
  isDefault: z.boolean(),
  count: z.number().int().nonnegative(),
});
export const DocSummary = z.object({
  id: z.string(),
  title: z.string(),
  path: z.string(),
  updatedAt: z.number(),
  meetings: z.array(z.string()),
});
export type DocSummary = z.infer<typeof DocSummary>;
export const Doc = DocSummary.extend({
  body: z.string(),
  hash: z.string(),
  /** Not UTF-8 text, or too large: qrtz shows it and never changes it. */
  readOnly: z.boolean(),
});
export type Doc = z.infer<typeof Doc>;

export const ipcContract = {
  'app:get-info': defineChannel({
    version: 1,
    capability: 'app.info',
    request: z.void(),
    response: AppInfo,
  }),
  /** The third-party notices (THIRD_PARTY_NOTICES.md, built into the app) for settings → "open-source licenses". */
  'app:licenses': defineChannel({
    version: 1,
    capability: 'app.info',
    request: z.void(),
    response: z.object({ text: z.string() }),
  }),
  'config:get': defineChannel({
    version: 1,
    capability: 'config.read',
    request: z.void(),
    response: z.record(z.string(), z.unknown()),
  }),
  'config:set': defineChannel({
    version: 1,
    capability: 'config.write',
    request: z.object({ patch: z.record(z.string(), z.unknown()) }),
    response: z.record(z.string(), z.unknown()),
  }),
  'db:health': defineChannel({
    version: 1,
    capability: 'db.read',
    request: z.void(),
    response: z.object({
      ok: z.boolean(),
      schemaVersion: z.number().int(),
      fts5: z.boolean(),
    }),
  }),
  'helper:status': defineChannel({
    version: 1,
    capability: 'helper.status',
    request: z.void(),
    response: HelperStatus,
  }),
  'permissions:get': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.void(),
    response: PermissionsStatus,
  }),
  'permissions:request': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.object({ permission: AppPermission }),
    response: PermissionsStatus,
  }),
  /** A permission prompt's button: System Settings at that pane. `restart`: the grant needs qrtz relaunched. */
  'permissions:open-settings': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.object({ pane: SettingsPane }),
    response: z.object({ opened: z.boolean(), restart: z.boolean() }),
  }),
  /** "restart qrtz" for a grant a running qrtz only sees after a relaunch (screen recording). */
  'app:restart': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.void(),
    response: z.object({ restarting: z.boolean() }),
  }),
  /**
   * ADR 0184: the permissions onboarding is done (continue or skip). The host writes `onboarding.done`; a renderer's
   * `config:set` of `onboarding` is refused.
   */
  'onboarding:finish': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.void(),
    response: z.object({ done: z.literal(true) }),
  }),
  'note:session': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.void(),
    response: SessionView,
  }),
  'note:start': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({ title: z.string().max(200).optional() }).optional(),
    response: SessionView,
  }),
  'note:stop': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.void(),
    response: SessionView,
  }),
  /** Post-processing state of one meeting (finalize → summary → index), or null if it never had one. */
  'note:pipeline': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({ id: z.string().max(64) }),
    response: PipelineView.nullable(),
  }),
  /** Manual retry of one post-processing step (re-runs it and everything after it). */
  'note:retry-step': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64), step: StepId }),
    response: PipelineView,
  }),
  /** Snapshot of the live transcript of the meeting being recorded; apply `note:transcript` deltas on top. */
  'note:live-transcript': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({ meetingId: z.string().max(64).optional() }).optional(),
    response: LiveTranscript.nullable(),
  }),
  'note:dismiss': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.void(),
    response: SessionView,
  }),
  /** The recorded meeting ended (note:auto-stop armed): keep recording, cancel the auto-stop countdown. */
  'note:keep-recording': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.void(),
    response: SessionView,
  }),
  'note:list': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({
      query: z.string().max(200).optional(),
      /** Page size (default 100). Up to limit+1 rows come back; the extra one signals more. */
      limit: z.number().int().min(1).max(1000).optional(),
      offset: z.number().int().min(0).optional(),
    }),
    response: z.array(MeetingListItem),
  }),
  'note:get': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({ id: z.string().max(64) }),
    response: MeetingDetail,
  }),
  'note:export': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({
      id: z.string().max(64),
      save: z.boolean().default(false),
    }),
    response: z.object({
      filename: z.string(),
      markdown: z.string(),
      savedTo: z.string().optional(),
    }),
  }),
  'note:resummarize': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64) }),
    response: MeetingSummary,
  }),
  /** ADR 0146 M1.4: the calling module's own settings (its view's settings page); see MODULE_SETTINGS_PATHS. */
  'module:get-settings': defineChannel({
    version: 1,
    capability: 'module.settings',
    request: z.void(),
    response: z.record(z.string(), z.unknown()),
  }),
  /** Refused (E_FORBIDDEN) if the patch reaches outside the caller's module settings. Answers the new settings. */
  'module:set-settings': defineChannel({
    version: 1,
    capability: 'module.settings',
    request: z.object({ patch: z.record(z.string(), z.unknown()) }),
    response: z.record(z.string(), z.unknown()),
  }),
  /** Scoped config write so Note's prompt can enable auto-record without the broad config.write capability. */
  'note:set-auto-record': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({ enabled: z.boolean() }),
    response: z.object({ autoRecord: z.boolean() }),
  }),
  'note:rename-speaker': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({
      meetingId: z.string().max(64),
      from: z.string().min(1).max(120),
      to: z.string().trim().min(1).max(120),
    }),
    response: z.object({ changed: z.number() }),
  }),
  'note:delete': defineChannel({
    version: 1,
    capability: 'note.write',
    /** deferMs: undoable delete. A tombstone is written now and the delete runs after deferMs (or on next launch). */
    request: z.object({
      id: z.string().max(64),
      deferMs: z.number().int().min(0).max(60_000).optional(),
    }),
    response: z.object({ ok: z.literal(true) }),
  }),
  'note:undo-delete': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ restored: z.boolean() }),
  }),
  /** Live notes. Without meetingId the note goes to the meeting being recorded, stamped at the current offset. */
  'note:add-note': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({
      meetingId: z.string().max(64).optional(),
      text: z.string().trim().min(1).max(4000),
      atMs: z.number().nonnegative().optional(),
    }),
    response: MeetingNote.extend({ meetingId: z.string() }),
  }),
  'note:update-note': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({
      id: z.string().max(64),
      text: z.string().trim().min(1).max(4000),
    }),
    response: z.object({ ok: z.boolean() }),
  }),
  'note:delete-note': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ ok: z.boolean() }),
  }),
  'note:list-notes': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({ meetingId: z.string().max(64).optional() }),
    response: z.array(MeetingNote),
  }),
  /** Store the meeting's template and re-run its summary with it. */
  'note:set-template': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64), template: SummaryTemplateId }),
    response: MeetingSummary,
  }),
  /** One line into the local log (ADR 0083), rate-limited in the backend; `accepted: false` = dropped. */
  'log:write': defineChannel({
    version: 1,
    capability: 'log.write',
    request: LogWrite,
    response: z.object({ accepted: z.boolean() }),
  }),
  /**
   * Save every local log file as one text file where the user picks, or (`reveal`) show the log folder.
   * `diagnostics`: the same file plus macOS's crash reports for qrtz and its helpers (local only; the user sends it).
   */
  'logs:export': defineChannel({
    version: 1,
    capability: 'logs.export',
    request: z
      .object({
        reveal: z.boolean().optional(),
        diagnostics: z.boolean().optional(),
      })
      .default({}),
    response: z.object({
      savedTo: z.string().optional(),
      revealed: z.boolean().optional(),
      files: z.number().int().nonnegative(),
      bytes: z.number().int().nonnegative(),
      crashReports: z.number().int().nonnegative().optional(),
    }),
  }),
  /** Floating mini HUD: grow to show the notes field, or collapse back to the strip. */
  'hud:set-expanded': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({ expanded: z.boolean() }),
    response: z.object({ expanded: z.boolean() }),
  }),
  /** Privacy pause/resume of capture while recording (omit `paused` to toggle). */
  'note:pause': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({ paused: z.boolean().optional() }).default({}),
    response: SessionView,
  }),
  /** Full backup: zip of a consistent DB snapshot + recordings + a sha256 manifest. */
  'data:export-backup': defineChannel({
    version: 1,
    capability: 'data.backup',
    request: z.object({}).default({}),
    response: z.object({
      savedTo: z.string().optional(),
      meetings: z.number(),
      audioFiles: z.number(),
      bytes: z.number(),
    }),
  }),
  /** Verifies the zip (manifest hashes, SQLite integrity, schema version) and merges meetings that aren't present. */
  'data:import-backup': defineChannel({
    version: 1,
    capability: 'data.backup',
    request: z.object({}).default({}),
    response: z.object({
      canceled: z.boolean().optional(),
      imported: z.number(),
      skipped: z.number(),
      audioFiles: z.number(),
    }),
  }),
  // --- qrtz talk ---
  'talk:state': defineChannel({
    version: 1,
    capability: 'talk.read',
    request: z.void(),
    response: TalkView,
  }),
  'talk:start': defineChannel({
    version: 1,
    capability: 'talk.dictate',
    request: z.void(),
    response: TalkView,
  }),
  /** Stop listening, then finalize → clean up → insert. */
  'talk:stop': defineChannel({
    version: 1,
    capability: 'talk.dictate',
    request: z.void(),
    response: TalkView,
  }),
  'talk:cancel': defineChannel({
    version: 1,
    capability: 'talk.dictate',
    request: z.void(),
    response: TalkView,
  }),
  'talk:history': defineChannel({
    version: 1,
    capability: 'talk.read',
    request: z.void(),
    response: z.array(TalkEntry),
  }),
  'talk:delete-entry': defineChannel({
    version: 1,
    capability: 'talk.write',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ ok: z.boolean() }),
  }),
  'talk:clear-history': defineChannel({
    version: 1,
    capability: 'talk.write',
    request: z.void(),
    response: z.object({ ok: z.boolean() }),
  }),
  'talk:copy': defineChannel({
    version: 1,
    capability: 'talk.read',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ ok: z.boolean() }),
  }),
  'talk:dictionary': defineChannel({
    version: 1,
    capability: 'talk.read',
    request: z.void(),
    response: z.array(DictionaryEntry),
  }),
  'talk:set-dictionary': defineChannel({
    version: 1,
    capability: 'talk.write',
    request: z.object({ entries: z.array(DictionaryEntry).max(500) }),
    response: z.array(DictionaryEntry),
  }),
  // --- the package manager (ADR 0157): the marketplace and the module manager (ADR 0161) ---
  /** Installed packages and the stored index; local only. */
  'packages:list': defineChannel({
    version: 1,
    capability: 'packages.read',
    request: z.void(),
    response: PackagesState,
  }),
  /** Check the package hosts for a newer signed index (the user pressed "check"). */
  'packages:refresh': defineChannel({
    version: 1,
    capability: 'packages.manage',
    request: z.void(),
    response: PackagesState,
  }),
  /**
   * "check for updates" (the user pressed it): refresh the signed index and compare its newest `qrtz` with this
   * build. Installs nothing. `not-set-up` = the package hosts don't exist yet; nothing was requested.
   */
  'packages:check-update': defineChannel({
    version: 1,
    capability: 'app.update',
    request: z.void(),
    response: z
      .object({
        current: z.string(),
        state: z.enum(['not-set-up', 'none-listed', 'up-to-date', 'available']),
        latest: z.string().nullable(),
        sizeBytes: z.number().int().nonnegative().nullable(),
      })
      .strict(),
  }),
  /** ADR 0181: qrtz's own update state (no request is made). */
  'app-update:status': defineChannel({
    version: 1,
    capability: 'app.update.read',
    request: z.void(),
    response: AppUpdateStatus,
  }),
  /** ADR 0181: "check for updates" for qrtz itself: asks modules.qrtz.ai/v1/app/latest.json; installs nothing. */
  'app-update:check': defineChannel({
    version: 1,
    capability: 'app.update',
    request: z.void(),
    response: AppUpdateStatus,
  }),
  /**
   * ADR 0181: "update": download, verify (signature, notarized Developer ID bundle) and put the new qrtz.app in
   * place. Answers at once (`downloading`); `app-update:state` follows. Nothing new runs until a restart.
   */
  'app-update:download': defineChannel({
    version: 1,
    capability: 'app.update',
    request: z.void(),
    response: AppUpdateStatus,
  }),
  /** ADR 0181: "restart to update", only once the new version is in place (`ready`). */
  'app-update:restart': defineChannel({
    version: 1,
    capability: 'app.update',
    request: z.void(),
    response: z.object({ restarting: z.boolean() }),
  }),
  /** What a change would add, remove and download; changes nothing. */
  'packages:plan': defineChannel({
    version: 1,
    capability: 'packages.manage',
    request: PackageOp,
    response: PackagePlan,
  }),
  /** Carry out the plan the user saw, named by its hash and download size. */
  'packages:apply': defineChannel({
    version: 1,
    capability: 'packages.manage',
    request: z.object({
      planSha256: z.string().regex(/^[0-9a-f]{64}$/),
      downloadBytes: z.number().int().nonnegative(),
    }),
    response: PackagesApplied,
  }),
  /** Delete package dirs and store blobs nothing in qrtz.lock references. */
  'packages:prune': defineChannel({
    version: 1,
    capability: 'packages.manage',
    request: z.void(),
    response: PackagesPruned,
  }),
  /** ADR 0161: switch one extension on or off (writes only `modules.<name>.enabled`). */
  'packages:set-enabled': defineChannel({
    version: 1,
    capability: 'packages.manage',
    request: ModuleEnabled,
    response: ModuleEnabled,
  }),
  // --- qrtz speech setup (ADR 0046 §7) ---
  /** `extensions`: the status of only the models these need (ADR 0174: echo's view, before switching echo on). */
  'speech:status': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: z
      .object({
        extensions: z
          .array(z.enum(['echo', 'nano', 'ctrl']))
          .max(8)
          .optional(),
      })
      .optional(),
    response: SpeechSetupStatus,
  }),
  /**
   * Look for the newest release (version + size); never downloads. `extensions`: answer with the status of only
   * these models (ADR 0175: a module view must name exactly itself).
   */
  'speech:check': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: SpeechScope,
    response: SpeechSetupStatus,
  }),
  /**
   * Download + install the newest release in the background (resumes a paused or cut one). `force` = re-download.
   * `extensions`: only the models these need (echo, nano); omitted = the modules switched on.
   */
  'speech:download': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z
      .object({
        force: z.boolean().optional(),
        extensions: z
          .array(z.enum(['echo', 'nano', 'ctrl']))
          .max(8)
          .optional(),
      })
      .default({}),
    response: SpeechSetupStatus,
  }),
  /** `extensions`: only a download of exactly these models (ADR 0175: a module view must name exactly itself). */
  'speech:pause': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: SpeechScope,
    response: SpeechSetupStatus,
  }),
  /** `extensions`: only a download of exactly these models (ADR 0175: a module view must name exactly itself). */
  'speech:resume': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: SpeechScope,
    response: SpeechSetupStatus,
  }),
  /**
   * Remove every installed version and partial download (refused while a session runs). `extensions`: only the
   * models these need and no other switched-on module does (ADR 0175: a module view must name exactly itself).
   */
  'speech:delete': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: SpeechScope,
    response: SpeechSetupStatus,
  }),
  // --- qrtz ctrl do (ADR 0214) ---
  /**
   * Do one command now: ctrl takes the foreground and runs the decision chain until it's done, asks, needs an ok,
   * or stops (Esc, or any input of Jared's). Answers the run's view. One run at a time; a new command cancels a run
   * that was still waiting on Jared.
   */
  'ctrl:do': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ text: z.string().trim().min(1).max(CTRL_TEXT_MAX) }),
    response: CtrlRunView,
  }),
  /** Every run, newest first. */
  'ctrl:runs': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.void(),
    response: z.array(CtrlRunSummary),
  }),
  /** One run's view after the first `at` records of its log (omit `at` = all): scrub without acting. */
  'ctrl:run': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.object({
      runId: z.string().max(64),
      at: z.number().int().min(0).optional(),
    }),
    response: CtrlRunView,
  }),
  /** Answer the question a run is asking (one of its options), or cancel the run (`optionId: null`). */
  'ctrl:answer': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      runId: z.string().max(64),
      decisionId: z.string().max(32),
      optionId: z.string().max(128).nullable(),
    }),
    response: CtrlRunView,
  }),
  /** Approve (acts once, after the window is read again) or deny (the run ends, nothing acts) a gated action. */
  'ctrl:confirm': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      runId: z.string().max(64),
      gateId: z.string().max(32),
      answer: z.enum(['approve', 'deny']),
    }),
    response: CtrlRunView,
  }),
  /** The stop button: a run being driven stops before its next step (`stopping`); a waiting one ends now. */
  'ctrl:stop': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ runId: z.string().max(64) }),
    response: z.object({
      runId: z.string(),
      status: z.union([CtrlRunStatus, z.literal('stopping')]),
    }),
  }),
  /** ctrl's decision model (ADR 0116): consent, download state and which scorer decides. */
  'ctrl:model': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.void(),
    response: CtrlModelState,
  }),
  /**
   * Say yes (starts the verified download; the only network ctrl ever uses) or no (deletes the model; the heuristic
   * scorer decides). Answers the state right after the change.
   */
  'ctrl:model-consent': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ consent: z.boolean() }),
    response: CtrlModelState,
  }),
  /** qrtz snap (ADR 0159): what qrtz-snap last reported, for snap's view and its shortcut editor. */
  'snap:status': defineChannel({
    version: 1,
    capability: 'snap.read',
    request: z.void(),
    response: SnapStatus,
  }),
  /**
   * snap's accessibility prompt: qrtz-app asks macOS (which adds qrtz to the list and shows its prompt) and opens the
   * Accessibility pane. The view then polls `snap:status`, which reads the grant live. `{permission:
   * 'input-monitoring'}` is summon's prompt, the same way (and answers `inputMonitoring`).
   */
  'snap:request-access': defineChannel({
    version: 1,
    capability: 'snap.read',
    request: z
      .object({ permission: z.enum(['accessibility', 'input-monitoring']) })
      .optional(),
    response: z.object({
      opened: z.boolean(),
      axTrusted: z.boolean(),
      inputMonitoring: z.boolean().optional(),
    }),
  }),
  /**
   * ADR 0217: nano's Apple Calendar (read-only, on this Mac): on/off, the access state (read without asking), the
   * calendars to pick from, the chosen ones (null = all) and the last read.
   */
  'calendar:status': defineChannel({
    version: 1,
    capability: 'calendar.read',
    request: z.void(),
    response: CalendarStatus,
  }),
  /**
   * ADR 0217: the only call that asks macOS for calendar access (nano's 'connect calendar', or the settings
   * switch). On a grant the calendar is turned on and read.
   */
  'calendar:connect': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.void(),
    response: CalendarAccess,
  }),
  /**
   * ADR 0217 (H1): the calendar access state alone (never prompts), for the onboarding's calendar row. No calendars,
   * no events: those are calendar.read, which only nano holds.
   */
  'calendar:access': defineChannel({
    version: 1,
    capability: 'os.permissions',
    request: z.void(),
    response: CalendarAccess,
  }),
  /** ADR 0217: read the calendar again now. */
  'calendar:refresh': defineChannel({
    version: 1,
    capability: 'calendar.read',
    request: z.void(),
    response: CalendarStatus,
  }),
  /** ADR 0217: the nano page's 'upcoming' block: the next timed events on the chosen calendars. */
  'calendar:upcoming': defineChannel({
    version: 1,
    capability: 'calendar.read',
    request: z.void(),
    response: z.object({ events: z.array(CalendarEvent).max(20) }),
  }),
  /** ADR 0220: nano's notes vault (a folder of .md files the user can see; the host only). */
  'vault:get': defineChannel({
    version: 1,
    capability: 'doc.read',
    request: z.void(),
    response: VaultInfo,
  }),
  /** ADR 0220: pick another notes folder (the host's folder panel; the renderer never names a path). */
  'vault:choose': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.void(),
    response: VaultInfo.extend({ canceled: z.boolean() }),
  }),
  'doc:list': defineChannel({
    version: 1,
    capability: 'doc.read',
    request: z
      .object({
        query: z.string().max(200).optional(),
        meetingId: z.string().max(64).optional(),
        limit: z.number().int().min(1).max(10000).optional(),
      })
      .optional(),
    response: z.array(DocSummary),
  }),
  'doc:get': defineChannel({
    version: 1,
    capability: 'doc.read',
    request: z.object({ id: z.string().max(64) }),
    response: Doc,
  }),
  'doc:create': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z
      .object({
        title: z.string().max(200).optional(),
        meetingId: z.string().max(64).optional(),
      })
      .optional(),
    response: Doc,
  }),
  /** `baseHash` = the hash the editor loaded; a file changed since is E_CONFLICT unless `force` (keep mine). */
  'doc:save': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({
      id: z.string().max(64),
      body: z.string().max(1_000_000),
      baseHash: z.string().max(64),
      force: z.boolean().optional(),
    }),
    response: Doc,
  }),
  /** Moves the file into the vault's hidden .trash folder. */
  'doc:trash': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ ok: z.boolean() }),
  }),
  'doc:link-meeting': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({
      id: z.string().max(64),
      meetingId: z.string().max(64),
    }),
    response: Doc,
  }),
  'doc:unlink-meeting': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.object({
      id: z.string().max(64),
      meetingId: z.string().max(64),
    }),
    response: Doc,
  }),
  'doc:rebuild-index': defineChannel({
    version: 1,
    capability: 'note.write',
    request: z.void(),
    response: z.object({ count: z.number().int().nonnegative() }),
  }),
  /** ADR 0228: Agent Dash's read-only view (agents, their widgets, the audit log). Holds no token or code. */
  'dash:view': defineChannel({
    version: 1,
    capability: 'dash.read',
    request: z.void(),
    response: DashView,
  }),
  /** ADR 0228: a single-use enrollment code (15 min) the user hands one agent. Shown once, kept only hashed. */
  'dash:issue-code': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.void(),
    response: z.object({
      // QRT-284: the dash server issues `ENR-<id>.<secret>`; the in-app crate (qrtz-dash) still issues ENR-XXXX-XXXX.
      code: z
        .string()
        .regex(/^ENR-(?:[A-Z0-9]{4}-[A-Z0-9]{4}|[A-Z0-9]{8}\.[A-Z0-9]{17})$/),
      expiresAt: z.number(),
    }),
  }),
  /**
   * QRT-298: "Copy prompt". Only when called, qrtz-app has the connected dash server issue a fresh ENR- code, builds
   * `Read <origin>/v1/readme and enroll in my qrtz Dash with code ENR-…` and writes it to the clipboard itself. The
   * code never comes back here. Not connected: refused with `not_connected`, nothing issued.
   */
  'dash:copy-prompt': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.void(),
    response: z.object({ copied: z.literal(true) }),
  }),
  /** ADR 0228: invalidate every outstanding code (agent tokens keep working). */
  'dash:rotate-codes': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.void(),
    response: z.object({ ok: z.literal(true) }),
  }),
  /** ADR 0228: approve or reject a pending agent (account security, not a channel to the agent). */
  'dash:review-agent': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.object({
      agentId: z.string().max(64),
      approve: z.boolean(),
    }),
    response: z.object({ ok: z.boolean() }),
  }),
  /**
   * ADR 0228 (Bozniak M3): open an agent's link in the default browser. qrtz-app re-checks it is https and opens it
   * through the Tauri opener; the webview never opens links itself.
   */
  'dash:open-link': defineChannel({
    version: 1,
    capability: 'dash.read',
    request: z.object({ url: z.string().max(2048) }),
    response: z.object({ opened: z.boolean() }),
  }),
  /** ADR 0228: delete a widget, an agent or the dashboard; the agent learns it only as a 410. */
  'dash:delete': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: DashDelete,
    response: z.object({ ok: z.boolean() }),
  }),
  /**
   * ADR 0231 amendment 1: use the dash server through the qrtz-dash-net helper (opt-in; debug builds only). With a
   * setup token the helper trades it for a new dashboard's owner key, which it keeps in its own Keychain item (the
   * app never sees it); without one it reuses the dashboard it already holds a key for.
   */
  'dash:connect': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.object({ setupToken: z.string().min(1).max(256).optional() }),
    response: z.object({ connected: z.literal(true), origin: z.string() }),
  }),
  /** ADR 0231 amendment 1: forget the dash server (the helper deletes its owner key; the dashboard stays on the server) and stop the helper. */
  'dash:disconnect': defineChannel({
    version: 1,
    capability: 'dash.manage',
    request: z.void(),
    response: z.object({ ok: z.literal(true) }),
  }),
} as const satisfies Record<string, ChannelDef>;

export type IpcContract = typeof ipcContract;
export type IpcChannel = keyof IpcContract;
export type IpcRequest<C extends IpcChannel> = z.input<
  IpcContract[C]['request']
>;
export type IpcResponse<C extends IpcChannel> = z.output<
  IpcContract[C]['response']
>;

export const IpcEnvelope = z.object({
  v: z.literal(PROTOCOL_VERSION),
  id: z.string().min(1).max(64),
  channel: z.string().min(1).max(64),
  version: z.number().int().positive(),
  subApp: z.string().min(1).max(64),
  payload: z.unknown(),
});
export type IpcEnvelope = z.infer<typeof IpcEnvelope>;

export const IpcResult = z.discriminatedUnion('ok', [
  z.object({ ok: z.literal(true), id: z.string(), data: z.unknown() }),
  z.object({ ok: z.literal(false), id: z.string(), error: AppErrorJSON }),
]);
export type IpcResult = z.infer<typeof IpcResult>;

export const isIpcChannel = (c: unknown): c is IpcChannel =>
  typeof c === 'string' && Object.prototype.hasOwnProperty.call(ipcContract, c);
