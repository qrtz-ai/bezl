import type {
  CtrlApproval,
  CtrlDecision,
  CtrlEventRow,
  CtrlNode,
  CtrlRun,
  CtrlRunState,
  CtrlRunStatus,
  CtrlSchedule,
} from '@qrtz/core';
import {
  WEEKDAYS,
  type BadgeTone,
  type DecisionItem,
  type ScheduleKind,
} from '@qrtz/ui';

/** What the schedule picker holds. */
export interface ScheduleForm {
  kind: ScheduleKind;
  /** `hh:mm`, or '' (once: as soon as possible). */
  time: string;
  weekday: number;
}

const parseTime = (s: string): { hour: number; minute: number } | null => {
  const m = /^(\d{1,2}):(\d{2})$/.exec(s.trim());
  if (!m) return null;
  const hour = Number(m[1]);
  const minute = Number(m[2]);
  return hour < 24 && minute < 60 ? { hour, minute } : null;
};

const pad = (n: number) => String(n).padStart(2, '0');
const hhmm = (h: number, m: number) => `${pad(h)}:${pad(m)}`;

/**
 * The picker → the contract's schedule, or null if it isn't complete. `once` at a time means the next time the
 * clock reads it (today, or tomorrow if that minute has passed), in local time.
 */
export function toSchedule(f: ScheduleForm, now: Date): CtrlSchedule | null {
  const t = parseTime(f.time);
  if (f.kind === 'once') {
    if (!f.time.trim()) return { kind: 'once', at: null };
    if (!t) return null;
    const at = new Date(now);
    at.setHours(t.hour, t.minute, 0, 0);
    if (at.getTime() <= now.getTime()) at.setDate(at.getDate() + 1);
    return { kind: 'once', at: at.getTime() };
  }
  if (!t) return null;
  if (f.kind === 'daily') return { kind: 'daily', ...t };
  if (!(f.weekday >= 0 && f.weekday <= 6)) return null;
  return { kind: 'weekly', weekday: f.weekday, ...t };
}

/** "every day at 09:00", "every monday at 18:30", "once, as soon as possible", "once at 07:15". */
export function describeSchedule(s: CtrlSchedule): string {
  switch (s.kind) {
    case 'once': {
      if (s.at === null) return 'once, as soon as possible';
      const d = new Date(s.at);
      return `once at ${hhmm(d.getHours(), d.getMinutes())}`;
    }
    case 'daily':
      return `every day at ${hhmm(s.hour, s.minute)}`;
    case 'weekly':
      return `every ${WEEKDAYS[s.weekday]} at ${hhmm(s.hour, s.minute)}`;
  }
}

const STATUS: Record<CtrlRunStatus, [string, BadgeTone]> = {
  'pending-intent': ['waiting for intent', 'neutral'],
  asking: ['needs you', 'warning'],
  'needs-confirm': ['needs your ok', 'warning'],
  planned: ['planned', 'primary'],
  running: ['running', 'primary'],
  done: ['done', 'success'],
  failed: ['failed', 'danger'],
  cancelled: ['cancelled', 'neutral'],
  'asked-out': ['needs you', 'warning'],
  deferred: ['deferred', 'neutral'],
  expired: ['expired, nothing done', 'neutral'],
};
export const runStatus = (s: CtrlRunStatus) => STATUS[s];

export const when = (ms: number) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hhmm(d.getHours(), d.getMinutes())}`;
};

export const runItem = (r: CtrlRun) => {
  const [status, tone] = runStatus(r.status);
  const v = r.violations ? ` · ${r.violations} guard violations` : '';
  return {
    id: r.id,
    status,
    tone,
    meta: `${when(r.at)} · ${r.events} events${v}`,
  };
};

/** What each decision layer asks (ADR 0106). */
const QUESTION: Record<CtrlNode, string> = {
  choose_intent: 'what kind of task',
  choose_app: 'which app',
  choose_target: 'on what',
  choose_text: 'what to type',
  choose_move: 'which move',
  choose_control: 'which control',
};
export const nodeQuestion = (n: CtrlNode) => QUESTION[n];

/** `intent.pending` → "intent pending"; action events carry their action id, decisions their layer. */
export const eventItem = (e: CtrlEventRow) => {
  const meta =
    e.actionId ??
    (e.node && e.node in QUESTION ? QUESTION[e.node as CtrlNode] : null);
  return {
    seq: e.seq,
    label: e.kind.replace(/[._]/g, ' '),
    ...(meta ? { meta } : {}),
  };
};

/** A decision for the inspector: its options with the chosen one marked. */
export const decisionItem = (d: CtrlDecision): DecisionItem => ({
  id: d.id,
  question: QUESTION[d.node],
  outcome: d.outcome,
  answered: d.answered,
  meta: `${Math.round(d.p * 100)}% · ${d.model}`,
  options: d.options.map((o) => ({
    id: o.id,
    label: o.label,
    p: o.p,
    chosen: o.id === d.chosen && (d.outcome !== 'ask' || d.answered),
    irreversible: o.irreversible,
    refused: o.refusedInBackground,
  })),
});

/** Why the executor stopped before acting (ADR 0108); irreversible shows as the run's status. */
const HELD: Record<NonNullable<CtrlRunState['held']>, string> = {
  irreversible: 'asks before it acts',
  'files-not-yet': 'file moves come in a later version',
  'no-route': 'no way to do it in the background',
  'unknown-app': 'app not found',
  'folder-not-yet': "ctrl can't tidy this folder yet, nothing moved",
  // ADR 0158: what the user does next; nothing else is tried.
  'focus-no-shortcut': 'set up the shortcut',
  'automation-denied': 'allow automation',
};

/**
 * ADR 0158: the one shortcut ctrl runs for a focus task, made once by the user. The name is fixed in code (Rust
 * `FOCUS_SHORTCUT`, Swift `CtrlFocus.shortcutName`); it is shown as typed because Shortcuts matches it exactly.
 */
export const FOCUS_SHORTCUT = 'qrtz Set Focus';
/** ADR 0158: what the user does once, shown in ctrl's own view when a focus run stops for it. */
export const FOCUS_SETUP_STEPS = [
  'open shortcuts and make a new shortcut',
  'add the set focus action and set its focus to the shortcut input',
  `name it ${FOCUS_SHORTCUT}`,
  'run this task again',
];
/** ADR 0158: where Automation is turned back on. */
export const AUTOMATION_STEPS = [
  'open system settings, privacy and security, automation',
  'turn on shortcuts events under qrtz',
  'run this task again',
];

/** The reducer's state in one line. */
export function stateSummary(s: CtrlRunState): string {
  const parts = [runStatus(s.status)[0]];
  if (s.decisions.length) parts.push(`${s.decisions.length} decisions`);
  if (s.actions.length) {
    const done = s.actions.filter((a) => a.status === 'done').length;
    parts.push(`${done}/${s.actions.length} actions done`);
  }
  const verified = s.verified ?? [];
  if (verified.length) {
    const ok = verified.filter((v) => v.outcome !== 'failed').length;
    parts.push(`${ok}/${verified.length} verified`);
  }
  if (s.held && s.held !== 'irreversible') parts.push(HELD[s.held]);
  if (s.violations) parts.push(`${s.violations} guard violations`);
  if (s.quarantined.length) parts.push(`${s.quarantined.length} routes off`);
  return parts.join(' · ');
}

/** A standing approval as the task shows it (ADR 0111): "quit an app in textedit, up to 2 items". */
export function describeApproval(a: CtrlApproval): string {
  const action = a.intent.replace(/^intent\./, '').replace(/_/g, ' ');
  const app = a.app ? ` in ${a.app.split('.').pop()!.toLowerCase()}` : '';
  const unsaved = a.unsaved ? `, ${a.unsaved} unsaved` : '';
  return `${action}${app}, up to ${a.items} ${a.items === 1 ? 'item' : 'items'}${unsaved}`;
}
