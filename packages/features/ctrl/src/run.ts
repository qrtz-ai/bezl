import type {
  CtrlDecision,
  CtrlEventRow,
  CtrlRunStatus,
  CtrlRunSummary,
  CtrlRunView,
  CtrlStop,
} from '@qrtz/core';
import type { BadgeTone, DecisionItem } from '@qrtz/ui';

const STATUS: Record<CtrlRunStatus, [string, BadgeTone]> = {
  running: ['running', 'primary'],
  asking: ['asking you', 'warning'],
  'needs-confirm': ['needs your ok', 'warning'],
  done: ['done', 'success'],
  failed: ["couldn't do it", 'danger'],
  cancelled: ['cancelled', 'neutral'],
  'asked-out': ['out of ideas', 'warning'],
  stopped: ['stopped', 'neutral'],
  'out-of-steps': ['too many steps', 'warning'],
};
export const runStatus = (s: CtrlRunStatus) => STATUS[s];

const STOP: Record<CtrlStop, string> = {
  'stop-key': 'you pressed esc',
  'user-input': 'you took over',
  'stop-button': 'you pressed stop',
  'no-watcher': "ctrl couldn't watch for esc, so it didn't act",
};
export const stopReason = (s: CtrlStop) => STOP[s];

const pad = (n: number) => String(n).padStart(2, '0');
export const when = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

/** A history row: "open youtube", its status, when and how many actions. */
export const runItem = (r: CtrlRunSummary) => {
  const [status, tone] = runStatus(r.status);
  return {
    id: r.runId,
    status: `${r.text} · ${status}`,
    tone,
    meta: `${when(r.startedAt)} · ${r.actions} ${r.actions === 1 ? 'action' : 'actions'}`,
  };
};

/** `decision.made` → "decision made". */
export const eventItem = (e: CtrlEventRow) => ({
  seq: e.seq,
  label: e.kind.replace(/[._]/g, ' '),
});

/** A decision for the inspector: what it chose, how sure, and how many options it weighed. */
export const decisionItem = (d: CtrlDecision): DecisionItem => ({
  id: d.id,
  question: d.question,
  outcome: d.outcome,
  answered: d.answered,
  meta: `${Math.round(d.p * 100)}% · ${d.options} options · ${d.model}${d.backtracks ? ` · tried ${d.backtracks} other${d.backtracks === 1 ? '' : 's'}` : ''}`,
  options: [{ id: d.chosen, label: d.chosenLabel, p: d.p, chosen: true }],
});

/** One line under the timeline: status, app, actions, and why it stopped. */
export function runSummary(v: CtrlRunView): string {
  const parts = [runStatus(v.status)[0]];
  if (v.app) parts.push(v.app);
  parts.push(
    `${v.actions.length} ${v.actions.length === 1 ? 'action' : 'actions'}`,
  );
  if (v.stop) parts.push(stopReason(v.stop));
  return parts.join(' · ');
}

/** The run is still live at the end of its log (not scrubbed back, not finished). */
export const live = (v: CtrlRunView) => v.at === v.total;
