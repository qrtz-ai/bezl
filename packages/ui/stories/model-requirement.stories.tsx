import type { Meta, StoryObj } from '@storybook/react-vite';
import { ModelRequirement, type ModelRequirementProps } from '../src';
import { Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/model requirement' };
export default meta;
type S = StoryObj;

// Fixture sizes stand in for the real `sizeBytes` each app reads from its backend.
const speech = {
  name: 'speech model',
  purpose: 'turns what you say into text, on this mac',
  sizeBytes: 1_200_000_000,
};
const decision = {
  name: 'decision model',
  purpose: "picks ctrl's next step, on this mac",
  sizeBytes: 484_000_000,
  fallback: 'ctrl decides with its built-in rules and never goes online',
};

const STATES: [string, ModelRequirementProps][] = [
  ['missing', { ...speech, state: 'missing', onDownload: noop }],
  [
    'downloading',
    {
      ...speech,
      state: 'downloading',
      progress: { done: 504_000_000, total: 1_200_000_000 },
      onCancel: noop,
    },
  ],
  [
    'downloading · checking',
    {
      ...speech,
      state: 'downloading',
      progress: { done: 0, total: 0, phase: 'checking' },
      onCancel: noop,
    },
  ],
  ['ready', { ...speech, state: 'ready', onRemove: noop }],
  [
    'error',
    {
      ...speech,
      state: 'error',
      error: "the file didn't match its fingerprint",
      onRetry: noop,
    },
  ],
  [
    'optional · fallback',
    {
      ...decision,
      state: 'missing',
      note: 'deciding with the built-in rules',
      onDownload: noop,
    },
  ],
  ['unavailable', { ...decision, state: 'unavailable', onRemove: noop }],
];

function All({ layout, w }: { layout: 'card' | 'row'; w: number }) {
  return (
    <>
      {STATES.map(([label, p]) => (
        <Row key={label} label={label}>
          <Width w={w}>
            <ModelRequirement {...p} layout={layout} />
          </Width>
        </Row>
      ))}
    </>
  );
}

export const Card: S = {
  name: 'card · every state',
  render: () => <All layout="card" w={480} />,
};
export const CompactRow: S = {
  name: 'row · every state',
  render: () => <All layout="row" w={560} />,
};
export const Missing: S = {
  render: () => (
    <Width w={480}>
      <ModelRequirement {...STATES[0]![1]} />
    </Width>
  ),
};
export const Downloading: S = {
  render: () => (
    <Width w={480}>
      <ModelRequirement {...STATES[1]![1]} />
    </Width>
  ),
};
export const Ready: S = {
  render: () => (
    <Width w={480}>
      <ModelRequirement {...STATES[3]![1]} />
    </Width>
  ),
};
export const Error: S = {
  render: () => (
    <Width w={480}>
      <ModelRequirement {...STATES[4]![1]} />
    </Width>
  ),
};
export const Optional: S = {
  name: 'optional · fallback',
  render: () => (
    <Width w={480}>
      <ModelRequirement {...STATES[5]![1]} />
    </Width>
  ),
};
