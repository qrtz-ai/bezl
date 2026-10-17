import { z } from 'zod';
import type { Capability } from '../capabilities';
import { AppErrorJSON } from '../errors';
import { OsPermission } from '../capabilities';
import {
  MeetingDetail,
  MeetingListItem,
  PermissionsStatus,
  SessionView,
} from '../note/contract';
import { MeetingSummary } from '../note/summary';
import { LiveTranscript, PipelineView, StepId } from '../note/lifecycle';
import { ChatAnswer, ChatTurn } from '../note/chat';
import { MeetingNote } from '../note/notes';
import { SummaryTemplateId } from '../note/templates';
import { DictionaryEntry, TalkEntry, TalkView } from '../talk/contract';
import { SearchModelStatus, SpeechSetupStatus } from '../speech/setup';

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
  'secrets:set': defineChannel({
    version: 1,
    capability: 'secrets.write',
    request: z.object({
      name: z.string().regex(/^[a-z0-9._-]+$/),
      value: z.string().max(4096).nullable(),
    }),
    response: z.object({ ok: z.literal(true) }),
  }),
  'secrets:list': defineChannel({
    version: 1,
    capability: 'secrets.write',
    request: z.void(),
    response: z.array(z.string()),
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
  /** Scoped config write so Note's prompt can enable auto-record without the broad config.write capability. */
  'note:set-auto-record': defineChannel({
    version: 1,
    capability: 'note.record',
    request: z.object({ enabled: z.boolean() }),
    response: z.object({ autoRecord: z.boolean() }),
  }),
  'note:chat': defineChannel({
    version: 1,
    capability: 'note.read',
    request: z.object({
      question: z.string().min(1).max(2000),
      meetingId: z.string().max(64).optional(),
      history: z.array(ChatTurn).max(20).optional(),
    }),
    response: ChatAnswer,
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
  // --- qrtz speech setup (ADR 0046 §7) and the search model ---
  'speech:status': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: z.void(),
    response: SpeechSetupStatus,
  }),
  /** Look for the newest release (version + size); never downloads. */
  'speech:check': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: z.void(),
    response: SpeechSetupStatus,
  }),
  /** Download + install the newest release in the background (resumes a paused or cut one). `force` = re-download. */
  'speech:download': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z.object({ force: z.boolean().optional() }).default({}),
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
  'search:model-status': defineChannel({
    version: 1,
    capability: 'speech.read',
    request: z.void(),
    response: SearchModelStatus,
  }),
  'search:model-download': defineChannel({
    version: 1,
    capability: 'speech.manage',
    request: z.void(),
    response: SearchModelStatus,
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
