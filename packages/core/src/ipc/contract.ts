import { z } from 'zod';
import type { Capability } from '../capabilities';
import { AppErrorJSON } from '../errors';
import { OsPermission } from '../capabilities';
import {
  MeetingDetail,
  MeetingListItem,
  PermissionsStatus,
  SessionView,
} from '../memo/contract';
import { MeetingSummary } from '../memo/summary';

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
  'memo:session': defineChannel({
    version: 1,
    capability: 'memo.read',
    request: z.void(),
    response: SessionView,
  }),
  'memo:start': defineChannel({
    version: 1,
    capability: 'memo.record',
    request: z.object({ title: z.string().max(200).optional() }).optional(),
    response: SessionView,
  }),
  'memo:stop': defineChannel({
    version: 1,
    capability: 'memo.record',
    request: z.void(),
    response: SessionView,
  }),
  'memo:dismiss': defineChannel({
    version: 1,
    capability: 'memo.record',
    request: z.void(),
    response: SessionView,
  }),
  'memo:list': defineChannel({
    version: 1,
    capability: 'memo.read',
    request: z.object({
      query: z.string().max(200).optional(),
      /** Page size (default 100). Up to limit+1 rows come back; the extra one signals more. */
      limit: z.number().int().min(1).max(1000).optional(),
      offset: z.number().int().min(0).optional(),
    }),
    response: z.array(MeetingListItem),
  }),
  'memo:get': defineChannel({
    version: 1,
    capability: 'memo.read',
    request: z.object({ id: z.string().max(64) }),
    response: MeetingDetail,
  }),
  'memo:export': defineChannel({
    version: 1,
    capability: 'memo.read',
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
  'memo:resummarize': defineChannel({
    version: 1,
    capability: 'memo.write',
    request: z.object({ id: z.string().max(64) }),
    response: MeetingSummary,
  }),
  'memo:delete': defineChannel({
    version: 1,
    capability: 'memo.write',
    request: z.object({ id: z.string().max(64) }),
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
