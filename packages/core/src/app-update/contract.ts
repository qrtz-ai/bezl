import { z } from 'zod';

/**
 * ADR 0180: qrtz's own update, as settings → qrtz shows it and the sidebar dot reads it (`app-update:status`, the
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

export const AppUpdateStatus = z
  .object({
    current: z.string(),
    state: AppUpdateState,
    latest: z.string().nullable(),
    notes: z.string().max(4000).nullable(),
    sizeBytes: z.number().int().nonnegative().nullable(),
    doneBytes: z.number().int().nonnegative().nullable(),
    error: z.string().nullable(),
    checkedAtMs: z.number().int().nonnegative().nullable(),
  })
  .strict();
export type AppUpdateStatus = z.infer<typeof AppUpdateStatus>;

/** The sidebar dot: a newer qrtz is waiting (to download, or downloaded and waiting for a restart). */
export const appUpdateDot = (s: AppUpdateStatus | null | undefined) =>
  s?.state === 'available' || s?.state === 'ready';
