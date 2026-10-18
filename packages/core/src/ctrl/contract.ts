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

/**
 * A standing approval of the confirm gate (ADR 0111): this task may do this action (intent + move) in this app
 * without asking again, as long as it touches no more items and no more unsaved ones than were approved.
 */
export const CtrlApproval = z.object({
  id: z.string(),
  intent: z.string(),
  move: z.string(),
  app: z.string().nullable(),
  items: z.number().int().nonnegative(),
  unsaved: z.number().int().nonnegative(),
  grantedAt: z.number(),
});
export type CtrlApproval = z.infer<typeof CtrlApproval>;

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
  /** Standing approvals of the confirm gate, oldest first (revocable, ADR 0111). */
  approvals: z.array(CtrlApproval).optional(),
});
export type CtrlTask = z.infer<typeof CtrlTask>;

/**
 * Derived from the run's event log: `task.finished`, else asking the user (a decision layer wasn't sure), else
 * the confirm gate waiting for the user's ok (needs-confirm, ADR 0111), else waiting for an intent, else planned (a
 * plan and no action: no helper connected, or a held file route), else running. `expired`: the gate went
 * unanswered past its timeout (or a newer run replaced it); nothing acted.
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
  'expired',
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

/**
 * The confirm gate (ADR 0111): exactly what an irreversible step would do, item by item. `label`s are lowercase;
 * window and tab titles come from memory only (never the log), else "window 2". `complete: false`: the helper
 * could only count the items. `answer`: approved, remembered (a standing approval was granted), standing (one
 * covered it, no question asked), denied or expired.
 */
export const CtrlConfirm = z.object({
  gateId: z.string(),
  action: z.string(),
  app: z.string().nullable(),
  move: z.string().nullable(),
  items: z.array(
    z.object({
      key: z.string(),
      kind: z.enum(['window', 'tab', 'file', 'action']),
      label: z.string(),
      unsaved: z.boolean().nullable(),
    }),
  ),
  complete: z.boolean(),
  rules: z.array(z.string()),
  answer: z
    .enum(['approved', 'remembered', 'standing', 'denied', 'expired'])
    .nullable(),
  /** The re-check right before acting: still inside what was approved. */
  reverified: z.boolean().nullable(),
  /** The approved action ran (an approval acts once). */
  used: z.boolean(),
});
export type CtrlConfirm = z.infer<typeof CtrlConfirm>;

/**
 * Putting a finished file batch back (ADR 0117). `ready`: files that would go back now (read from the disk at the
 * end of the log; 0 while scrubbed back). `cantUndo`: files that stay where they are: `moved` (renamed or moved
 * since) or `occupied` (something with its name is back where it came from). `done`: the batch was put back.
 */
export const CtrlUndo = z.object({
  actionId: z.string(),
  files: z.number().int().nonnegative(),
  ready: z.number().int().nonnegative(),
  cantUndo: z.array(
    z.object({
      key: z.string(),
      label: z.string(),
      why: z.enum(['moved', 'occupied']),
    }),
  ),
  done: z.object({ undone: z.number().int().nonnegative() }).nullable(),
});
export type CtrlUndo = z.infer<typeof CtrlUndo>;

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
  /** The confirm gate of the current plan, if it has one (ADR 0111). */
  confirm: CtrlConfirm.nullable().optional(),
  /** The run's finished file batch and whether it can be (or was) put back (ADR 0117). */
  undo: CtrlUndo.nullable().optional(),
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

/**
 * ctrl's Laya decision model (ADR 0116). Downloaded only after the user says yes here or in onboarding; until then
 * (and whenever the model is missing or fails) the deterministic heuristic scorer decides. `unavailable`: consented,
 * but the pinned manifest is still a placeholder, so nothing can be fetched. Mirrors qrtz-app `ctrl_model`.
 */
export const CtrlModelState = z.object({
  consented: z.boolean(),
  state: z.enum([
    'off',
    'unavailable',
    'not_installed',
    'downloading',
    'installed',
    'failed',
  ]),
  sizeBytes: z.number().int().nonnegative(),
  bytesOnDisk: z.number().int().nonnegative(),
  scorer: z.enum(['heuristic', 'laya']),
  /** A short code when the last download failed (`laya.placeholder`, `checksum`), never content. */
  error: z.string().max(64).nullable(),
});
export type CtrlModelState = z.infer<typeof CtrlModelState>;
