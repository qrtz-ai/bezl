import { useState, type ReactNode } from 'react';
import { useTheme, ThemeScope } from '../theme';
import { TYPE_VARIANTS, transition, type ThemeMode } from '../tokens';
import { Text, Label, Code, Link } from '../Text';
import { Heading } from '../Heading';
import { Button } from '../Button';
import { Badge } from '../Badge';
import { Kbd } from '../Kbd';
import { Spinner } from '../Spinner';
import { Progress } from '../Progress';
import { Field, Input, Select } from '../Input';
import { Dialog, Switch, TextArea, Tooltip, useToast } from '../Overlay';
import { Tabs } from '../Tabs';
import { Callout } from '../Callout';
import { Card } from '../Card';
import { EmptyState } from '../EmptyState';
import {
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
  ChatAnswer,
  ChatQuestion,
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
        {title}
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
  const [bar, setBar] = useState('memo');
  const [sel, setSel] = useState(0);
  const [notes, setNotes] = useState(NOTES);
  const [ask, setAsk] = useState('');
  const [chip, setChip] = useState(false);
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
        <Heading cursor>Gallery</Heading>
        <div data-testid="gallery-theme">
          <Tabs
            tabs={[
              { id: 'dark', label: 'Dark' },
              { id: 'light', label: 'Light' },
            ]}
            value={mode}
            onChange={setMode}
          />
        </div>
      </header>

      <Section id="type" title="Typography">
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
                  ? '00:12:34 · memo.record()'
                  : 'Meeting notes'}
              </Text>
              <Text variant="caption" muted>
                {t.type[v].size} / {t.type[v].lineHeight}
                {t.type[v].tracking !== '0em' ? ` / ${t.type[v].tracking}` : ''}
              </Text>
            </div>
          ))}
          <Stack gap={2}>
            <Heading level={1}>Heading 1</Heading>
            <Heading level={2}>Heading 2</Heading>
            <Heading level={3}>Heading 3</Heading>
            <Label>Label</Label>
            <Label caps tone="muted">
              Section label
            </Label>
            <Text>
              Body with <Code>code</Code> and a <Link href="#type">link</Link>.
            </Text>
            <Stack dir="row" gap={4} wrap>
              {(
                ['default', 'muted', 'success', 'warning', 'danger'] as const
              ).map((tone) => (
                <Text key={tone} tone={tone}>
                  {tone}
                </Text>
              ))}
            </Stack>
            <Code block>npm run lint</Code>
          </Stack>
        </div>
      </Section>

      <Section id="color" title="Color">
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

      <Section id="buttons" title="Buttons">
        <Button>Primary</Button>
        <Button variant="secondary">Secondary</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="danger">Danger</Button>
        <Button size="sm">Small</Button>
        <Button loading>Loading</Button>
        <Button disabled>Disabled</Button>
        <IconButton icon="⌕" label="Search" onClick={() => {}} />
        <IconButton icon="■" label="Stop" tone="record" onClick={() => {}} />
        <ActionMenu
          label="More actions"
          items={[
            { label: 'Export .md', onSelect: () => {} },
            { label: 'Delete', onSelect: () => {}, danger: true },
          ]}
        />
        <Chip pressed={chip} onClick={() => setChip(!chip)}>
          Grace
        </Chip>
      </Section>

      <Section id="badges" title="Badges, Kbd, Spinner, Progress, Tooltip">
        {(['neutral', 'primary', 'success', 'warning', 'danger'] as const).map(
          (tone) => (
            <Badge key={tone} tone={tone}>
              {tone}
            </Badge>
          ),
        )}
        <span>
          <Kbd>⌘</Kbd> <Kbd>K</Kbd>
        </span>
        <Spinner />
        <div style={{ width: 160 }}>
          <Progress value={0.6} label="Example progress" />
        </div>
        <Tooltip label="Start recording (⌘⇧R)">
          <Button variant="ghost" size="sm">
            Hover me
          </Button>
        </Tooltip>
      </Section>

      <Section id="inputs" title="Inputs">
        <div
          style={{
            display: 'grid',
            gap: t.space[3],
            width: '100%',
            gridTemplateColumns: '1fr 1fr',
          }}
        >
          <Field label="Input">
            <Input placeholder="Type here" />
          </Field>
          <Field label="Select">
            <Select defaultValue="b">
              <option value="a">Alpha</option>
              <option value="b">Beta</option>
            </Select>
          </Field>
          <Field label="TextArea">
            <TextArea placeholder="Notes" />
          </Field>
          <Field label="Switch">
            <Switch
              checked={on}
              onChange={setOn}
              label={on ? 'On' : 'Off'}
              showLabel
            />
          </Field>
          <Composer
            value={ask}
            onChange={setAsk}
            onSubmit={() => setAsk('')}
            placeholder="Ask"
            label="Gallery composer"
          />
        </div>
      </Section>

      <Section id="shell" title="App frame and tab bar">
        <Frame height={140}>
          <AppFrame
            embedded
            tabs={
              <TabBar
                label="Example tabs"
                testIdPrefix="gallery-tab"
                items={[
                  { id: 'memo', label: 'Memo' },
                  { id: 'settings', label: 'Settings' },
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

      <Section id="split" title="Split view, meeting rows, toolbar">
        <Frame height={260}>
          <SplitView
            sidebar={
              <>
                <Toolbar
                  end={
                    <IconButton icon="⌕" label="Search" onClick={() => {}} />
                  }
                >
                  <Button size="sm">● Record</Button>
                </Toolbar>
                <Stack as="ul" gap={0} aria-label="Example meetings">
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
            <Heading level={2}>{meetings[sel]}</Heading>
          </SplitView>
        </Frame>
      </Section>

      <Section id="nav" title="Tabs">
        <Tabs
          tabs={[
            { id: 'one', label: 'Summary' },
            { id: 'two', label: 'Notes' },
            { id: 'three', label: 'Transcript' },
          ]}
          value={tab}
          onChange={setTab}
        />
      </Section>

      <Section id="meeting" title="Summary, notes, transcript">
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

      <Section id="chat" title="Chat">
        <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
          <ChatQuestion>When is the beta?</ChatQuestion>
          <ChatAnswer
            text="Friday, per Grace."
            sources={[
              {
                label: '[1] 00:41 · Grace',
                title: "Let's ship the beta Friday.",
                onOpen: () => {},
              },
            ]}
            footer="openrouter"
          />
        </div>
      </Section>

      <Section id="hud" title="Recording HUD">
        <div style={{ display: 'grid', gap: t.space[4], width: '100%' }}>
          <Banner actions={<Button size="sm">Record</Button>}>
            <Text>Weekly Product Sync</Text>
            <Text variant="caption" muted>
              Zoom
            </Text>
          </Banner>
          <RecordingBar
            clock="12:04"
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
            reconnecting={{ attempt: 2, secondsLeft: 12 }}
            degraded={[{ id: 'mic-only', label: 'Mic only' }]}
            error="Microphone permission denied"
            onDismiss={() => {}}
          />
          <PipelineSteps
            testId="gallery-pipeline"
            steps={[
              {
                id: 'finalize',
                label: 'Transcript',
                status: 'done',
                attempts: 1,
                maxAttempts: 3,
              },
              {
                id: 'summary',
                label: 'Summary',
                status: 'failed',
                attempts: 3,
                maxAttempts: 3,
                error: 'LLM HTTP 429',
                fallback: true,
              },
              {
                id: 'index',
                label: 'Search index',
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
                      icon="■"
                      tone="record"
                      label="Stop recording"
                      onClick={() => {}}
                    />
                  </NoDrag>
                </>
              }
            />
          </Frame>
        </div>
      </Section>

      <Section id="settings" title="Settings rows, permissions">
        <div style={{ display: 'grid', gap: t.space[4], width: '100%' }}>
          <SettingsSection title="Recording">
            <SettingRow label="Record automatically">
              <Switch
                checked={on}
                onChange={setOn}
                label="Record automatically"
              />
            </SettingRow>
          </SettingsSection>
          <PermissionList
            items={[
              { id: 'mic', label: 'Microphone', state: 'Granted' },
              {
                id: 'ax',
                label: 'Accessibility',
                state: '',
                onGrant: () => {},
              },
            ]}
          />
        </div>
      </Section>

      <Section id="overlays" title="Dialog, confirm, palette, toast">
        <Button
          variant="secondary"
          onClick={() => setOpen(true)}
          data-testid="gallery-open-dialog"
        >
          Open dialog
        </Button>
        <Button variant="secondary" onClick={() => setConfirm(true)}>
          Confirm
        </Button>
        <Button variant="secondary" onClick={() => setPalette(true)}>
          Palette
        </Button>
        <Button
          variant="secondary"
          onClick={() => toast('Saved to Downloads', 'success')}
          data-testid="gallery-toast"
        >
          Show toast
        </Button>
        <Dialog
          open={open}
          onClose={() => setOpen(false)}
          title="Delete meeting?"
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
              Cancel
            </Button>
            <Button variant="danger" onClick={() => setOpen(false)}>
              Delete
            </Button>
          </div>
        </Dialog>
        <ConfirmDialog
          open={confirm}
          title="Delete this meeting?"
          body="Weekly Product Sync"
          confirmLabel="Delete"
          onCancel={() => setConfirm(false)}
          onConfirm={() => setConfirm(false)}
        />
        <CommandPalette
          open={palette}
          onClose={() => setPalette(false)}
          query={pq}
          onQuery={setPq}
          results={[
            { id: 'rec', title: 'Start recording', hint: '⌘⇧R' },
            { id: 'set', title: 'Go to Settings' },
          ].filter((r) => r.title.toLowerCase().includes(pq.toLowerCase()))}
          index={pi}
          onIndex={setPi}
          onPick={() => setPalette(false)}
        />
      </Section>

      <Section id="callouts" title="Callout">
        <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
          {(['info', 'warning', 'danger'] as const).map((tone) => (
            <Callout
              key={tone}
              tone={tone}
              title={`${tone[0]!.toUpperCase()}${tone.slice(1)}`}
              action={
                <Button size="sm" variant="secondary">
                  Action
                </Button>
              }
            />
          ))}
        </div>
      </Section>

      <Section id="cards" title="Card">
        <Card padding={3}>
          <Text>padding 3</Text>
        </Card>
        <Card padding={6}>
          <Text>padding 6</Text>
        </Card>
      </Section>

      <Section id="space" title="Spacing and radius">
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

      <Section id="motion" title="Motion">
        <div style={{ display: 'grid', gap: t.space[3], width: '100%' }}>
          <Stack dir="row">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => setMoved((m) => !m)}
              data-testid="gallery-motion"
            >
              Play
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

      <Section id="focus" title="Focus ring">
        <Button size="sm">Button</Button>
        <Input aria-label="Focus demo input" placeholder="Input" />
        <Link href="#focus">Link</Link>
      </Section>

      <Section id="empty" title="Empty state">
        <EmptyState
          icon="◎"
          title="No meetings"
          action={<Button size="sm">● Record</Button>}
        />
      </Section>
    </div>
  );
}
