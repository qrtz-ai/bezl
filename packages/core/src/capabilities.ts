import { z } from 'zod';

/** Fine-grained capabilities a sub app must declare to call IPC channels or receive events. */
export const Capability = z.enum([
  'app.info',
  'config.read',
  'config.write',
  'secrets.write',
  'db.read',
  'helper.status',
  'memo.record',
  'memo.read',
  'memo.write',
  'llm.invoke',
]);
export type Capability = z.infer<typeof Capability>;

/** macOS TCC permissions a sub app needs; drives onboarding. */
export const OsPermission = z.enum([
  'accessibility',
  'screen-recording',
  'microphone',
]);
export type OsPermission = z.infer<typeof OsPermission>;
