import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import {
  ActionMenu,
  AudioPlayer,
  Banner,
  Button,
  CaptureNotice,
  CaptureStatus,
  CommandPalette,
  Composer,
  ConfirmDialog,
  IconButton,
  LevelMeter,
  MiniHudFrame,
  NoDrag,
  NotesApp,
  NotesEditor,
  PermissionList,
  PipelineSteps,
  RecordDot,
  RecordingBar,
  StatusLines,
  StreamingTranscript,
  SummaryBlock,
  Text,
  TranscriptList,
  TranscriptRow,
  type NoteItem,
} from '../src';
import { Frame, Row, Width, noop } from './_util';

const meta: Meta = { title: 'components/nano' };
export default meta;
type S = StoryObj;

export const Summary: S = {
  name: 'summary block',
  render: () => (
    <SummaryBlock
      summary="Agreed to ship the beta on Friday; pricing moves to next week."
      actionItems={[
        { text: 'Send recap', owner: 'Jared' },
        { text: 'Draft pricing page' },
      ]}
      qa={[{ question: 'When is the beta?', answer: 'Friday.' }]}
      source="on-device"
    />
  ),
};

function NotesDemo() {
  const [notes, setNotes] = useState<NoteItem[]>([
    { id: 'a', time: '00:41', text: 'Ship the beta Friday' },
    { id: 'b', time: '03:12', text: 'Ask Grace about pricing' },
  ]);
  return (
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
  );
}
export const Notes: S = { name: 'notes editor', render: () => <NotesDemo /> };

export const Transcript: S = {
  name: 'transcript',
  render: () => (
    <>
      <TranscriptList>
        <TranscriptRow
          time="00:41"
          speaker="Grace"
          text="Let's ship the beta Friday."
          active
          onSeek={noop}
        />
        <TranscriptRow
          time="00:47"
          speaker="Jared"
          text="Works for me."
          active={false}
          onSeek={noop}
        />
      </TranscriptList>
      <Row label="streaming (live)">
        <div style={{ width: '100%' }}>
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
          />
        </div>
      </Row>
      <Row label="audio player">
        <Width w={480}>
          <AudioPlayer src="" onTime={noop} />
        </Width>
      </Row>
    </>
  ),
};

export const Hud: S = {
  name: 'recording hud',
  render: () => (
    <div style={{ display: 'grid', gap: 16 }}>
      <Banner actions={<Button size="sm">record</Button>}>
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
        onPause={noop}
        onStop={noop}
      />
      <Row label="record dot, level meter">
        <RecordDot />
        {[0, 0.4, 0.9].map((l) => (
          <LevelMeter key={l} level={l} />
        ))}
      </Row>
      <Frame height={56}>
        <MiniHudFrame
          strip={
            <>
              <RecordDot />
              <StatusLines top="Grace · recording" bottom="Ship it Friday." />
              <NoDrag>
                <Text variant="mono">12:04</Text>
                <IconButton
                  icon="stop"
                  tone="record"
                  label="stop recording"
                  onClick={noop}
                />
              </NoDrag>
            </>
          }
        />
      </Frame>
    </div>
  ),
};

export const Capture: S = {
  name: 'capture status, pipeline',
  render: () => (
    <div style={{ display: 'grid', gap: 16 }}>
      <CaptureStatus
        degraded={[]}
        silent={[{ source: 'mic', since: '10:04' }]}
      />
      <CaptureStatus degraded={[]} noAudio />
      <CaptureStatus
        reconnecting={{ attempt: 2, secondsLeft: 12 }}
        degraded={[{ id: 'mic-only', label: 'mic only' }]}
        error="microphone permission denied"
        onDismiss={noop}
      />
      <CaptureNotice
        items={[
          { id: 's', text: 'no audio from mic 10:04–10:09' },
          { id: 'f', text: 'meeting audio tap failed, used screen capture' },
        ]}
      />
      <PipelineSteps
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
            error: 'llm http 429',
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
        onRetry={noop}
      />
    </div>
  ),
};

function InteractDemo() {
  const [ask, setAsk] = useState('');
  const [confirm, setConfirm] = useState(false);
  const [palette, setPalette] = useState(false);
  const [q, setQ] = useState('');
  const [i, setI] = useState(0);
  return (
    <>
      <Row label="composer">
        <div style={{ width: '100%' }}>
          <Composer
            value={ask}
            onChange={setAsk}
            onSubmit={() => setAsk('')}
            placeholder="ask"
            label="ask about this meeting"
          />
        </div>
      </Row>
      <Row label="action menu, confirm, palette">
        <ActionMenu
          label="more actions"
          items={[
            { label: 'export .md', onSelect: noop },
            { label: 'delete', onSelect: noop, danger: true },
          ]}
        />
        <Button variant="secondary" onClick={() => setConfirm(true)}>
          confirm
        </Button>
        <Button variant="secondary" onClick={() => setPalette(true)}>
          palette
        </Button>
      </Row>
      <Row label="permission list">
        <div style={{ width: '100%' }}>
          <PermissionList
            items={[
              { id: 'mic', label: 'microphone', state: 'Granted' },
              { id: 'ax', label: 'accessibility', state: '', onGrant: noop },
            ]}
          />
        </div>
      </Row>
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
        query={q}
        onQuery={setQ}
        results={[
          { id: 'rec', title: 'start recording', hint: '⌘⇧R' },
          { id: 'opts', title: 'go to opts' },
        ].filter((r) => r.title.includes(q.toLowerCase()))}
        index={i}
        onIndex={setI}
        onPick={() => setPalette(false)}
      />
    </>
  );
}
export const Interact: S = {
  name: 'composer, menus, palette, permissions',
  render: () => <InteractDemo />,
};

function NotesAppDemo({ empty }: { empty?: boolean }) {
  const [sel, setSel] = useState<string | null>(empty ? null : 'a');
  const [draft, setDraft] = useState('Ship the beta Friday\n\nthen pricing.');
  const [q, setQ] = useState('');
  return (
    <Frame height={360}>
      <NotesApp
        notes={
          empty
            ? []
            : [
                { id: 'a', title: 'Ship the beta Friday', meta: 'oct 19' },
                { id: 'b', title: 'Pricing ideas', meta: 'oct 18' },
              ]
        }
        selectedId={sel}
        draft={draft}
        query={q}
        status="saved"
        onQuery={setQ}
        onSelect={setSel}
        onNew={() => setSel('new')}
        onDraft={setDraft}
        onDelete={() => setSel(null)}
      />
    </Frame>
  );
}
export const NotesAppStory: S = {
  name: 'notes app',
  render: () => <NotesAppDemo />,
};
export const NotesAppEmpty: S = {
  name: 'notes app (empty)',
  render: () => <NotesAppDemo empty />,
};
