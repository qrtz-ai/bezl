import { describe, expect, it } from 'vitest';
import { CtrlSchedule } from '@qrtz/core';
import {
  decisionItem,
  describeSchedule,
  eventItem,
  runItem,
  runStatus,
  stateSummary,
  toSchedule,
} from './schedule';

// Local time throughout: a schedule is wall-clock time (ADR 0103).
const now = new Date(2026, 9, 18, 8, 30, 0);

describe('ctrl schedule form', () => {
  it('builds every kind the contract accepts', () => {
    const daily = toSchedule({ kind: 'daily', time: '09:05', weekday: 0 }, now);
    expect(daily).toEqual({ kind: 'daily', hour: 9, minute: 5 });
    const weekly = toSchedule(
      { kind: 'weekly', time: '18:00', weekday: 1 },
      now,
    );
    expect(weekly).toEqual({ kind: 'weekly', weekday: 1, hour: 18, minute: 0 });
    const asap = toSchedule({ kind: 'once', time: '', weekday: 0 }, now);
    expect(asap).toEqual({ kind: 'once', at: null });
    for (const s of [daily, weekly, asap])
      expect(CtrlSchedule.safeParse(s).success).toBe(true);
  });

  it('a once time is the next time the clock reads it', () => {
    const later = toSchedule({ kind: 'once', time: '09:00', weekday: 0 }, now);
    expect(later).toEqual({
      kind: 'once',
      at: new Date(2026, 9, 18, 9, 0).getTime(),
    });
    const passed = toSchedule({ kind: 'once', time: '08:30', weekday: 0 }, now);
    expect(passed).toEqual({
      kind: 'once',
      at: new Date(2026, 9, 19, 8, 30).getTime(),
    });
  });

  it('refuses incomplete or impossible times', () => {
    expect(toSchedule({ kind: 'daily', time: '', weekday: 0 }, now)).toBeNull();
    expect(
      toSchedule({ kind: 'daily', time: '24:00', weekday: 0 }, now),
    ).toBeNull();
    expect(
      toSchedule({ kind: 'weekly', time: '09:00', weekday: 7 }, now),
    ).toBeNull();
    expect(
      toSchedule({ kind: 'once', time: '9:6', weekday: 0 }, now),
    ).toBeNull();
  });

  it('describes schedules in lowercase', () => {
    expect(describeSchedule({ kind: 'daily', hour: 9, minute: 0 })).toBe(
      'every day at 09:00',
    );
    expect(
      describeSchedule({ kind: 'weekly', weekday: 0, hour: 18, minute: 30 }),
    ).toBe('every sunday at 18:30');
    expect(describeSchedule({ kind: 'once', at: null })).toBe(
      'once, as soon as possible',
    );
    expect(
      describeSchedule({
        kind: 'once',
        at: new Date(2026, 9, 18, 7, 15).getTime(),
      }),
    ).toBe('once at 07:15');
  });
});

describe('ctrl run history', () => {
  it('words runs, events and the scrubbed state', () => {
    const r = runItem({
      id: 'run-1',
      taskId: 'task-0',
      at: new Date(2026, 9, 18, 8, 31).getTime(),
      status: 'pending-intent',
      events: 2,
      violations: 0,
    });
    expect(r).toEqual({
      id: 'run-1',
      status: 'waiting for intent',
      tone: 'neutral',
      meta: '2026-10-18 08:31 · 2 events',
    });
    expect(
      eventItem({ seq: 3, kind: 'guard.result', at: 0, actionId: 'a1' }),
    ).toEqual({ seq: 3, label: 'guard result', meta: 'a1' });
    expect(
      stateSummary({
        actions: [
          { id: 'a1', status: 'done' },
          { id: 'a2', status: 'refused' },
        ],
        violations: 1,
        inconclusive: 0,
        quarantined: [{ app: 'com.apple.Safari', rung: 'ax_action' }],
        status: 'running',
        decisions: [],
        plan: null,
        ask: null,
      }),
    ).toBe('running · 1/2 actions done · 1 guard violations · 1 routes off');
    expect(
      stateSummary({
        actions: [
          { id: 'a0', status: 'done' },
          { id: 'a1', status: 'done' },
        ],
        violations: 0,
        inconclusive: 0,
        quarantined: [],
        status: 'done',
        decisions: [],
        plan: null,
        ask: null,
        verified: [
          { actionId: 'a0', outcome: 'failed' },
          { actionId: 'a1', outcome: 'passed' },
        ],
        held: null,
      }),
    ).toBe('done · 2/2 actions done · 1/2 verified');
    expect(runStatus('needs-confirm')).toEqual([
      'asks before it acts',
      'warning',
    ]);
  });

  it('words the decision layers: status, event layer, inspector items', () => {
    expect(runStatus('asking')).toEqual(['needs you', 'warning']);
    expect(runStatus('planned')).toEqual(['planned', 'primary']);
    expect(
      eventItem({
        seq: 3,
        kind: 'decision.made',
        at: 0,
        actionId: null,
        node: 'choose_app',
      }),
    ).toEqual({ seq: 3, label: 'decision made', meta: 'which app' });
    const o = (id: string, p: number) => ({
      id,
      label: id,
      p,
      irreversible: id === 'b',
      refusedInBackground: false,
    });
    const d = {
      id: 'd1',
      node: 'choose_app' as const,
      outcome: 'ask' as const,
      answered: true,
      chosen: 'b',
      chosenLabel: 'b',
      p: 0.4,
      model: 'qrtz-heuristic 1',
      options: [o('a', 0.4), o('b', 0.35), o('none', 0.25)],
    };
    const item = decisionItem(d);
    expect(item.question).toBe('which app');
    expect(item.meta).toBe('40% · qrtz-heuristic 1');
    expect(item.options.filter((x) => x.chosen).map((x) => x.id)).toEqual([
      'b',
    ]);
    expect(item.options[1]!.irreversible).toBe(true);
    // An unanswered ask has no chosen option yet.
    expect(
      decisionItem({ ...d, answered: false }).options.some((x) => x.chosen),
    ).toBe(false);
  });
});
