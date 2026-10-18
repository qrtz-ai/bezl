import type {
  CtrlEventRow,
  CtrlRun,
  CtrlRunState,
  CtrlRunStatus,
  CtrlSchedule,
} from '@qrtz/core';
import { WEEKDAYS, type BadgeTone, type ScheduleKind } from '@qrtz/ui';

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
  running: ['running', 'primary'],
  done: ['done', 'success'],
  failed: ['failed', 'danger'],
  cancelled: ['cancelled', 'neutral'],
  'asked-out': ['needs you', 'warning'],
  deferred: ['deferred', 'neutral'],
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

/** `intent.pending` → "intent pending"; action events carry their action id. */
export const eventItem = (e: CtrlEventRow) => ({
  seq: e.seq,
  label: e.kind.replace(/[._]/g, ' '),
  ...(e.actionId ? { meta: e.actionId } : {}),
});

/** The reducer's state in one line. */
export function stateSummary(s: CtrlRunState): string {
  const parts = [runStatus(s.status)[0]];
  if (s.actions.length) {
    const done = s.actions.filter((a) => a.status === 'done').length;
    parts.push(`${done}/${s.actions.length} actions done`);
  }
  if (s.violations) parts.push(`${s.violations} guard violations`);
  if (s.quarantined.length) parts.push(`${s.quarantined.length} routes off`);
  return parts.join(' · ');
}
