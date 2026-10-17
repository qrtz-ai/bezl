import { useEffect, useState } from 'react';
import { Banner, Button, RecordingBar, Spinner, Stack, Text } from '@qrtz/ui';
import type { SessionView } from '@qrtz/core';
import { PLATFORM_LABEL, fmtClock } from './format';
import { Notes } from './Notes';

function useNow(active: boolean) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [active]);
  return now;
}

/** Live recording HUD: detection prompt → live transcript → processing → done. */
export function Hud({
  s,
  onRecord,
  onStop,
  onDismiss,
  onAlways,
  onOpen,
  onPause,
}: {
  s: SessionView;
  onPause?: () => void;
  onRecord: () => void;
  onStop: () => void;
  onDismiss: () => void;
  onAlways?: () => void;
  onOpen: (id: string) => void;
}) {
  const capturing =
    s.phase === 'starting' ||
    s.phase === 'recording' ||
    s.phase === 'reconnecting';
  const now = useNow(capturing);
  if (s.phase === 'idle') return null;
  if (s.phase === 'detected')
    return (
      <Banner
        data-testid="note-detected"
        actions={
          <>
            <Button size="sm" onClick={onRecord} data-testid="note-record">
              Record
            </Button>
            <Button size="sm" variant="ghost" onClick={onDismiss}>
              Not now
            </Button>
            {onAlways ? (
              <Button
                size="sm"
                variant="ghost"
                onClick={onAlways}
                data-testid="note-always-record"
              >
                Always
              </Button>
            ) : null}
          </>
        }
      >
        <Text truncate>{s.detected?.title}</Text>
        <Text variant="caption" muted>
          {PLATFORM_LABEL[s.detected?.platform ?? 'other']}
        </Text>
      </Banner>
    );
  if (capturing) {
    const elapsed = s.startedAt ? now - s.startedAt : s.elapsedMs;
    return (
      <Stack gap={4} data-testid="note-hud">
        <RecordingBar
          clock={fmtClock(elapsed)}
          title={s.detected?.title ?? 'Recording'}
          level={s.level}
          speaker={s.activeSpeaker}
          paused={s.paused}
          {...(onPause ? { onPause } : {})}
          onStop={onStop}
        />
        <Notes compact />
      </Stack>
    );
  }
  if (s.phase === 'stopping' || s.phase === 'processing')
    return (
      <Banner data-testid="note-processing" actions={<Spinner size={14} />}>
        <Text>
          {s.phase === 'stopping' ? 'Stopping' : (s.progress ?? 'Summarizing')}…
        </Text>
      </Banner>
    );
  if (s.phase === 'done')
    return (
      <Banner
        data-testid="note-done"
        actions={
          <>
            {s.meetingId ? (
              <Button
                size="sm"
                onClick={() => onOpen(s.meetingId!)}
                data-testid="note-open"
              >
                Open
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" onClick={onDismiss}>
              Dismiss
            </Button>
          </>
        }
      >
        <Text>Ready</Text>
      </Banner>
    );
  return (
    <Banner
      tone="danger"
      data-testid="note-error"
      actions={
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          Dismiss
        </Button>
      }
    >
      <Text tone="danger">Recording failed: {s.error}</Text>
    </Banner>
  );
}
