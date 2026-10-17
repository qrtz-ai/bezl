import { z } from 'zod';

/**
 * Where the words came from: the qrtz speech module behind the helper (`on-device`). `elevenlabs` is read only, from
 * history written before the cloud engine was removed (ADR 0076).
 */
export const TalkEngineId = z.enum(['elevenlabs', 'on-device']);
export type TalkEngineId = z.infer<typeof TalkEngineId>;

export const TalkPhase = z.enum([
  'idle',
  /** Mic open, text streaming. */
  'listening',
  /** Key released; waiting for the engine's final text. */
  'finalizing',
  'inserting',
  'error',
]);
export type TalkPhase = z.infer<typeof TalkPhase>;

export const InsertMethod = z.enum([
  'ax-selected-text',
  'ax-value',
  'paste',
  'none',
]);
export type InsertMethod = z.infer<typeof InsertMethod>;

/** Milliseconds; every mark is optional because a dictation can end early. */
export const TalkLatency = z.object({
  /** Key-down → first partial on screen. */
  firstPartialMs: z.number().optional(),
  /** Key-up → final ASR text. */
  finalMs: z.number().optional(),
  /** Cleanup pass duration (LLM or rules). */
  cleanupMs: z.number().optional(),
  /** Key-up → text inserted into the focused app (the number users feel). */
  insertedMs: z.number().optional(),
});
export type TalkLatency = z.infer<typeof TalkLatency>;

/**
 * A stored history entry from the removed Deepgram engine keeps its text and reads as `elevenlabs`: still a
 * cloud engine, so history never claims it stayed on the device (crates/qrtz-talk `store::legacy_entry`).
 */
export function legacyTalkEntry(x: unknown): unknown {
  if (typeof x !== 'object' || x === null || Array.isArray(x)) return x;
  const e = x as Record<string, unknown>;
  return e.engine === 'deepgram' ? { ...e, engine: 'elevenlabs' } : x;
}

export const TalkEntry = z.object({
  id: z.string(),
  at: z.number(),
  raw: z.string(),
  text: z.string(),
  cleanup: z.enum(['llm', 'rules', 'off']),
  engine: TalkEngineId,
  /** Legacy (read only): the removed cloud engine failed and on-device text was used. Always false now. */
  failover: z.boolean().default(false),
  insert: InsertMethod,
  inserted: z.boolean(),
  app: z.string().optional(),
  durationMs: z.number(),
  latency: TalkLatency,
  error: z.string().optional(),
});
export type TalkEntry = z.infer<typeof TalkEntry>;

/** Why a dictation ended in `error` (machine-readable; `error` is the sentence shown). */
export const TalkErrorKind = z.enum([
  'mic-denied',
  'mic-did-not-start',
  'busy',
  'helper-lost',
  'helper-unavailable',
  'permission-revoked',
  'mic-lost',
  'speech-incompatible',
  'speech-unavailable',
  'speech-lost',
]);
export type TalkErrorKind = z.infer<typeof TalkErrorKind>;

export const TalkView = z.object({
  phase: TalkPhase,
  id: z.string().optional(),
  startedAt: z.number().optional(),
  /** Committed (final) text so far. */
  text: z.string(),
  /** Current unstable hypothesis after `text`. */
  partial: z.string(),
  level: z.number(),
  engine: TalkEngineId.optional(),
  failover: z.boolean().optional(),
  error: z.string().optional(),
  errorKind: TalkErrorKind.optional(),
  last: TalkEntry.optional(),
});
export type TalkView = z.infer<typeof TalkView>;

export const DictionaryEntry = z.object({
  /** How it should be written. */
  term: z.string().trim().min(1).max(80),
  /** What the recogniser tends to hear instead (optional). */
  aliases: z.array(z.string().trim().min(1).max(80)).max(10).default([]),
});
export type DictionaryEntry = z.infer<typeof DictionaryEntry>;

export const idleTalkView = (): TalkView => ({
  phase: 'idle',
  text: '',
  partial: '',
  level: 0,
});
