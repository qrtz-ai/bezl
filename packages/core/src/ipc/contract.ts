import { z } from 'zod';
import type { Capability } from '../capabilities';
import { AppErrorJSON } from '../errors';
import { OsPermission, SettingsPane } from '../capabilities';
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
  CtrlRun,
  CtrlRunView,
  CtrlSchedule,
  CtrlTask,
  CtrlModelState,
} from '../ctrl/contract';
import { SnapStatus } from '../snap/contract';

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

/** The single source of truth for request/response IPC. */
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
    request: z.object({ permission: OsPermission }),
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
  /** Look for the newest release (version + size); never downloads. */
  'speech:check': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: z.void(),
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
  'speech:pause': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z.void(),
    response: SpeechSetupStatus,
  }),
  'speech:resume': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z.void(),
    response: SpeechSetupStatus,
  }),
  /** Remove every installed version and partial download (refused while a session runs). */
  'speech:delete': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z.void(),
    response: SpeechSetupStatus,
  }),
  // --- qrtz ctrl (ADR 0100, 0103) ---
  /** Every task, newest first. */
  'ctrl:tasks': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.void(),
    response: z.array(CtrlTask),
  }),
  'ctrl:create-task': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      text: z.string().trim().min(1).max(CTRL_TEXT_MAX),
      schedule: CtrlSchedule,
    }),
    response: CtrlTask,
  }),
  'ctrl:set-paused': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ id: z.string().max(64), paused: z.boolean() }),
    response: CtrlTask,
  }),
  /** The task leaves the list; its runs' logs stay on disk (append-only, ADR 0103). */
  'ctrl:delete-task': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ id: z.string().max(64) }),
    response: z.object({ ok: z.literal(true) }),
  }),
  /** Record a run now; the decision layers decide its intent right away (`planned` or `asking`, ADR 0106). */
  'ctrl:run-now': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({ id: z.string().max(64) }),
    response: CtrlRun,
  }),
  /** A task's runs, newest first, each derived from its event log. */
  'ctrl:runs': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.object({ taskId: z.string().max(64) }),
    response: z.array(CtrlRun),
  }),
  /** One run's events and the state after the first `at` of them (omit `at` = all): scrub without executing. */
  'ctrl:run': defineChannel({
    version: 1,
    capability: 'ctrl.read',
    request: z.object({
      runId: z.string().max(64),
      at: z.number().int().min(0).optional(),
    }),
    response: CtrlRunView,
  }),
  /**
   * Answer the decision a run is asking about (one of its top three options), or cancel the run (`optionId:
   * null`). The decision layers then continue (ADR 0106). Answers the run's view after all events.
   */
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
  /**
   * Answer a run's confirm gate (ADR 0111): approve (acts once, after the preview is checked again), remember
   * (approve, and let this task do the same again without asking while it stays inside this scope) or deny (the
   * run ends, nothing acts). Answers the run's view after all events.
   */
  'ctrl:confirm': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      runId: z.string().max(64),
      gateId: z.string().max(32),
      answer: z.enum(['approve', 'remember', 'deny']),
    }),
    response: CtrlRunView,
  }),
  /**
   * Put a finished downloads cleanup back (ADR 0117): every file goes back where it was, all or nothing. If any file
   * moved since, ctrl refuses (logged, nothing moves) unless `rest` says to put back the others. Once per run.
   */
  'ctrl:undo': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      runId: z.string().max(64),
      rest: z.boolean().optional(),
    }),
    response: CtrlRunView,
  }),
  /** Drop a task's standing approval: the next run of that action asks again. */
  'ctrl:revoke-approval': defineChannel({
    version: 1,
    capability: 'ctrl.write',
    request: z.object({
      taskId: z.string().max(64),
      approvalId: z.string().max(64),
    }),
    response: CtrlTask,
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
   * Accessibility pane. The view then polls `snap:status`, which reads the grant live.
   */
  'snap:request-access': defineChannel({
    version: 1,
    capability: 'snap.read',
    request: z.void(),
    response: z.object({ opened: z.boolean(), axTrusted: z.boolean() }),
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
