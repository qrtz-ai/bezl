import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  AskCard,
  ConfirmCard,
  DecisionList,
  DecisionModelCard,
  RunHistory,
  RunTimeline,
  StopBar,
  ruleLabel,
} from './Ctrl';

const noop = () => {};

describe('ctrl components', () => {
  it('history marks the selected run; the timeline dims events after the scrubber', () => {
    const runs = renderToStaticMarkup(
      <RunHistory
        runs={[
          { id: 'run-2', status: 'done', tone: 'success', meta: 'b' },
          {
            id: 'run-1',
            status: 'asking',
            tone: 'neutral',
            meta: 'a',
          },
        ]}
        selected="run-1"
        onSelect={noop}
      />,
    );
    expect(runs.match(/aria-current="true"/g)).toHaveLength(1);
    expect(runs).toContain('asking');
    const tl = renderToStaticMarkup(
      <RunTimeline
        events={[
          { seq: 0, label: 'started' },
          { seq: 1, label: 'read the apps' },
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

  it('the decision inspector shows the top options with bars, the chosen one and gated marks', () => {
    const opts = Array.from({ length: 7 }, (_, i) => ({
      id: `o${i}`,
      label: `option ${i}`,
      p: (7 - i) / 28,
      chosen: i === 6,
      irreversible: i === 0,
    }));
    const html = renderToStaticMarkup(
      <DecisionList
        decisions={[
          {
            id: 'd0',
            question: 'which app',
            outcome: 'tentative',
            meta: '25% · qrtz-heuristic 2',
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
    expect(html).toContain('25%');
  });

  it('the ask offers its options and cancel', () => {
    const html = renderToStaticMarkup(
      <AskCard
        question="what should ctrl do next?"
        options={[
          { id: 'a', label: 'switch workspace… (menu)' },
          { id: 'b', label: 'look in web area' },
        ]}
        onPick={noop}
        onCancel={noop}
      />,
    );
    expect(html).toContain('switch workspace… (menu)');
    expect(html).toContain('cancel run');
  });

  it('the confirm card names the action and why it asks, and is a group, never a dialog', () => {
    const html = renderToStaticMarkup(
      <ConfirmCard
        action="press send"
        app="mail"
        rules={['label:send', 'app:messaging_submit']}
        onApprove={noop}
        onDeny={noop}
      />,
    );
    expect(html).toContain('role="group"');
    expect(html).not.toContain('dialog');
    expect(html).toContain('press send · mail');
    expect(html).toContain('it says &quot;send&quot;');
    expect(html).toContain('ctrl-confirm-approve');
    expect(html).toContain('ctrl-confirm-deny');
    expect(html).not.toContain('remember');
  });

  it('rule ids read as lowercase copy', () => {
    expect(ruleLabel('label:send')).toBe('it says "send"');
    expect(ruleLabel('label:mixed_script')).toBe('a label mixing alphabets');
    expect(ruleLabel('app:system_settings')).toBe(
      'it changes a system setting',
    );
    expect(ruleLabel('key:quit')).toBe('the shortcut quits');
  });

  it('the stop bar says esc stops it and offers a stop button', () => {
    const html = renderToStaticMarkup(<StopBar onStop={noop} />);
    expect(html).toContain('press esc');
    expect(html).toContain('ctrl-stop');
    expect(html).not.toMatch(/>[^<]*[A-Z][^<]*</);
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
