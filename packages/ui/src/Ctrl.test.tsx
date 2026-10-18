import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  AskCard,
  ConfirmCard,
  ConfirmOutcome,
  CtrlTaskRow,
  DecisionList,
  DecisionModelCard,
  PlanSummary,
  RunHistory,
  RunTimeline,
  SchedulePicker,
  StandingApprovals,
  UndoCard,
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

  it('says why when ctrl could not look inside any of the tabs, and only then', () => {
    const tabs = (unsaved: boolean | null) => [
      { key: 'a#0', kind: 'tab', label: 'mail', unsaved },
      { key: 'b#0', kind: 'tab', label: 'form', unsaved: null },
    ];
    const render = (items: ReturnType<typeof tabs>, complete = true) =>
      renderToStaticMarkup(
        <ConfirmCard
          action="close browser tabs"
          app="safari"
          items={items}
          complete={complete}
          onApprove={noop}
          onRemember={noop}
          onDeny={noop}
        />,
      );
    const html = render(tabs(null));
    expect(html).toContain('ctrl-confirm-tabs-hint');
    expect(html).toContain('allow javascript from apple events');
    expect(html.replace(/<[^>]+>/g, '')).toBe(
      html.replace(/<[^>]+>/g, '').toLowerCase(),
    );
    expect(render(tabs(true))).not.toContain('ctrl-confirm-tabs-hint');
    expect(render(tabs(null), false)).not.toContain('ctrl-confirm-tabs-hint');
  });

  it('the confirm card lists every item, flags unsaved work, and is a group, never a dialog', () => {
    const html = renderToStaticMarkup(
      <ConfirmCard
        action="quit an app"
        app="textedit"
        items={[
          { key: 'a#0', label: 'notes.txt', unsaved: false },
          { key: 'b#0', label: 'draft.txt', unsaved: true },
          { key: 'n2', label: 'window 3', unsaved: null },
        ]}
        complete={false}
        onApprove={noop}
        onRemember={noop}
        onDeny={noop}
      />,
    );
    expect(html).toContain('role="group"');
    expect(html).not.toContain('dialog');
    expect(html).toContain('quit an app · textedit: this can&#x27;t be undone');
    expect(html).toContain('ctrl could only count them');
    expect(html.match(/data-testid="ctrl-confirm-item"/g)).toHaveLength(3);
    expect(html).toContain('draft.txt · unsaved changes');
    expect(html).toContain('window 3 · may have unsaved changes');
    expect(html).toContain('2 with unsaved work');
    for (const id of ['approve', 'remember', 'deny'])
      expect(html).toContain(`ctrl-confirm-${id}`);
    expect(renderToStaticMarkup(<ConfirmOutcome answer="expired" />)).toContain(
      'no answer in time; nothing was done',
    );
    const list = renderToStaticMarkup(
      <StandingApprovals
        approvals={[
          { id: 'approval-4', label: 'quit app in textedit, up to 2 items' },
        ]}
        onRevoke={noop}
      />,
    );
    expect(list).toContain('ctrl-approval-revoke');
    expect(list).toContain('quit app in textedit, up to 2 items');
  });
});

describe('DecisionModelCard (ADR 0116)', () => {
  const base = {
    consented: false,
    state: 'off' as const,
    sizeBytes: 0,
    scorer: 'heuristic' as const,
    error: null,
  };
  it('offers the download only as a choice, all lowercase', () => {
    const html = renderToStaticMarkup(
      <DecisionModelCard model={base} onConsent={noop} />,
    );
    expect(html).toContain('data-testid="ctrl-model-on"');
    expect(html).not.toContain('ctrl-model-off');
    expect(html).toContain('built-in rules');
    expect(html.replace(/<[^>]*>/g, '')).toBe(
      html.replace(/<[^>]*>/g, '').toLowerCase(),
    );
  });
  it('after a yes on the placeholder it says nothing was downloaded and offers turning off', () => {
    const html = renderToStaticMarkup(
      <DecisionModelCard
        model={{
          ...base,
          consented: true,
          state: 'unavailable',
          error: 'laya.placeholder',
        }}
        onConsent={noop}
      />,
    );
    expect(html).toContain('data-state="unavailable"');
    expect(html).toContain('nothing was downloaded');
    expect(html).toContain('data-testid="ctrl-model-off"');
  });
  it('names the on-device model only when it decides', () => {
    const html = renderToStaticMarkup(
      <DecisionModelCard
        model={{
          ...base,
          consented: true,
          state: 'installed',
          scorer: 'laya',
          sizeBytes: 420e6,
        }}
        onConsent={noop}
      />,
    );
    expect(html).toContain('deciding with the on-device model');
  });
});

describe('UndoCard', () => {
  const card = (p: Partial<Parameters<typeof UndoCard>[0]>) =>
    renderToStaticMarkup(
      <UndoCard
        files={3}
        ready={3}
        cantUndo={[]}
        done={null}
        onUndo={noop}
        onRest={noop}
        {...p}
      />,
    );
  it('offers to put everything back when nothing changed', () => {
    const html = card({});
    expect(html).toContain('role="group"');
    expect(html).not.toContain('dialog');
    expect(html).toContain('ctrl moved 3 files');
    expect(html).toContain('ctrl-undo-all');
    expect(html).not.toContain('ctrl-undo-rest');
  });
  it("says what can't go back and only offers the rest", () => {
    const html = card({
      ready: 1,
      cantUndo: [
        { key: 'a', label: 'photo.png', why: 'moved' },
        { key: 'b', label: 'taxes.pdf', why: 'occupied' },
      ],
    });
    expect(html).toContain('2 files can&#x27;t go back');
    expect(html).toContain('photo.png · moved or renamed since');
    expect(html).toContain('taxes.pdf · a file with its name is back');
    expect(html).toContain('put back the other 1');
    expect(html).not.toContain('ctrl-undo-all');
  });
  it('shows nothing to press when nothing can go back, and the outcome once done', () => {
    expect(
      card({ ready: 0, cantUndo: [{ key: 'a', label: 'x', why: 'moved' }] }),
    ).toContain('nothing can go back right now');
    const done = card({
      ready: 0,
      done: 2,
      cantUndo: [{ key: 'a', label: 'x', why: 'moved' }],
    });
    expect(done).toContain('put back 2 files');
    expect(done).toContain('1 file stayed put');
    expect(done).not.toContain('<button');
  });
});
