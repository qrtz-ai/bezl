import { describe, expect, it } from 'vitest';
import type { CtrlRunView } from '@qrtz/core';
import {
  decisionItem,
  eventItem,
  runItem,
  runSummary,
  stopReason,
} from './run';

const view: CtrlRunView = {
  runId: 'run-1',
  doId: 'do-0',
  text: 'open youtube',
  status: 'stopped',
  stop: 'stop-key',
  app: 'safari',
  startedAt: 0,
  at: 3,
  total: 3,
  events: [],
  decisions: [],
  actions: [
    {
      id: 'a0',
      kind: 'open_app',
      label: 'open safari',
      status: 'done',
      err: null,
    },
  ],
  ask: null,
  confirm: null,
  backtracks: 0,
};

describe('ctrl run helpers', () => {
  it('summarizes a run in lowercase, with why it stopped', () => {
    expect(runSummary(view)).toBe(
      'stopped · safari · 1 action · you pressed esc',
    );
    expect(stopReason('user-input')).toBe('you took over');
  });

  it('a history row carries the command and its status', () => {
    const r = runItem({
      runId: 'run-1',
      doId: 'do-0',
      text: 'open youtube',
      status: 'done',
      startedAt: 0,
      actions: 3,
    });
    expect(r.status).toBe('open youtube · done');
    expect(r.meta).toMatch(/3 actions$/);
    expect(r.tone).toBe('success');
  });

  it('events read as words and decisions show how sure they were', () => {
    expect(eventItem({ seq: 2, kind: 'decision.made' }).label).toBe(
      'decision made',
    );
    const d = decisionItem({
      id: 'd0',
      node: 'choose-app',
      question: 'which app should do it?',
      chosen: 'app.com.apple.Safari',
      chosenLabel: 'open safari',
      p: 0.93,
      outcome: 'act',
      answered: false,
      backtracks: 1,
      model: 'qrtz-heuristic 2',
      options: 7,
    });
    expect(d.meta).toBe('93% · 7 options · qrtz-heuristic 2 · tried 1 other');
    expect(d.options[0]!.chosen).toBe(true);
  });
});
