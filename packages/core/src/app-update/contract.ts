import { z } from 'zod';

/**
 * ADR 0181: qrtz's own update, as settings → qrtz shows it and the sidebar dot reads it (`app-update:status`, the
 * `app-update:state` event). `not-set-up`: this build has no app-update key, so nothing is asked of any host.
 */
export const AppUpdateState = z.enum([
  'not-set-up',
  'idle',
  'checking',
  'up-to-date',
  'available',
  'downloading',
  'installing',
  'ready',
  'failed',
]);
export type AppUpdateState = z.infer<typeof AppUpdateState>;

/** QRT-313: the one coded update error (self_update.rs `update_redirect::CODE`). */
export const AppUpdateErrorCode = z.enum(['update_redirect_blocked']);
export type AppUpdateErrorCode = z.infer<typeof AppUpdateErrorCode>;

export const AppUpdateStatus = z
  .object({
    current: z.string(),
    state: AppUpdateState,
    latest: z.string().nullable(),
    notes: z.string().max(4000).nullable(),
    sizeBytes: z.number().int().nonnegative().nullable(),
    doneBytes: z.number().int().nonnegative().nullable(),
    error: z.string().nullable(),
    /**
     * QRT-313: set with `error` when the download was redirected somewhere other than GitHub's asset hosts
     * (`update_redirect_blocked`); `errorDetails` names the refused host and scheme. Null for any other error.
     */
    errorCode: AppUpdateErrorCode.nullable().optional(),
    errorDetails: z
      .object({ host: z.string(), scheme: z.string() })
      .strict()
      .nullable()
      .optional(),
    checkedAtMs: z.number().int().nonnegative().nullable(),
  })
  .strict();
export type AppUpdateStatus = z.infer<typeof AppUpdateStatus>;

/** The sidebar dot: a newer qrtz is waiting (to download, or downloaded and waiting for a restart). */
export const appUpdateDot = (s: AppUpdateStatus | null | undefined) =>
  s?.state === 'available' || s?.state === 'ready';
