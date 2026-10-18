import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  AskCard,
  CtrlTaskRow,
  DecisionList,
  PlanSummary,
  RunHistory,
  RunTimeline,
  SchedulePicker,
} from './Ctrl';

const noop = () => {};
const picker = (kind: 'once' | 'daily' | 'weekly', time = '09:00') =>
  renderToStaticMarkup(
    <SchedulePicker
      kind={kind}
      time={time}
      weekday={1}
      onKind={noop}
      onTime={noop}
      onWeekday={noop}
    />,
  );

describe('ctrl components', () => {
  it('the schedule picker shows the day only for weekly and asap only for an empty once', () => {
    expect(picker('daily')).not.toContain('ctrl-schedule-day');
    const weekly = picker('weekly');
    expect(weekly).toContain('ctrl-schedule-day');
    expect(weekly).toContain('>monday</option>');
    expect(weekly).toMatch(
      /aria-pressed="true"[^>]*data-testid="ctrl-schedule-weekly"/,
    );
    expect(picker('once', '')).toContain('as soon as possible');
    expect(picker('once', '07:00')).not.toContain('as soon as possible');
  });

  it('a task row says when it runs and that it is paused', () => {
    const html = renderToStaticMarkup(
      <CtrlTaskRow
        text="every day close all my tabs"
        schedule="every day at 09:00"
        meta="never run"
        paused
        onSelect={noop}
        onRun={noop}
        onPause={noop}
        onDelete={noop}
      />,
    );
    expect(html).toContain('data-paused="true"');
    expect(html).toContain('paused · every day at 09:00');
    expect(html).toContain('aria-label="resume"');
    expect(html).toContain('aria-label="run now"');
    expect(html).toContain('data-icon="play"');
  });

  it('history marks the selected run; the timeline dims events after the scrubber', () => {
    const runs = renderToStaticMarkup(
      <RunHistory
        runs={[
          { id: 'run-2', status: 'done', tone: 'success', meta: 'b' },
          {
            id: 'run-1',
            status: 'waiting for intent',
            tone: 'neutral',
            meta: 'a',
          },
        ]}
        selected="run-1"
        onSelect={noop}
      />,
    );
    expect(runs.match(/aria-current="true"/g)).toHaveLength(1);
    expect(runs).toContain('waiting for intent');
    const tl = renderToStaticMarkup(
      <RunTimeline
        events={[
          { seq: 0, label: 'task started' },
          { seq: 1, label: 'intent pending' },
        ]}
        at={1}
        onAt={noop}
        summary="running"
      />,
    );
    expect(tl).toContain('step 1 of 2 · running');
    expect(tl.match(/data-applied="true"/g)).toHaveLength(1);
    expect(tl).toContain('max="2"');
  });

  it('the decision inspector shows the top options with bars, the chosen one and irreversible marks', () => {
    const opts = Array.from({ length: 7 }, (_, i) => ({
      id: `o${i}`,
      label: `option ${i}`,
      p: (7 - i) / 28,
      chosen: i === 6,
      irreversible: i === 0,
      refused: i === 1,
    }));
    const html = renderToStaticMarkup(
      <DecisionList
        decisions={[
          {
            id: 'd0',
            question: 'which app',
            outcome: 'tentative',
            meta: '25% · qrtz-heuristic 1',
            options: opts,
          },
        ]}
      />,
    );
    expect(html).toContain('data-outcome="tentative"');
    expect(html).toContain('tentative');
    // Top 4 plus the chosen one (ranked last): 5 rows, 2 summarized.
    expect(html.match(/data-testid="ctrl-option"/g)).toHaveLength(5);
    expect(html).toContain('2 more options');
    expect(html.match(/data-chosen="true"/g)).toHaveLength(1);
    expect(html).toContain('aria-label="can&#x27;t be undone"');
    expect(html).toContain('not in the background');
    expect(html).toContain('25%');
  });

  it('the plan says when it waits for an ok and the ask offers its options and cancel', () => {
    const plan = renderToStaticMarkup(
      <PlanSummary
        steps={['empty the trash', 'files (no app)']}
        irreversible
      />,
    );
    expect(plan).toContain('plan: empty the trash → files (no app)');
    expect(plan).toContain('asks you before it acts');
    const ask = renderToStaticMarkup(
      <AskCard
        question="what kind of task"
        options={[
          { id: 'a', label: 'close browser tabs' },
          { id: 'b', label: 'clean up a folder' },
        ]}
        onPick={noop}
        onCancel={noop}
      />,
    );
    expect(ask).toContain('what kind of task: which one?');
    expect(ask.match(/data-testid="ctrl-ask-option"/g)).toHaveLength(2);
    expect(ask).toContain('cancel run');
  });
});
