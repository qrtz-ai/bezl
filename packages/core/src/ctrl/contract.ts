import { z } from 'zod';

/**
 * ctrl (ADR 0100, 0103): typed tasks that run in the background, once or on a cadence. The schedule is local
 * wall-clock time (hour, minute, weekday 0 = sunday), so "every day at 9" stays 9 across DST; `once` is one
 * instant (epoch ms) or `null` = as soon as possible. Mirrors crates/qrtz-ctrl `task::Schedule`.
 */
export const CTRL_TEXT_MAX = 500;
const hour = z.number().int().min(0).max(23);
const minute = z.number().int().min(0).max(59);
export const CtrlSchedule = z.discriminatedUnion('kind', [
  z.object({
    kind: z.literal('once'),
    at: z.number().int().nonnegative().nullable(),
  }),
  z.object({ kind: z.literal('daily'), hour, minute }),
  z.object({
    kind: z.literal('weekly'),
    weekday: z.number().int().min(0).max(6),
    hour,
    minute,
  }),
]);
export type CtrlSchedule = z.infer<typeof CtrlSchedule>;

export const CtrlTask = z.object({
  id: z.string(),
  text: z.string(),
  schedule: CtrlSchedule,
  paused: z.boolean(),
  createdAt: z.number(),
  /** How many runs it has (history, newest first, from `ctrl:runs`). */
  runs: z.number().int().nonnegative(),
  lastRunAt: z.number().nullable(),
});
export type CtrlTask = z.infer<typeof CtrlTask>;

/** Derived from the run's event log: `task.finished`, else waiting for an intent, else running. */
export const CtrlRunStatus = z.enum([
  'pending-intent',
  'running',
  'done',
  'failed',
  'cancelled',
  'asked-out',
  'deferred',
]);
export type CtrlRunStatus = z.infer<typeof CtrlRunStatus>;

export const CtrlRun = z.object({
  id: z.string(),
  taskId: z.string(),
  at: z.number(),
  status: CtrlRunStatus,
  /** Records in the run's log. */
  events: z.number().int().nonnegative(),
  violations: z.number().int().nonnegative(),
});
export type CtrlRun = z.infer<typeof CtrlRun>;

/** One record of a run's log as the timeline shows it: ids and kinds only (ADR 0083). */
export const CtrlEventRow = z.object({
  seq: z.number().int().nonnegative(),
  kind: z.string(),
  at: z.number(),
  actionId: z.string().nullable(),
});
export type CtrlEventRow = z.infer<typeof CtrlEventRow>;

export const CtrlActionStatus = z.enum([
  'requested',
  'executing',
  'deferred',
  'asked',
  'refused',
  'done',
  'failed',
]);

/** The reducer's state after `at` events (crates/qrtz-ctrl `log::CtrlState`). */
export const CtrlRunState = z.object({
  actions: z.array(z.object({ id: z.string(), status: CtrlActionStatus })),
  violations: z.number().int().nonnegative(),
  inconclusive: z.number().int().nonnegative(),
  quarantined: z.array(z.object({ app: z.string(), rung: z.string() })),
  status: CtrlRunStatus,
});
export type CtrlRunState = z.infer<typeof CtrlRunState>;

export const CtrlRunView = z.object({
  run: CtrlRun,
  events: z.array(CtrlEventRow),
  /** Events applied to `state` (0 … events.length): the scrubber's position. */
  at: z.number().int().nonnegative(),
  state: CtrlRunState,
});
export type CtrlRunView = z.infer<typeof CtrlRunView>;
