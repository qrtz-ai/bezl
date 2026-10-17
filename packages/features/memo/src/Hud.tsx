import { useEffect, useState } from 'react';
import { Badge, Button, Card, Spinner, Text, useTheme } from '@qrtz/ui';
import type { SessionView } from '@qrtz/core';
import { PLATFORM_LABEL, fmtClock } from './format';

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
  onOpen,
}: {
  s: SessionView;
  onRecord: () => void;
  onStop: () => void;
  onDismiss: () => void;
  onOpen: (id: string) => void;
}) {
  const t = useTheme();
  const now = useNow(s.phase === 'recording');
  if (s.phase === 'idle') return null;
  if (s.phase === 'detected')
    return (
      <Card
        padding={4}
        data-testid="memo-detected"
        style={{ display: 'flex', alignItems: 'center', gap: 12 }}
      >
        <Badge>{PLATFORM_LABEL[s.detected?.platform ?? 'other']}</Badge>
        <div style={{ flex: 1 }}>
          <Text weight={600}>{s.detected?.title}</Text>
          <Text muted size="sm">
            {' '}
            · meeting detected
            {s.participants.length
              ? ` · ${s.participants.length} participants`
              : ''}
          </Text>
        </div>
        <Button onClick={onRecord} data-testid="memo-record">
          ● Record
        </Button>
        <Button variant="ghost" onClick={onDismiss}>
          Not now
        </Button>
      </Card>
    );
  if (s.phase === 'recording') {
    const elapsed = s.startedAt ? now - s.startedAt : s.elapsedMs;
    return (
      <Card
        padding={4}
        data-testid="memo-hud"
        style={{ display: 'grid', gap: 12, borderColor: '#7f1d1d' }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <span
            aria-label="recording"
            style={{
              width: 10,
              height: 10,
              borderRadius: 99,
              background: t.color.recording,
              animation: 'q-pulse 1.4s ease-in-out infinite',
            }}
          />
          <Text weight={600} style={{ fontFamily: t.font.mono }}>
            {fmtClock(elapsed)}
          </Text>
          <Text muted size="sm">
            {s.detected?.title ?? 'Recording'}
          </Text>
          <div
            aria-hidden
            style={{
              width: 80,
              height: 4,
              background: t.color.border,
              borderRadius: 99,
              overflow: 'hidden',
            }}
          >
            <div
              style={{
                width: `${Math.round(s.level * 100)}%`,
                height: '100%',
                background: t.color.success,
                transition: 'width 120ms linear',
              }}
            />
          </div>
          {s.activeSpeaker ? (
            <Badge tone="neutral">
              <span data-testid="memo-active-speaker">{s.activeSpeaker}</span>{' '}
              speaking
            </Badge>
          ) : null}
          <div style={{ flex: 1 }} />
          <Button
            variant="danger"
            size="sm"
            onClick={onStop}
            data-testid="memo-stop"
          >
            ■ Stop
          </Button>
        </div>
        <div
          data-testid="memo-live"
          style={{ display: 'grid', gap: 6, maxHeight: 200, overflow: 'auto' }}
        >
          {s.lines.map((l) => (
            <div
              key={l.id}
              data-testid="memo-live-line"
              style={{
                animation: `q-fade-in ${t.motion.normal} ${t.motion.easeOut}`,
              }}
            >
              <Text size="sm" weight={600}>
                {l.speaker ?? 'Speaker'}:{' '}
              </Text>
              <Text size="sm">{l.text}</Text>
            </div>
          ))}
          {s.partial ? (
            <Text size="sm" muted data-testid="memo-partial">
              {s.partial}…
            </Text>
          ) : null}
        </div>
      </Card>
    );
  }
  if (s.phase === 'processing')
    return (
      <Card
        padding={4}
        data-testid="memo-processing"
        style={{ display: 'flex', gap: 12, alignItems: 'center' }}
      >
        <Spinner />
        <Text>{s.progress ?? 'Summarizing'}…</Text>
      </Card>
    );
  if (s.phase === 'done')
    return (
      <Card
        padding={4}
        data-testid="memo-done"
        style={{ display: 'flex', gap: 12, alignItems: 'center' }}
      >
        <Badge tone="success">Ready</Badge>
        <Text style={{ flex: 1 }}>Summary and action items are ready.</Text>
        {s.meetingId ? (
          <Button
            size="sm"
            onClick={() => onOpen(s.meetingId!)}
            data-testid="memo-open"
          >
            Open
          </Button>
        ) : null}
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          Dismiss
        </Button>
      </Card>
    );
  return (
    <Card padding={4} data-testid="memo-error">
      <Text style={{ color: t.color.danger }}>Recording failed: {s.error}</Text>{' '}
      <Button size="sm" variant="ghost" onClick={onDismiss}>
        Dismiss
      </Button>
    </Card>
  );
}
