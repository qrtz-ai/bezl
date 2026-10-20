import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  Button,
  Input,
  ModelRequired,
  ModelRequirement,
  Page,
  SettingsSection,
  Stack,
  Text,
  useTheme,
  type ModelRequiredProps,
  type ModelRequirementProps,
} from '../src';
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
  ['unavailable', { ...decision, state: 'unavailable' }],
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
/** Stand-in app body: what shows blurred and inert behind the banner. */
function FakeBody() {
  const t = useTheme();
  return (
    <>
      <Stack dir="row" gap={2}>
        <Input aria-label="do" placeholder="do…" />
        <Button>do</Button>
      </Stack>
      <SettingsSection title="history">
        {['open safari', 'reply to sam', 'rename the q3 folder'].map((x) => (
          <div
            key={x}
            style={{
              padding: t.space[2],
              borderBottom: `1px solid ${t.color.border}`,
            }}
          >
            <Text>{x}</Text>
          </div>
        ))}
      </SettingsSection>
    </>
  );
}

const REQUIRED: [string, ModelRequiredProps][] = [
  ['missing', { ...speech, app: 'echo', state: 'missing', onDownload: noop }],
  [
    'downloading',
    {
      ...speech,
      app: 'echo',
      state: 'downloading',
      progress: { done: 504_000_000, total: 1_200_000_000 },
      onCancel: noop,
    },
  ],
  [
    'error',
    {
      ...speech,
      app: 'nano',
      state: 'error',
      error: "the file didn't match its fingerprint",
      onRetry: noop,
    },
  ],
  ['unavailable', { ...decision, app: 'ctrl', state: 'unavailable' }],
];

function RequiredPage({ p }: { p: ModelRequiredProps }) {
  return (
    <Page title={p.app}>
      <ModelRequired {...p}>
        <FakeBody />
      </ModelRequired>
    </Page>
  );
}

export const RequiredEvery: S = {
  name: 'required · every state',
  render: () => (
    <>
      {REQUIRED.map(([label, p]) => (
        <Row key={label} label={label}>
          <Width w={720}>
            <RequiredPage p={p} />
          </Width>
        </Row>
      ))}
    </>
  ),
};
export const RequiredMissing: S = {
  name: 'required · missing',
  render: () => <RequiredPage p={REQUIRED[0]![1]} />,
};
export const RequiredDownloading: S = {
  name: 'required · downloading',
  render: () => <RequiredPage p={REQUIRED[1]![1]} />,
};
export const RequiredError: S = {
  name: 'required · error',
  render: () => <RequiredPage p={REQUIRED[2]![1]} />,
};
export const RequiredUnavailable: S = {
  name: 'required · unavailable',
  render: () => <RequiredPage p={REQUIRED[3]![1]} />,
};
export const RequiredReady: S = {
  name: 'required · ready (no gate)',
  render: () => <RequiredPage p={{ ...speech, app: 'echo', state: 'ready' }} />,
};
