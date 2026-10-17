import { useEffect, useMemo, useState } from 'react';
import { Text, ThemeProvider, useTheme } from '@qrtz/ui';
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
  const t = useTheme();
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
    <div
      style={{
        height: '100vh',
        display: 'grid',
        gridTemplateRows: expanded ? '56px 1fr' : '1fr',
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: 12,
        overflow: 'hidden',
      }}
    >
      <div
        data-testid="mini-hud"
        className="drag-region-full"
        style={{
          height: expanded ? 56 : '100%',
          display: 'grid',
          gridTemplateColumns: 'auto 1fr auto',
          alignItems: 'center',
          gap: 10,
          padding: '0 12px',
          overflow: 'hidden',
        }}
      >
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: 4,
            background: recording ? t.color.recording : t.color.textSubtle,
            animation: recording
              ? 'q-pulse 1.4s ease-in-out infinite'
              : undefined,
          }}
        />
        <div style={{ minWidth: 0, display: 'grid' }}>
          <Text
            size="xs"
            muted
            style={{
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {s?.activeSpeaker ? `${s.activeSpeaker} · ` : ''}
            {recording
              ? 'Recording'
              : s?.phase === 'processing'
                ? 'Summarizing…'
                : 'qrtz'}
          </Text>
          <Text
            size="sm"
            data-testid="mini-hud-line"
            style={{
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {line || '\u00a0'}
          </Text>
        </div>
        <div
          className="no-drag"
          style={{ display: 'flex', alignItems: 'center', gap: 8 }}
        >
          <Text mono size="sm" data-testid="mini-hud-clock">
            {clock(elapsed)}
          </Text>
          {recording ? (
            <button
              type="button"
              aria-label={expanded ? 'Hide notes' : 'Take a note'}
              aria-expanded={expanded}
              data-testid="mini-hud-notes-toggle"
              onClick={() => toggle(!expanded)}
              style={{
                all: 'unset',
                cursor: 'pointer',
                width: 24,
                height: 24,
                borderRadius: 6,
                display: 'grid',
                placeItems: 'center',
                color: t.color.text,
                border: `1px solid ${t.color.borderStrong}`,
                fontSize: 12,
              }}
            >
              ✎
            </button>
          ) : null}
          {recording ? (
            <button
              type="button"
              aria-label="Stop recording"
              data-testid="mini-hud-stop"
              onClick={() => void client.invoke('memo:stop')}
              style={{
                all: 'unset',
                cursor: 'pointer',
                width: 24,
                height: 24,
                borderRadius: 6,
                display: 'grid',
                placeItems: 'center',
                background: t.color.accent,
                border: `1px solid ${t.color.borderStrong}`,
              }}
            >
              <span
                style={{
                  width: 8,
                  height: 8,
                  borderRadius: 2,
                  background: t.color.recording,
                }}
              />
            </button>
          ) : null}
        </div>
      </div>
      {expanded ? (
        <div
          className="no-drag"
          data-testid="mini-hud-notes"
          style={{
            display: 'grid',
            gridTemplateRows: '1fr auto',
            gap: 6,
            padding: '0 12px 10px',
            minHeight: 0,
          }}
        >
          <ul
            aria-label="Notes"
            style={{
              listStyle: 'none',
              margin: 0,
              padding: 0,
              overflow: 'auto',
            }}
          >
            {notes.map((n) => (
              <li key={n.id} data-testid="mini-hud-note">
                <Text size="xs" muted mono>
                  {clockLabel(n.atMs)}
                </Text>{' '}
                <Text size="sm">{n.text}</Text>
              </li>
            ))}
          </ul>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              addNote();
            }}
          >
            <input
              autoFocus
              aria-label="Add a note"
              placeholder="Note (Enter to save)"
              value={draft}
              maxLength={4000}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape') toggle(false);
              }}
              data-testid="mini-hud-note-input"
              style={{
                width: '100%',
                boxSizing: 'border-box',
                height: 30,
                padding: '0 8px',
                borderRadius: 6,
                border: `1px solid ${t.color.border}`,
                background: t.color.bg,
                color: t.color.text,
                font: 'inherit',
                fontSize: 13,
              }}
            />
          </form>
        </div>
      ) : null}
    </div>
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
