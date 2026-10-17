import { useEffect, useMemo, useState } from 'react';
import { Text, ThemeProvider, useTheme } from '@qrtz/ui';
import type { SessionView } from '@qrtz/core';
import { shellClient } from './shellClient';

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
  const recording = s?.phase === 'recording';
  const elapsed =
    recording && s?.startedAt ? now - s.startedAt : (s?.elapsedMs ?? 0);
  const line = s?.partial || s?.lines?.at(-1)?.text || '';
  return (
    <div
      data-testid="mini-hud"
      className="drag-region-full"
      style={{
        height: '100vh',
        display: 'grid',
        gridTemplateColumns: 'auto 1fr auto',
        alignItems: 'center',
        gap: 10,
        padding: '0 12px',
        background: t.color.surface,
        border: `1px solid ${t.color.borderStrong}`,
        borderRadius: 12,
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
  );
}

/** Root for the floating always-on-top recording window (index.html#hud). */
export function MiniHudApp() {
  return (
    <ThemeProvider>
      <Hud />
    </ThemeProvider>
  );
}
