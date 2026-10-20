import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  AskCard,
  ConfirmCard,
  DecisionList,
  CtrlModelRequired,
  DecisionModelCard,
  RunHistory,
  RunTimeline,
  StopBar,
  Text,
  type DecisionModelInfo,
} from '../src';
import { Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/ctrl' };
export default meta;
type S = StoryObj;

function RunsDemo() {
  const [sel, setSel] = useState<string | null>('r1');
  const [at, setAt] = useState(2);
  return (
    <div style={{ display: 'grid', gap: 24 }}>
      <RunHistory
        runs={[
          {
            id: 'r1',
            status: 'done',
            tone: 'success',
            meta: 'turn on night shift · 10:42',
          },
          {
            id: 'r2',
            status: 'pending intent',
            tone: 'warning',
            meta: 'open the q3 doc · 10:40',
          },
          {
            id: 'r3',
            status: 'stopped',
            tone: 'danger',
            meta: 'send the recap · 10:31',
          },
        ]}
        selected={sel}
        onSelect={setSel}
      />
      <RunTimeline
        events={[
          { seq: 1, label: 'read the screen', meta: '12 ms' },
          { seq: 2, label: 'open system settings' },
          { seq: 3, label: 'turn on night shift' },
        ]}
        at={at}
        onAt={setAt}
        summary={<Text muted>step {at}</Text>}
      />
    </div>
  );
}
export const Runs: S = {
  name: 'run history, timeline',
  render: () => <RunsDemo />,
};

export const Decisions: S = {
  name: 'decision list',
  render: () => (
    <DecisionList
      decisions={[
        {
          id: 'd1',
          question: 'what kind of task',
          outcome: 'act',
          meta: '93% · qrtz-heuristic 1',
          options: [
            { id: 'a', label: 'change a setting', p: 0.93, chosen: true },
            { id: 'b', label: 'open an app', p: 0.05 },
          ],
        },
        {
          id: 'd2',
          question: 'which app',
          outcome: 'tentative',
          meta: '61% · laya',
          options: [
            { id: 'a', label: 'system settings', p: 0.61, chosen: true },
            { id: 'b', label: 'control center', p: 0.3 },
          ],
        },
        {
          id: 'd3',
          question: 'which button',
          outcome: 'ask',
          answered: true,
          meta: '40% · laya',
          options: [
            { id: 'a', label: 'send', p: 0.4, irreversible: true },
            { id: 'b', label: 'save draft', p: 0.38 },
          ],
        },
      ]}
    />
  ),
};

export const Cards: S = {
  name: 'ask, confirm, stop bar',
  render: () => (
    <Width w={560}>
      <div style={{ display: 'grid', gap: 16 }}>
        <AskCard
          question="which doc?"
          options={[
            { id: 'a', label: 'q3 plan' },
            { id: 'b', label: 'q3 review' },
          ]}
          onPick={noop}
          onCancel={noop}
        />
        <ConfirmCard
          action="click send"
          app="mail"
          rules={['label:send', 'app:submit']}
          onApprove={noop}
          onDeny={noop}
        />
        <StopBar onStop={noop} />
      </div>
    </Width>
  ),
};

const model = (m: Partial<DecisionModelInfo>): DecisionModelInfo => ({
  consented: true,
  state: 'installed',
  sizeBytes: 820_000_000,
  scorer: 'laya',
  error: null,
  ...m,
});
export const Model: S = {
  name: 'decision model · required',
  render: () => (
    <>
      {(
        [
          ['not asked', model({ consented: false, state: 'not_installed' })],
          ['downloading', model({ state: 'downloading' })],
          ['failed', model({ state: 'failed', error: 'download failed' })],
          ['unavailable', model({ state: 'unavailable' })],
          ['installed', model({})],
        ] as const
      ).map(([label, m]) => (
        <Row key={label} label={label}>
          <Width w={640}>
            <CtrlModelRequired model={m} onConsent={noop}>
              <Text>ctrl&apos;s page</Text>
              {m.state === 'installed' ? (
                <DecisionModelCard model={m} onConsent={noop} />
              ) : null}
            </CtrlModelRequired>
          </Width>
        </Row>
      ))}
    </>
  ),
};
