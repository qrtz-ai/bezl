import { useEffect, useMemo, useState } from 'react';
import {
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
import { clockLabel, type MeetingNote, type SessionView } from '@qrtz/core';
import { shellClient } from './shellClient';
import { useConfigTheme } from './useConfigTheme';

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, '0');
  return s >= 3600
    ? `${Math.floor(s / 3600)}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}`
    : `${p(Math.floor(s / 60))}:${p(s % 60)}`;
};

function Hud() {
  const client = useMemo(shellClient, []);
  const [s, setS] = useState<SessionView | null>(null);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    client.invoke('memo:session').then(setS, () => {});
    const off = client.on('memo:session', setS);
    const tick = setInterval(() => setNow(Date.now()), 500);
    return () => {
      off();
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
      void client.invoke('memo:list-notes', {}).then(setNotes, () => {});
  };
  useEffect(() => {
    if (!recording && expanded) toggle(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording]);
  const addNote = () => {
    const text = draft.trim();
    if (!text) return;
    setDraft('');
    client.invoke('memo:add-note', { text }).then(
      (n) => setNotes((prev) => [...prev, n]),
      () => setDraft(text),
    );
  };
  const elapsed =
    recording && s?.startedAt ? now - s.startedAt : (s?.elapsedMs ?? 0);
  const line = s?.partial || s?.lines?.at(-1)?.text || '';
  return (
    <MiniHudFrame
      strip={
        <>
          <RecordDot on={recording} />
          <StatusLines
            top={`${s?.activeSpeaker ? `${s.activeSpeaker} · ` : ''}${
              recording
                ? 'Recording'
                : s?.phase === 'processing'
                  ? 'Summarizing…'
                  : 'qrtz'
            }`}
            bottom={line || '\u00a0'}
            bottomTestId="mini-hud-line"
          />
          <NoDrag>
            <Text variant="mono" data-testid="mini-hud-clock">
              {clock(elapsed)}
            </Text>
            {recording ? (
              <IconButton
                icon="edit"
                label={expanded ? 'Hide notes' : 'Take a note'}
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
                label="Stop recording"
                data-testid="mini-hud-stop"
                onClick={() => void client.invoke('memo:stop')}
              />
            ) : null}
          </NoDrag>
        </>
      }
      body={
        expanded ? (
          <>
            <Stack as="ul" gap={1} scroll aria-label="Notes">
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
                aria-label="Add a note"
                placeholder="Note"
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

/** Root for the floating always-on-top recording window (index.html#hud). */
export function MiniHudApp() {
  const theme = useConfigTheme();
  return (
    <ThemeProvider mode={theme}>
      <Hud />
    </ThemeProvider>
  );
}
