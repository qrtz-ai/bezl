import { AppError } from '../errors';
import type { Logger } from '../logger';
import { nullLogger } from '../logger';
import type { SubAppManifest } from '../subapp';
import {
  IpcEnvelope,
  ipcContract,
  isIpcChannel,
  type IpcChannel,
  type IpcContract,
  type IpcResult,
} from './contract';
import type { z } from 'zod';

export interface CallContext {
  subApp: SubAppManifest;
  requestId: string;
}

export type Handlers = {
  [C in IpcChannel]: (
    req: z.output<IpcContract[C]['request']>,
    ctx: CallContext,
  ) =>
    | Promise<z.input<IpcContract[C]['response']>>
    | z.input<IpcContract[C]['response']>;
};

export interface RouterOptions {
  handlers: Partial<Handlers>;
  resolveSubApp: (id: string) => SubAppManifest | undefined;
  logger?: Logger;
  /** Validate handler responses too (cheap; on by default). */
  validateResponses?: boolean;
}

/** Transport-agnostic, fully validated IPC dispatcher. Never throws. */
export function createRouter(opts: RouterOptions) {
  const log = opts.logger ?? nullLogger;
  const validateResponses = opts.validateResponses ?? true;

  async function dispatch(raw: unknown): Promise<IpcResult> {
    const rawId =
      typeof raw === 'object' &&
      raw !== null &&
      typeof (raw as { id?: unknown }).id === 'string'
        ? (raw as { id: string }).id
        : 'unknown';
    try {
      const env = IpcEnvelope.safeParse(raw);
      if (!env.success)
        throw new AppError('E_VALIDATION', 'Malformed IPC envelope');
      const { id, channel, version, subApp, payload } = env.data;
      if (!isIpcChannel(channel))
        throw new AppError('E_UNKNOWN_CHANNEL', `Unknown channel "${channel}"`);
      const def = ipcContract[channel];
      if (version !== def.version) {
        throw new AppError(
          'E_VERSION',
          `Channel "${channel}" is v${def.version}, caller sent v${version}`,
        );
      }
      const manifest = opts.resolveSubApp(subApp);
      if (!manifest)
        throw new AppError('E_FORBIDDEN', `Unknown sub app "${subApp}"`);
      if (!manifest.capabilities.includes(def.capability)) {
        throw new AppError(
          'E_FORBIDDEN',
          `Sub app "${subApp}" lacks capability "${def.capability}"`,
        );
      }
      const req = def.request.safeParse(payload);
      if (!req.success) {
        throw new AppError('E_VALIDATION', `Invalid request for "${channel}"`, {
          details: req.error.issues.map((i) => ({
            path: i.path.join('.'),
            message: i.message,
          })),
        });
      }
      const handler = opts.handlers[channel] as
        | ((r: unknown, c: CallContext) => unknown)
        | undefined;
      if (!handler)
        throw new AppError('E_UNAVAILABLE', `No handler for "${channel}"`);
      let data = await handler(req.data, { subApp: manifest, requestId: id });
      if (validateResponses) {
        const res = def.response.safeParse(data);
        if (!res.success)
          throw new AppError(
            'E_INTERNAL',
            `Handler for "${channel}" returned invalid data`,
          );
        data = res.data;
      }
      return { ok: true, id, data };
    } catch (err) {
      const e = AppError.from(err);
      (e.code === 'E_INTERNAL' ? log.error : log.warn)('ipc call failed', {
        id: rawId,
        code: e.code,
        err: e.code === 'E_INTERNAL' ? err : e.message,
      });
      return { ok: false, id: rawId, error: e.toJSON() };
    }
  }

  return { dispatch };
}
