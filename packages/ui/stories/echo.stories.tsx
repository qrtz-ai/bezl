import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  ContextMenu,
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

const HELPER_FAILED =
  "the capture helper isn't running, so the mic can't open (echo: signature mismatch: the helper isn't signed with qrtz's Developer ID identity)";

/** QRT-291: echo's helper gave up (Failed); the pill says why, in full. */
export const HelperFailed: S = {
  name: 'helper failed: reason',
  render: () => (
    <TalkPill inline phase="error" level={0} error={HELPER_FAILED} />
  ),
};

/** QRT-291: the helper's fixed public reasons (qrtz-app installed::public_refusal), each read in full, no paths. */
const HELPER_REASONS = [
  "signature mismatch: the helper isn't signed with qrtz's Developer ID identity",
  "signing requirement mismatch: the expected requirement isn't qrtz's Developer ID one",
  'launch refused: a launch check failed (see the log)',
  "dev stub: the real helper isn't built yet (bun run build:helper)",
  "missing binary: the helper isn't where qrtz expects it",
].map(
  (r) => `the capture helper isn't running, so the mic can't open (echo: ${r})`,
);

/** QRT-291: every fixed reason, the longest included, still reads in full. */
export const HelperFailedLong: S = {
  name: 'helper failed: reasons',
  render: () => (
    <div style={{ display: 'grid', gap: 12 }}>
      {HELPER_REASONS.map((r) => (
        <TalkPill key={r} inline phase="error" level={0} error={r} />
      ))}
    </div>
  ),
};

/** QRT-291: in a narrow column the reason wraps onto more lines instead of being cut off. */
export const HelperFailedWrap: S = {
  name: 'helper failed: wrapping',
  render: () => (
    <div style={{ width: 260 }} data-testid="pill-narrow">
      <TalkPill inline phase="error" level={0} error={HELPER_FAILED} />
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
      <ContextMenu
        label="actions"
        items={[{ id: 'copy', label: 'copy', icon: 'copy', onSelect: noop }]}
      >
        <Text>right-click for the bare context menu</Text>
      </ContextMenu>
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
