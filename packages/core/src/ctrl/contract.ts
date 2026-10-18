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
  /** Next due instant (epoch ms), or null when paused / once already run / no schedule left (ADR 0105). */
  nextRunAt: z.number().nullable(),
});
export type CtrlTask = z.infer<typeof CtrlTask>;

/**
 * Derived from the run's event log: `task.finished`, else asking the user (a decision layer wasn't sure), else
 * waiting for an intent, else needs-confirm (an irreversible plan held before acting, ADR 0108), else planned (a
 * plan and no action: no helper connected, or a held file route), else running.
 */
export const CtrlRunStatus = z.enum([
  'pending-intent',
  'asking',
  'needs-confirm',
  'planned',
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
  /** `decision.made` only: which decision layer (`choose_intent`, `choose_app`, …). */
  node: z.string().nullable().optional(),
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

/** The decision layers (ADR 0106): intent → app → target / text → move. */
export const CtrlNode = z.enum([
  'choose_intent',
  'choose_app',
  'choose_target',
  'choose_text',
  'choose_move',
  /** Inside a chosen AX pane (window/pane → control), ADR 0108. */
  'choose_control',
]);
export type CtrlNode = z.infer<typeof CtrlNode>;

/** One enumerated option of a decision, with its calibrated probability. `label` is lowercase UI copy. */
export const CtrlOption = z.object({
  id: z.string(),
  label: z.string(),
  p: z.number().min(0).max(1),
  /** Can't be undone (send, empty trash, quit, a system setting): the confirm gate (slice 6) asks first. */
  irreversible: z.boolean(),
  /** A focus-intent shortcut the ladder refuses in the background (ADR 0100). */
  refusedInBackground: z.boolean(),
});
export type CtrlOption = z.infer<typeof CtrlOption>;

export const CtrlDecision = z.object({
  id: z.string(),
  node: CtrlNode,
  /** act (p ≥ 0.8), tentative (≥ 0.5, alternatives kept for backtracking) or ask the user. */
  outcome: z.enum(['act', 'tentative', 'ask']),
  answered: z.boolean(),
  chosen: z.string(),
  chosenLabel: z.string(),
  p: z.number().min(0).max(1),
  /** "qrtz-heuristic 1" or the consented model's id and version. */
  model: z.string(),
  options: z.array(CtrlOption),
});
export type CtrlDecision = z.infer<typeof CtrlDecision>;

export const CtrlPlan = z.object({
  steps: z.array(
    z.object({ node: CtrlNode, option: z.string(), label: z.string() }),
  ),
  irreversible: z.boolean(),
});
export type CtrlPlan = z.infer<typeof CtrlPlan>;

/** The decision waiting for the user: its top three options ("which one: …"). */
export const CtrlAsk = z.object({
  decisionId: z.string(),
  node: CtrlNode,
  options: z.array(CtrlOption),
});
export type CtrlAsk = z.infer<typeof CtrlAsk>;

/** The reducer's state after `at` events (crates/qrtz-ctrl `log::CtrlState`). */
export const CtrlRunState = z.object({
  actions: z.array(z.object({ id: z.string(), status: CtrlActionStatus })),
  violations: z.number().int().nonnegative(),
  inconclusive: z.number().int().nonnegative(),
  quarantined: z.array(z.object({ app: z.string(), rung: z.string() })),
  status: CtrlRunStatus,
  decisions: z.array(CtrlDecision),
  plan: CtrlPlan.nullable(),
  ask: CtrlAsk.nullable(),
  /** Why the executor stopped before acting (ADR 0108). */
  held: z
    .enum(['irreversible', 'files-not-yet', 'no-route', 'unknown-app'])
    .nullable()
    .optional(),
  /** Each action's verify step: did the move do what it meant? A failure backtracks. */
  verified: z
    .array(
      z.object({
        actionId: z.string(),
        outcome: z.enum(['passed', 'failed', 'unverifiable']),
      }),
    )
    .optional(),
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
