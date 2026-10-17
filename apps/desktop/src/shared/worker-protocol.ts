import { z } from 'zod';
import { AppErrorJSON } from '@qrtz/core';

/** main ↔ worker (utilityProcess) messages. */
export const WorkerRequest = z.object({
  kind: z.literal('req'),
  id: z.number(),
  method: z.string(),
  params: z.unknown().optional(),
});

const WorkerResponse = z.discriminatedUnion('ok', [
  z.object({
    kind: z.literal('res'),
    id: z.number(),
    ok: z.literal(true),
    data: z.unknown(),
  }),
  z.object({
    kind: z.literal('res'),
    id: z.number(),
    ok: z.literal(false),
    error: AppErrorJSON,
  }),
]);

export const WorkerMessage = z.union([
  WorkerResponse,
  z.object({ kind: z.literal('heartbeat'), ts: z.number() }),
  z.object({ kind: z.literal('ready') }),
  z.object({
    kind: z.literal('log'),
    level: z.enum(['debug', 'info', 'warn', 'error']),
    msg: z.string(),
    fields: z.record(z.string(), z.unknown()).optional(),
  }),
]);
export type WorkerRequest = z.infer<typeof WorkerRequest>;
export type WorkerMessage = z.infer<typeof WorkerMessage>;

export interface WorkerMethods {
  'db.health': {
    params: void;
    result: { ok: boolean; schemaVersion: number; fts5: boolean };
  };
}
