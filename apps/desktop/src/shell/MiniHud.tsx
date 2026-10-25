import { useEffect, useMemo, useState } from 'react';
import {
  Button,
  IconButton,
  Input,
  MiniHudFrame,
  NoDrag,
  RecordDot,
  Stack,
  StatusLines,
  Text,
  ThemeProvider,
} from '@qrtz/ui';
import {
  clockLabel,
  sessionIsNewer,
  type MeetingNote,
  type SessionView,
} from '@qrtz/core';
import { shellClient } from './shellClient';
import { useConfigTheme } from './useConfigTheme';

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, '0');
  return s >= 3600
    ? `${Math.floor(s / 3600)}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`
    : `${p(Math.floor(s / 60))}:${p(s % 60)}`;
};

/** `m:ss` left on the auto-stop countdown. */
const autoStopLabel = (ms: number) => {
  const s = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

function Hud() {
  const client = useMemo(shellClient, []);
  const [s, setS] = useState<SessionView | null>(null);
  const [stopAt, setStopAt] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    // N155: an older snapshot never overwrites a newer event.
    const apply = (v: SessionView) =>
      setS((cur) => (sessionIsNewer(cur, v) ? v : cur));
    client.invoke('note:session').then(apply, () => {});
    const off = client.on('note:session', apply);
    // Bozniak H3: the recorded meeting ended; show the countdown with "keep recording".
    const offStop = client.on('note:auto-stop', (e) =>
      setStopAt(e.state === 'armed' ? e.deadlineAt : null),
    );
    const tick = setInterval(() => setNow(Date.now()), 500);
    return () => {
      off();
      offStop();
      clearInterval(tick);
    };
  }, [client]);
  const [expanded, setExpanded] = useState(false);
  const [notes, setNotes] = useState<MeetingNote[]>([]);
  const [draft, setDraft] = useState('');
  const recording = ['starting', 'recording', 'reconnecting'].includes(
    s?.phase ?? '',
  );
  const toggle = (next: boolean) => {
    setExpanded(next);
    void client.invoke('hud:set-expanded', { expanded: next }).catch(() => {});
    if (next)
      void client.invoke('note:list-notes', {}).then(setNotes, () => {});
  };
  useEffect(() => {
    if (!recording && expanded) toggle(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording]);
  const addNote = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft('');
    client.invoke('note:add-note', { text }).then(
      (n) => setNotes((prev) => [...prev, n]),
      () => setDraft(text),
    );
  };
  const stopping = recording && stopAt !== null;
  useEffect(() => {
    if (!recording) setStopAt(null);
  }, [recording]);
  const elapsed =
    recording && s?.startedAt ? now - s.startedAt : (s?.elapsedMs ?? 0);
  const line = s?.partial || s?.lines?.at(-1)?.text || '';
  return (
    <MiniHudFrame
      strip={
        <>
          <RecordDot on={recording} />
          <StatusLines
            top={`${s?.activeSpeaker && !stopping ? `${s.activeSpeaker} · ` : ''}${
              stopping
                ? `meeting ended, stopping in ${autoStopLabel(stopAt - now)}`
                : recording
                  ? 'recording'
                  : s?.phase === 'processing'
                    ? 'summarizing…'
                    : 'qrtz'
            }`}
            bottom={line || '\u00a0'}
            bottomTestId="mini-hud-line"
          />
          <NoDrag>
            <Text variant="mono" data-testid="mini-hud-clock">
              {clock(elapsed)}
            </Text>
            {stopping ? (
              <Button
                variant="primary"
                size="sm"
                data-testid="mini-hud-keep-recording"
                onClick={() => {
                  setStopAt(null);
                  void client.invoke('note:keep-recording').catch(() => {});
                }}
              >
                keep recording
              </Button>
            ) : null}
            {recording ? (
              <IconButton
                icon="edit"
                label={expanded ? 'hide notes' : 'take a note'}
                aria-expanded={expanded}
                pressed={expanded}
                data-testid="mini-hud-notes-toggle"
                onClick={() => toggle(!expanded)}
              />
            ) : null}
            {recording ? (
              <IconButton
                icon="stop"
                tone="record"
                label="stop recording"
                data-testid="mini-hud-stop"
                onClick={() => void client.invoke('note:stop')}
              />
            ) : null}
          </NoDrag>
        </>
      }
      body={
        expanded ? (
          <>
            <Stack as="ul" gap={1} scroll aria-label="notes">
              {notes.map((n) => (
                <Stack
                  as="li"
                  dir="row"
                  gap={2}
                  align="baseline"
                  key={n.id}
                  data-testid="mini-hud-note"
                >
                  <Text variant="mono" muted>
                    {clockLabel(n.atMs)}
                  </Text>
                  <Text>{n.text}</Text>
                </Stack>
              ))}
            </Stack>
            <Stack as="form" onSubmit={addNote}>
              <Input
                autoFocus
                aria-label="add a note"
                placeholder="note"
                value={draft}
                maxLength={4000}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Escape') toggle(false);
                }}
                data-testid="mini-hud-note-input"
              />
            </Stack>
          </>
        ) : undefined
      }
    />
  );
}

/** Root for the floating always-on-top recording window (hud.html). */
export function MiniHudApp() {
  const theme = useConfigTheme();
  return (
    <ThemeProvider mode={theme}>
      <Hud />
    </ThemeProvider>
  );
}
