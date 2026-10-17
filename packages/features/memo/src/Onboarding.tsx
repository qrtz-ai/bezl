import { Badge, Button, Card, Heading, Text } from '@qrtz/ui';
import type { OsPermission, PermissionsStatus } from '@qrtz/core';

const COPY: Record<OsPermission, { title: string; why: string }> = {
  accessibility: {
    title: 'Accessibility',
    why: 'Detects Zoom and Google Meet calls and reads the meeting title, participants and who is speaking.',
  },
  'screen-recording': {
    title: 'Screen & System Audio Recording',
    why: 'Captures the other participants’ audio from the meeting app. Nothing on screen is recorded.',
  },
  microphone: {
    title: 'Microphone',
    why: 'Captures your side of the conversation.',
  },
};

export const needsOnboarding = (p: PermissionsStatus) =>
  Object.values(p).some((s) => s === 'denied' || s === 'not-determined');

export function Onboarding({
  status,
  onRequest,
  onDone,
}: {
  status: PermissionsStatus;
  onRequest: (p: OsPermission) => void;
  onDone: () => void;
}) {
  return (
    <div
      data-testid="memo-onboarding"
      style={{ display: 'grid', gap: 16, maxWidth: 560 }}
    >
      <Heading cursor>Set up Memo</Heading>
      <Text muted>
        Memo runs entirely on your Mac. Grant these so it can find and
        transcribe your meetings.
      </Text>
      {(Object.keys(COPY) as OsPermission[]).map((p) => {
        const s = status[p] ?? 'unknown';
        const ok = s === 'granted' || s === 'unsupported';
        return (
          <Card
            key={p}
            padding={4}
            style={{ display: 'flex', gap: 16, alignItems: 'center' }}
          >
            <div style={{ flex: 1, display: 'grid', gap: 4 }}>
              <Text weight={600}>{COPY[p].title}</Text>
              <Text muted size="sm">
                {COPY[p].why}
              </Text>
            </div>
            {ok ? (
              <Badge tone="success">
                {s === 'granted' ? 'Granted' : 'Not needed'}
              </Badge>
            ) : (
              <Button size="sm" onClick={() => onRequest(p)}>
                {s === 'denied' ? 'Open Settings' : 'Allow'}
              </Button>
            )}
          </Card>
        );
      })}
      <div>
        <Button
          variant="ghost"
          onClick={onDone}
          data-testid="onboarding-continue"
        >
          Continue
        </Button>
      </div>
    </div>
  );
}
