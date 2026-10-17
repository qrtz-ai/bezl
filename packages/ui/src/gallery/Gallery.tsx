import { lowerTitle } from '../case';
import { useState, type ReactNode } from 'react';
import { useTheme, ThemeScope } from '../theme';
import { TYPE_VARIANTS, transition, type ThemeMode } from '../tokens';
import { Text, Label, Code, Link } from '../Text';
import { Heading } from '../Heading';
import { Button } from '../Button';
import { Badge } from '../Badge';
import { Kbd } from '../Kbd';
import {
  DictionaryTerm,
  LevelBars,
  PlainList,
  TalkHistoryRow,
  TalkPill,
  TermList,
} from '../Talk';
import { Spinner } from '../Spinner';
import { Progress } from '../Progress';
import { Field, Input, Select } from '../Input';
import { Dialog, Switch, TextArea, Tooltip, useToast } from '../Overlay';
import { Tabs } from '../Tabs';
import { Callout } from '../Callout';
import { Card } from '../Card';
import { EmptyState } from '../EmptyState';
import { Icon, ICON_NAMES, type IconSize } from '../Icon';
import {
  CaptureNotice,
  CaptureStatus,
  PipelineSteps,
  StreamingTranscript,
} from '../Lifecycle';
import {
  AppFrame,
  Chip,
  IconButton,
  MeetingRow,
  SettingRow,
  SettingsSection,
  SplitView,
  Stack,
  TabBar,
  Toolbar,
} from '../Layout';
import {
  ActionMenu,
  Banner,
  CommandPalette,
  Composer,
  ConfirmDialog,
  LevelMeter,
  MiniHudFrame,
  NoDrag,
  NotesEditor,
  PermissionList,
  RecordDot,
  RecordingBar,
  StatusLines,
  SummaryBlock,
  TranscriptList,
  TranscriptRow,
  type NoteItem,
} from '../Meeting';
import { SpeechNotice, SpeechSetupPanel, type SpeechSetupView } from '../Setup';

const speechView = (
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
const noop = () => {};

function Section({
  id,
  title,
  children,
}: {
  id: string;
  title: string;
  children: ReactNode;
}) {
  const t = useTheme();
  return (
    <section
      data-testid={`gallery-${id}`}
      style={{ display: 'grid', gap: t.space[3] }}
    >
      <Label caps tone="muted">
        {lowerTitle(title)}
      </Label>
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: t.space[3],
          alignItems: 'center',
          padding: t.space[4],
          border: `1px solid ${t.color.border}`,
          borderRadius: t.radius.lg,
        }}
      >
        {children}
      </div>
    </section>
  );
}

/** A fixed-size frame for previewing whole-window components. */
function Frame({
  children,
  height = 220,
}: {
  children: ReactNode;
  height?: number;
}) {
  const t = useTheme();
  return (
    <div
      style={{
        width: '100%',
        height,
        position: 'relative',
        overflow: 'hidden',
        border: `1px solid ${t.color.border}`,
        borderRadius: t.radius.md,
        transform: 'translateZ(0)',
      }}
    >
      {children}
    </div>
  );
}

/** Dev-only gallery of every @qrtz/ui component, in either palette. */
export function ComponentGallery() {
  const [mode, setMode] = useState<ThemeMode>('dark');
  return (
    <ThemeScope mode={mode} style={{ borderRadius: 8 }}>
      <Showcase mode={mode} setMode={setMode} />
    </ThemeScope>
  );
}

const NOTES: NoteItem[] = [
  { id: 'a', time: '00:41', text: 'Ship the beta Friday' },
  { id: 'b', time: '03:12', text: 'Ask Grace about pricing' },
];

function Showcase({
  mode,
  setMode,
}: {
  mode: ThemeMode;
  setMode: (m: ThemeMode) => void;
}) {
  const t = useTheme();
  const [moved, setMoved] = useState(false);
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [palette, setPalette] = useState(false);
  const [pq, setPq] = useState('');
  const [pi, setPi] = useState(0);
  const [on, setOn] = useState(true);
  const [tab, setTab] = useState<'one' | 'two' | 'three'>('one');
  const [bar, setBar] = useState('note');
  const [sel, setSel] = useState(0);
  const [notes, setNotes] = useState(NOTES);
  const [ask, setAsk] = useState('');
  const [chip, setChip] = useState(false);
  const [page, setPage] = useState<'components' | 'icons'>('components');
  const meetings = ['Weekly Product Sync', 'Design review', '1:1 with Grace'];
  return (
    <div
      style={{
        display: 'grid',
        gap: t.space[8],
        maxWidth: 880,
        margin: '0 auto',
        padding: `40px ${t.space[6]} ${t.space[8]}`,
      }}
    >
      <header style={{ display: 'grid', gap: t.space[2] }}>
        <Heading cursor>gallery</Heading>
        <div data-testid="gallery-theme">
          <Tabs
            tabs={[
              { id: 'dark', label: 'dark' },
              { id: 'light', label: 'light' },
            ]}
            value={mode}
            onChange={setMode}
          />
        </div>
        <div data-testid="gallery-page">
          <Tabs
            tabs={[
              { id: 'components', label: 'components' },
              { id: 'icons', label: 'icons' },
            ]}
            value={page}
            onChange={setPage}
          />
        </div>
      </header>

      {page === 'icons' ? (
        <IconsPage />
      ) : (
        <>
          <Section id="type" title="typography">
            <div
              style={{ display: 'grid', gap: t.space[3], width: '100%' }}
              data-testid="gallery-type-scale"
            >
              {TYPE_VARIANTS.map((v) => (
                <div
                  key={v}
                  data-testid="type-sample"
                  style={{
                    display: 'grid',
                    gridTemplateColumns: '96px 1fr 180px',
                    gap: t.space[4],
                    alignItems: 'baseline',
                  }}
                >
                  <Code>{v}</Code>
                  <Text variant={v} truncate>
                    {v === 'code' || v === 'mono'
                      ? '00:12:34 · note.record()'
                      : 'meeting notes'}{' '}
                    // check-titles-ignore (sample user content)
                  </Text>
                  <Text variant="caption" muted>
                    {t.type[v].size} / {t.type[v].lineHeight}
                    {t.type[v].tracking !== '0em'
                      ? ` / ${t.type[v].tracking}`
                      : ''}
                  </Text>
                </div>
              ))}
              <Stack gap={2}>
                <Heading level={1}>heading 1</Heading>
                <Heading level={2}>heading 2</Heading>
                <Heading level={3}>heading 3</Heading>
                <Label>label</Label>
                <Label caps tone="muted">
                  section label
                </Label>
                <Text>
                  {/* check-titles-ignore: sample user content */}
                  Body with <Code>code</Code> and a{' '}
                  <Link href="#type">link</Link>.
                </Text>
                <Stack dir="row" gap={4} wrap>
                  {(
                    [
                      'default',
                      'muted',
                      'success',
                      'warning',
                      'danger',
                    ] as const
                  ).map((tone) => (
                    <Text key={tone} tone={tone}>
                      {tone}
                    </Text>
                  ))}
                </Stack>
                <Code block>bun run lint</Code>
              </Stack>
            </div>
          </Section>

          <Section id="color" title="color">
            {Object.entries(t.color)
              .filter(([k]) => k !== 'accentText')
              .map(([k, v]) => (
                <div key={k} style={{ display: 'grid', gap: 4, width: 96 }}>
                  <div
                    style={{
                      height: 40,
                      borderRadius: t.radius.md,
                      background: v,
                      border: `1px solid ${t.color.border}`,
                    }}
                  />
                  <Text variant="caption" truncate>
                    {k}
                  </Text>
                </div>
              ))}
          </Section>

          <Section id="buttons" title="buttons">
            <Button>primary</Button>
            <Button variant="secondary">secondary</Button>
            <Button variant="ghost">ghost</Button>
            <Button variant="danger">danger</Button>
            <Button size="sm">small</Button>
            <Button loading>loading</Button>
            <Button disabled>disabled</Button>
            <Button size="sm" icon="circle">
              record
            </Button>
            <IconButton icon="search" label="search" onClick={() => {}} />
            <IconButton
              icon="stop"
              label="stop"
              tone="record"
              onClick={() => {}}
            />
            <ActionMenu
              label="more actions"
              items={[
                { label: 'export .md', onSelect: () => {} },
                { label: 'delete', onSelect: () => {}, danger: true },
              ]}
            />
            <Chip pressed={chip} onClick={() => setChip(!chip)}>
              {/* check-titles-ignore: sample user content */}
              Grace
            </Chip>
          </Section>

          <Section id="badges" title="badges, kbd, spinner, progress, tooltip">
            {(
              ['neutral', 'primary', 'success', 'warning', 'danger'] as const
            ).map((tone) => (
              <Badge key={tone} tone={tone}>
                {tone}
              </Badge>
            ))}
            <span>
              <Kbd>⌘</Kbd> <Kbd>K</Kbd>
            </span>
            <Spinner />
            <div style={{ width: 160 }}>
              <Progress value={0.6} label="example progress" />
            </div>
            <Tooltip label="start recording (⌘⇧r)">
              <Button variant="ghost" size="sm">
                hover me
              </Button>
            </Tooltip>
          </Section>

          <Section id="speech-setup" title="speech setup">
            {[
              speechView({ state: 'idle', done: 0, total: 0 }),
              speechView({
                state: 'downloading',
                done: 160_000_000,
                total: 412_000_000,
              }),
              speechView({
                state: 'error',
                done: 0,
                total: 0,
                error: "couldn't reach the download server",
              }),
              speechView({ state: 'idle', done: 0, total: 0 }, '1.0.0'),
            ].map((v, i) => (
              <div key={i} style={{ width: 320 }}>
                <SpeechSetupPanel
                  status={v}
                  manage
                  onDownload={noop}
                  onPause={noop}
                  onResume={noop}
                  onDelete={noop}
                  data-testid={`gallery-speech-${i}`}
                />
              </div>
            ))}
            <div style={{ width: 320 }}>
              <SpeechNotice
                status={speechView({
                  state: 'paused',
                  done: 100_000_000,
                  total: 412_000_000,
                })}
                onOpenSettings={noop}
              />
            </div>
          </Section>

          <Section id="inputs" title="inputs">
            <div
              style={{
                display: 'grid',
                gap: t.space[3],
                width: '100%',
                gridTemplateColumns: '1fr 1fr',
              }}
            >
              <Field label="input">
                <Input placeholder="type here" />
              </Field>
              <Field label="select">
                <Select defaultValue="b">
                  <option value="a">alpha</option>
                  <option value="b">beta</option>
                </Select>
              </Field>
              <Field label="textarea">
                <TextArea placeholder="notes" />
              </Field>
              <Field label="switch">
                <Switch
                  checked={on}
                  onChange={setOn}
                  label={on ? 'on' : 'off'}
                  showLabel
                />
              </Field>
              <Composer
                value={ask}
                onChange={setAsk}
                onSubmit={() => setAsk('')}
                placeholder="ask"
                label="gallery composer"
              />
            </div>
          </Section>

          <Section id="shell" title="app frame and tab bar">
            <Frame height={140}>
              <AppFrame
                embedded
                tabs={
                  <TabBar
                    label="example tabs"
                    testIdPrefix="gallery-tab"
                    items={[
                      { id: 'note', label: 'note' },
                      { id: 'settings', label: 'settings' },
                    ]}
                    active={bar}
                    onSelect={setBar}
                  />
                }
              >
                <Stack pad={4}>
                  <Text muted>{bar}</Text>
                </Stack>
              </AppFrame>
            </Frame>
          </Section>

          <Section id="split" title="split view, meeting rows, toolbar">
            <Frame height={260}>
              <SplitView
                sidebar={
                  <>
                    <Toolbar
                      end={
                        <IconButton
                          icon="search"
                          label="search"
                          onClick={() => {}}
                        />
                      }
                    >
                      <Button size="sm" icon="circle">
                        record
                      </Button>
                    </Toolbar>
                    <Stack as="ul" gap={0} aria-label="example meetings">
                      {meetings.map((m, i) => (
                        <MeetingRow
                          key={m}
                          title={m}
                          date={`Oct ${12 + i}`}
                          selected={sel === i}
                          onSelect={() => setSel(i)}
                        />
                      ))}
                    </Stack>
                  </>
                }
              >
                <Heading level={2} preserveCase>
                  {meetings[sel]}
                </Heading>
              </SplitView>
            </Frame>
          </Section>

          <Section id="nav" title="tabs">
            <Tabs
              tabs={[
                { id: 'one', label: 'summary' },
                { id: 'two', label: 'notes' },
                { id: 'three', label: 'transcript' },
              ]}
              value={tab}
              onChange={setTab}
            />
          </Section>

          <Section id="meeting" title="summary, notes, transcript">
            <div style={{ display: 'grid', gap: t.space[6], width: '100%' }}>
              <SummaryBlock
                summary="Agreed to ship the beta on Friday; pricing moves to next week."
                actionItems={[
                  { text: 'Send recap', owner: 'Jared' },
                  { text: 'Draft pricing page' },
                ]}
                qa={[{ question: 'When is the beta?', answer: 'Friday.' }]}
                source="openrouter"
              />
              <NotesEditor
                notes={notes}
                onAdd={(text) =>
                  setNotes((n) => [
                    ...n,
                    { id: String(n.length + 1), time: '05:00', text },
                  ])
                }
                onUpdate={(id, text) =>
                  setNotes((n) =>
                    text
                      ? n.map((x) => (x.id === id ? { ...x, text } : x))
                      : n.filter((x) => x.id !== id),
                  )
                }
              />
              <TranscriptList>
                <TranscriptRow
                  time="00:41"
                  speaker="Grace"
                  text="Let's ship the beta Friday."
                  active
                  onSeek={() => {}}
                />
                <TranscriptRow
                  time="00:47"
                  speaker="Jared"
                  text="Works for me."
                  active={false}
                  onSeek={() => {}}
                />
              </TranscriptList>
            </div>
          </Section>

          <Section id="hud" title="recording hud">
            <div style={{ display: 'grid', gap: t.space[4], width: '100%' }}>
              <Banner actions={<Button size="sm">record</Button>}>
                {/* check-titles-ignore: sample user content */}
                <Text>Weekly Product Sync</Text>
                <Text variant="caption" muted>
                  {/* check-titles-ignore: sample user content */}
                  Zoom
                </Text>
              </Banner>
              <RecordingBar
                clock="12:04"
                // check-titles-ignore: sample user content
                title="Weekly Product Sync"
                level={0.6}
                speaker="Grace"
                onPause={() => {}}
                onStop={() => {}}
              />
              <StreamingTranscript
                live
                segments={[
                  {
                    id: '1',
                    startMs: 41000,
                    speaker: 'Grace',
                    text: "Let's ship the beta Friday.",
                  },
                  {
                    id: '2',
                    startMs: 47000,
                    speaker: 'Jared',
                    text: 'Works for me.',
                  },
                ]}
                partial={{ id: 'p', text: 'and pricing next' }}
                maxHeight={160}
                testId="gallery-live-transcript"
              />
              <CaptureStatus
                degraded={[]}
                silent={[{ source: 'mic', since: '10:04' }]}
              />
              <CaptureStatus degraded={[]} noAudio />
              <CaptureNotice
                items={[
                  { id: 's', text: 'no audio from mic 10:04–10:09' },
                  {
                    id: 'f',
                    text: 'meeting audio tap failed, used screen capture',
                  },
                ]}
              />
              <CaptureStatus
                reconnecting={{ attempt: 2, secondsLeft: 12 }}
                degraded={[{ id: 'mic-only', label: 'mic only' }]}
                error="Microphone permission denied"
                onDismiss={() => {}}
              />
              <PipelineSteps
                testId="gallery-pipeline"
                steps={[
                  {
                    id: 'finalize',
                    label: 'transcript',
                    status: 'done',
                    attempts: 1,
                    maxAttempts: 3,
                  },
                  {
                    id: 'summary',
                    label: 'summary',
                    status: 'failed',
                    attempts: 3,
                    maxAttempts: 3,
                    error: 'LLM HTTP 429',
                    fallback: true,
                  },
                  {
                    id: 'index',
                    label: 'search index',
                    status: 'retrying',
                    attempts: 1,
                    maxAttempts: 3,
                  },
                ]}
                onRetry={() => {}}
              />
              <Stack dir="row" gap={3} align="center">
                <RecordDot />
                <LevelMeter level={0.4} />
              </Stack>
              <Frame height={56}>
                <MiniHudFrame
                  strip={
                    <>
                      <RecordDot />
                      <StatusLines
                        top="Grace · Recording"
                        bottom="Ship it Friday."
                      />
                      <NoDrag>
                        <Text variant="mono">12:04</Text>
                        <IconButton
                          icon="stop"
                          tone="record"
                          label="stop recording"
                          onClick={() => {}}
                        />
                      </NoDrag>
                    </>
                  }
                />
              </Frame>
            </div>
          </Section>

          <Section id="talk" title="talk: pill, history, dictionary">
            <div style={{ display: 'grid', gap: t.space[4], width: '100%' }}>
              <LevelBars level={0.6} />
              <TalkPill
                inline
                phase="listening"
                level={0.5}
                text="So I think we should"
                partial="ship qrtz"
              />
              <TalkPill
                inline
                phase="finalizing"
                level={0}
                text="So I think we should ship qrtz talk"
                partial=""
              />
              <TalkPill
                inline
                phase="error"
                level={0}
                text=""
                partial=""
                error="Microphone did not start"
              />
              <PlainList label="history">
                <TalkHistoryRow
                  text="I think we should ship qrtz talk on Friday."
                  meta="10:42 PM · Notes · elevenlabs · 312 ms"
                  onCopy={() => {}}
                  onDelete={() => {}}
                />
              </PlainList>
              <TermList>
                <DictionaryTerm
                  term="qrtz"
                  aliases={['cue arts']}
                  onRemove={() => {}}
                />
              </TermList>
            </div>
          </Section>

          <Section id="settings" title="settings rows, permissions">
            <div style={{ display: 'grid', gap: t.space[4], width: '100%' }}>
              <SettingsSection title="recording">
                <SettingRow label="record automatically">
                  <Switch
                    checked={on}
                    onChange={setOn}
                    label="record automatically"
                  />
                </SettingRow>
              </SettingsSection>
              <PermissionList
                items={[
                  { id: 'mic', label: 'microphone', state: 'Granted' },
                  {
                    id: 'ax',
                    label: 'accessibility',
                    state: '',
                    onGrant: () => {},
                  },
                ]}
              />
            </div>
          </Section>

          <Section id="overlays" title="dialog, confirm, palette, toast">
            <Button
              variant="secondary"
              onClick={() => setOpen(true)}
              data-testid="gallery-open-dialog"
            >
              open dialog
            </Button>
            <Button variant="secondary" onClick={() => setConfirm(true)}>
              confirm
            </Button>
            <Button variant="secondary" onClick={() => setPalette(true)}>
              palette
            </Button>
            <Button
              variant="secondary"
              onClick={() => toast('saved to downloads', 'success')}
              data-testid="gallery-toast"
            >
              show toast
            </Button>
            <Dialog
              open={open}
              onClose={() => setOpen(false)}
              title="delete meeting?"
            >
              <div
                style={{
                  padding: t.space[5],
                  display: 'flex',
                  gap: t.space[2],
                  justifyContent: 'flex-end',
                }}
              >
                <Button variant="ghost" onClick={() => setOpen(false)}>
                  cancel
                </Button>
                <Button variant="danger" onClick={() => setOpen(false)}>
                  delete
                </Button>
              </div>
            </Dialog>
            <ConfirmDialog
              open={confirm}
              title="delete this meeting?"
              body="Weekly Product Sync"
              confirmLabel="delete"
              onCancel={() => setConfirm(false)}
              onConfirm={() => setConfirm(false)}
            />
            <CommandPalette
              open={palette}
              onClose={() => setPalette(false)}
              query={pq}
              onQuery={setPq}
              results={[
                { id: 'rec', title: 'start recording', hint: '⌘⇧R' },
                { id: 'set', title: 'go to settings' },
              ].filter((r) => r.title.toLowerCase().includes(pq.toLowerCase()))}
              index={pi}
              onIndex={setPi}
              onPick={() => setPalette(false)}
            />
          </Section>

          <Section id="callouts" title="callout">
            <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
              {(['info', 'warning', 'danger'] as const).map((tone) => (
                <Callout
                  key={tone}
                  tone={tone}
                  title={`${tone[0]!.toUpperCase()}${tone.slice(1)}`}
                  action={
                    <Button size="sm" variant="secondary">
                      action
                    </Button>
                  }
                />
              ))}
            </div>
          </Section>

          <Section id="cards" title="card">
            <Card padding={3}>
              <Text>padding 3</Text>
            </Card>
            <Card padding={6}>
              <Text>padding 6</Text>
            </Card>
          </Section>

          <Section id="space" title="spacing and radius">
            {Object.entries(t.space).map(([k, v]) => (
              <div
                key={k}
                style={{ display: 'grid', gap: 4, justifyItems: 'center' }}
              >
                <div
                  style={{
                    width: v,
                    height: 16,
                    minWidth: 1,
                    background: t.color.textMuted,
                    borderRadius: 2,
                  }}
                />
                <Text variant="caption" muted>
                  {k}
                </Text>
              </div>
            ))}
            {Object.entries(t.radius).map(([k, v]) => (
              <div
                key={k}
                style={{ display: 'grid', gap: 4, justifyItems: 'center' }}
              >
                <div
                  style={{
                    width: 40,
                    height: 40,
                    border: `1px solid ${t.color.borderStrong}`,
                    borderRadius: v,
                  }}
                />
                <Text variant="caption" muted>
                  {k}
                </Text>
              </div>
            ))}
          </Section>

          <Section id="motion" title="motion">
            <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
              <Stack dir="row">
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => setMoved((m) => !m)}
                  data-testid="gallery-motion"
                >
                  play
                </Button>
              </Stack>
              {(['hover', 'enter', 'exit'] as const).map((speed) => (
                <div
                  key={speed}
                  style={{ display: 'flex', gap: 12, alignItems: 'center' }}
                >
                  <Text variant="caption" muted style={{ width: 140 }}>
                    {speed} · {t.motion[speed]}
                  </Text>
                  <div style={{ flex: 1, height: 8, position: 'relative' }}>
                    <div
                      style={{
                        position: 'absolute',
                        left: moved ? 'calc(100% - 8px)' : 0,
                        width: 8,
                        height: 8,
                        borderRadius: 99,
                        background: t.color.primary,
                        transition: transition('left', speed, t),
                      }}
                    />
                  </div>
                </div>
              ))}
            </div>
          </Section>

          <Section id="focus" title="focus ring">
            <Button size="sm">button</Button>
            <Input aria-label="focus demo input" placeholder="input" />
            <Link href="#focus">link</Link>
          </Section>

          <Section id="empty" title="empty state">
            <EmptyState
              icon="mic"
              title="no meetings"
              action={
                <Button size="sm" icon="circle">
                  record
                </Button>
              }
            />
          </Section>
        </>
      )}
    </div>
  );
}

const ICON_SIZES: IconSize[] = ['xs', 'sm', 'md', 'lg', 'xl'];

/** Every Feather icon in the set, by name, plus the size scale and tones. */
function IconsPage() {
  const t = useTheme();
  return (
    <>
      <Section id="icons" title="icons (feather)">
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(120px, 1fr))',
            gap: t.space[3],
            width: '100%',
          }}
        >
          {ICON_NAMES.map((n) => (
            <div
              key={n}
              data-testid={`gallery-icon-${n}`}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: t.space[2],
                padding: t.space[2],
                border: `1px solid ${t.color.border}`,
                borderRadius: t.radius.md,
              }}
            >
              <Icon name={n} />
              <Text variant="caption" muted truncate>
                {n}
              </Text>
            </div>
          ))}
        </div>
      </Section>
      <Section id="icon-sizes" title="icon sizes">
        {ICON_SIZES.map((sz) => (
          <span
            key={sz}
            style={{ display: 'grid', justifyItems: 'center', gap: t.space[1] }}
          >
            <Icon name="mic" size={sz} />
            <Text variant="caption" muted>
              {sz} · {t.icon.size[sz]}
            </Text>
          </span>
        ))}
      </Section>
      <Section id="icon-tones" title="icon tones">
        <Icon name="info" />
        <Icon name="info" tone="muted" />
        <Icon name="check" tone="success" />
        <Icon name="alert" tone="warning" />
        <Icon name="close" tone="danger" />
        <Icon name="circle" tone="record" fill />
      </Section>
    </>
  );
}
