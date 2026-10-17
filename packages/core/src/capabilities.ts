import { z } from 'zod';

/** Fine-grained capabilities a sub app must declare to call IPC channels or receive events. */
export const Capability = z.enum([
  'app.info',
  'config.read',
  'config.write',
  'db.read',
  'helper.status',
  'os.permissions',
  'note.record',
  'note.read',
  'note.write',
  'llm.invoke',
  'data.backup',
  /** qrtz talk: read dictation state/history, start/stop dictation, edit history + dictionary. */
  'talk.read',
  'talk.dictate',
  'talk.write',
  /** qrtz speech (ADR 0046) and the on-device search model: read setup state / download, update, delete. */
  'speech.read',
  'speech.manage',
  /** ADR 0083: write a line to the local log (uncaught renderer errors, the audio player's errors). */
  'log.write',
  /** ADR 0083: save a copy of the local logs, or show their folder. */
  'logs.export',
]);
export type Capability = z.infer<typeof Capability>;

/** macOS TCC permissions a sub app needs; drives onboarding. */
export const OsPermission = z.enum([
  'accessibility',
  'screen-recording',
  'microphone',
]);
export type OsPermission = z.infer<typeof OsPermission>;
