import { z } from 'zod';

/**
 * qrtz speech setup (ADR 0046 §7): the installed module and its download, as settings and onboarding show it.
 * Model-agnostic: a version, a size and a download state; never an engine or model name.
 */
export const SpeechDownloadState = z.enum([
  'idle',
  'checking',
  'downloading',
  'paused',
  'installing',
  'error',
]);
export type SpeechDownloadState = z.infer<typeof SpeechDownloadState>;

export const SpeechErrorKind = z.enum([
  'network',
  'checksum',
  'signature',
  'list-expired',
  'health',
  'disk',
  'busy',
  'unavailable',
  'other',
]);
export type SpeechErrorKind = z.infer<typeof SpeechErrorKind>;

export const SpeechSetupStatus = z.object({
  /** This build has a speech module source (download server + signing keys). */
  configured: z.boolean(),
  /** Installed version, or null when speech is not downloaded. */
  installed: z.string().nullable(),
  /** Bytes on disk for every installed version. */
  bytes: z.number().int().nonnegative(),
  /** Newest release the last check saw, and its download size. */
  latest: z.string().nullable(),
  latestBytes: z.number().int().nonnegative().nullable(),
  /** A verified update waiting for the current session to end. */
  waiting: z.string().nullable(),
  /** The revocation list could not be refreshed in time: no install or update until it can. */
  listExpired: z.boolean(),
  download: z.object({
    state: SpeechDownloadState,
    done: z.number().int().nonnegative(),
    total: z.number().int().nonnegative(),
    error: z.string().optional(),
    errorKind: SpeechErrorKind.optional(),
  }),
});
export type SpeechSetupStatus = z.infer<typeof SpeechSetupStatus>;
