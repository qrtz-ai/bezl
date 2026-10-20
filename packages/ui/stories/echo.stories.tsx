import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  DictionaryTerm,
  ModelsPanel,
  PlainList,
  SpeechNotice,
  SpeechRequired,
  Text,
  SpeechSetupPanel,
  TalkHistoryRow,
  TalkPill,
  TermList,
  Waveform,
  type SpeechSetupView,
} from '../src';
import { Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/echo' };
export default meta;
type S = StoryObj;

export const Wave: S = {
  name: 'waveform',
  render: () => (
    <>
      <Row label="live">
        {[0, 0.3, 0.8].map((l) => (
          <Waveform key={l} level={l} />
        ))}
      </Row>
      <Row label="idle (processing)">
        <Waveform level={0} mode="idle" />
      </Row>
    </>
  ),
};

export const Pill: S = {
  name: 'talk pill',
  render: () => (
    <div style={{ display: 'grid', gap: 12 }}>
      <TalkPill inline phase="idle" level={0} />
      <TalkPill inline phase="listening" level={0.5} />
      <TalkPill inline phase="finalizing" level={0} />
      <TalkPill inline phase="inserting" level={0} />
      <TalkPill
        inline
        phase="error"
        level={0}
        error="microphone did not start"
      />
    </div>
  ),
};

export const History: S = {
  name: 'history, dictionary',
  render: () => (
    <>
      <PlainList label="history">
        <TalkHistoryRow
          text="I think we should ship qrtz echo on Friday."
          meta="10:42 PM · Notes · on-device · 312 ms"
          onCopy={noop}
          onDelete={noop}
        />
        <TalkHistoryRow
          text="Remind me to call Grace."
          meta="10:40 PM · Messages · on-device · 280 ms"
          onCopy={noop}
          onDelete={noop}
        />
      </PlainList>
      <TermList>
        <DictionaryTerm term="qrtz" aliases={['cue arts']} onRemove={noop} />
        <DictionaryTerm term="Parakeet" aliases={[]} onRemove={noop} />
      </TermList>
    </>
  ),
};

const view = (
  download: SpeechSetupView['download'],
  installed: string | null = null,
): SpeechSetupView => ({
  configured: true,
  installed,
  bytes: installed ? 412_000_000 : 0,
  latest: '1.1.0',
  latestBytes: 412_000_000,
  waiting: null,
  listExpired: false,
  download,
});
const STATES: [string, SpeechSetupView][] = [
  ['not downloaded', view({ state: 'idle', done: 0, total: 0 })],
  [
    'downloading',
    view({ state: 'downloading', done: 160_000_000, total: 412_000_000 }),
  ],
  ['paused', view({ state: 'paused', done: 100_000_000, total: 412_000_000 })],
  [
    'error',
    view({
      state: 'error',
      done: 0,
      total: 0,
      error: "couldn't reach the download server",
    }),
  ],
  ['installed', view({ state: 'idle', done: 0, total: 0 }, '1.0.0')],
];

export const Speech: S = {
  name: 'speech setup',
  render: () => (
    <>
      {STATES.map(([label, v]) => (
        <Row key={label} label={label}>
          <Width w={360}>
            <SpeechSetupPanel
              status={v}
              manage
              onDownload={noop}
              onPause={noop}
              onResume={noop}
              onDelete={noop}
            />
          </Width>
        </Row>
      ))}
      {STATES.map(([label, v]) => (
        <Row key={`required-${label}`} label={`required · ${label}`}>
          <Width w={560}>
            <SpeechRequired
              app="echo"
              status={v}
              onDownload={noop}
              onCancel={noop}
            >
              <Text>echo&apos;s page</Text>
            </SpeechRequired>
          </Width>
        </Row>
      ))}
      {STATES.map(([label, v]) => (
        <Row key={`notice-${label}`} label={`notice · ${label}`}>
          <Width w={360}>
            <SpeechNotice status={v} onOpen={noop} />
          </Width>
        </Row>
      ))}
    </>
  ),
};

export const Models: S = {
  name: 'models panel',
  render: () => (
    <Width w={560}>
      <ModelsPanel
        status={STATES[4]![1]}
        sets={[{ name: 'parakeet', keptFor: ['nano'] }, { name: 'laya' }]}
        onDownload={noop}
        onPause={noop}
        onResume={noop}
        onDelete={noop}
      />
    </Width>
  ),
};
