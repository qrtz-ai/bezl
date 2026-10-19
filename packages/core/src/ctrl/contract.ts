import { z } from 'zod';

/**
 * ctrl do (ADR 0214): Jared types one command and ctrl does it in the foreground through a chain of small decisions
 * over the macOS accessibility tree (choose the app → a move → descend → an element action → the text → act →
 * verify). Esc stops a run at once; any other key, click or scroll also stops it (user input wins). Mirrors
 * crates/qrtz-ctrl `service.rs`. Labels are lowercase UI copy from memory, never the log.
 */
export const CTRL_TEXT_MAX = 500;

/**
 * Derived from the run's log: finished (done, failed, cancelled, asked-out = out of alternatives, stopped,
 * out-of-steps = the step budget ran out), else asking Jared, else waiting for his ok, else running.
 */
export const CtrlRunStatus = z.enum([
  'running',
  'asking',
  'needs-confirm',
  'done',
  'failed',
  'cancelled',
  'asked-out',
  'stopped',
  'out-of-steps',
]);
export type CtrlRunStatus = z.infer<typeof CtrlRunStatus>;

/** Why a run stopped: Esc, Jared's own input, the stop button, or no stop-key watcher (ctrl then never acts),
 * naming the missing permission or secure input when the helper knows it. */
export const CtrlStop = z.enum([
  'stop-key',
  'user-input',
  'stop-button',
  'no-watcher',
  'no-accessibility',
  'no-input-monitoring',
  'secure-input',
]);
export type CtrlStop = z.infer<typeof CtrlStop>;

export const CtrlNode = z.enum([
  'choose-app',
  'choose-move',
  'choose-element',
  'choose-text',
  'verify',
]);
export type CtrlNode = z.infer<typeof CtrlNode>;

/** One run in the history. */
export const CtrlRunSummary = z.object({
  runId: z.string(),
  doId: z.string(),
  text: z.string(),
  status: CtrlRunStatus,
  startedAt: z.number(),
  /** Actions it took. */
  actions: z.number().int().nonnegative(),
});
export type CtrlRunSummary = z.infer<typeof CtrlRunSummary>;

/** One option Jared can pick, with its calibrated probability. `gated`: acting on it asks for his ok first. */
export const CtrlOption = z.object({
  id: z.string(),
  label: z.string(),
  p: z.number().min(0).max(1),
  gated: z.boolean(),
});
export type CtrlOption = z.infer<typeof CtrlOption>;

export const CtrlDecision = z.object({
  id: z.string(),
  node: CtrlNode,
  question: z.string(),
  chosen: z.string(),
  chosenLabel: z.string(),
  p: z.number().min(0).max(1),
  /** act (p ≥ 0.8), tentative (≥ 0.5, alternatives kept for backtracking) or ask Jared. */
  outcome: z.enum(['act', 'tentative', 'ask']),
  answered: z.boolean(),
  backtracks: z.number().int().nonnegative(),
  /** "qrtz-heuristic 2" or the consented model's id and version. */
  model: z.string(),
  /** How many options the question had (at most 20, "none of these" included). */
  options: z.number().int().nonnegative(),
});
export type CtrlDecision = z.infer<typeof CtrlDecision>;

/** The question waiting for Jared: the top three options. */
export const CtrlAsk = z.object({
  decisionId: z.string(),
  node: CtrlNode,
  question: z.string(),
  options: z.array(CtrlOption).max(3),
});
export type CtrlAsk = z.infer<typeof CtrlAsk>;

/**
 * The confirm gate: the host's rules matched (a send, a purchase, a deletion, a quit, a System Settings change, …).
 * An approval covers exactly this action on this element in this window; if the window changes before ctrl acts,
 * it asks again.
 */
export const CtrlConfirm = z.object({
  gateId: z.string(),
  rules: z.array(z.string()),
  action: z.string(),
  app: z.string(),
});
export type CtrlConfirm = z.infer<typeof CtrlConfirm>;

export const CtrlAction = z.object({
  id: z.string(),
  kind: z.enum([
    'press',
    'set_value',
    'increment',
    'decrement',
    'show_menu',
    'focus',
    'scroll',
    'key_combo',
    'open_url',
    'open_app',
  ]),
  label: z.string(),
  status: z.enum(['requested', 'refused', 'done', 'failed']),
  /** A short code (`act.stale`, `app.not_running`), never content. */
  err: z.string().nullable(),
});
export type CtrlAction = z.infer<typeof CtrlAction>;

/** One record of a run's log as the timeline shows it: its kind only. */
export const CtrlEventRow = z.object({
  seq: z.number().int().nonnegative(),
  kind: z.string(),
});
export type CtrlEventRow = z.infer<typeof CtrlEventRow>;

/** One run after the first `at` of its `total` log records (omit `at` = all): scrub without acting. */
export const CtrlRunView = z.object({
  runId: z.string(),
  doId: z.string(),
  text: z.string(),
  status: CtrlRunStatus,
  stop: CtrlStop.nullable(),
  app: z.string().nullable(),
  startedAt: z.number(),
  at: z.number().int().nonnegative(),
  total: z.number().int().nonnegative(),
  events: z.array(CtrlEventRow),
  decisions: z.array(CtrlDecision),
  actions: z.array(CtrlAction),
  ask: CtrlAsk.nullable(),
  confirm: CtrlConfirm.nullable(),
  backtracks: z.number().int().nonnegative(),
});
export type CtrlRunView = z.infer<typeof CtrlRunView>;

/**
 * ctrl's Laya decision model (ADR 0116). Downloaded only after the user says yes here; until then
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
