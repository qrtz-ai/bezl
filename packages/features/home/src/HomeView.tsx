import { useCallback, useEffect, useState } from 'react';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Heading,
  Kbd,
  ListItem,
  Spinner,
  Text,
  useTheme,
  Callout,
} from '@qrtz/ui';
import {
  requestNavigate,
  type OsPermission,
  type PermissionsStatus,
  type SessionView,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

const PERMS: { id: OsPermission; label: string; why: string }[] = [
  {
    id: 'accessibility',
    label: 'Accessibility',
    why: 'Detect calls, names and who is speaking',
  },
  {
    id: 'screen-recording',
    label: 'Screen & System Audio',
    why: 'Hear the other side of the call',
  },
  { id: 'microphone', label: 'Microphone', why: 'Hear you' },
];

const fmtWhen = (ms: number) => {
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay
    ? `Today, ${d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`
    : d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
};
const fmtDur = (ms?: number) => {
  if (!ms) return '';
  const m = Math.round(ms / 60_000);
  return m < 1
    ? '<1 min'
    : m < 60
      ? `${m} min`
      : `${Math.floor(m / 60)} h ${m % 60} min`;
};

type Item = {
  meeting: {
    id: string;
    title: string;
    startedAt: number;
    status: string;
    durationMs?: number;
    platform: string;
  };
};

export default function HomeView() {
  const t = useTheme();
  const qrtz = useQrtz();
  const [recent, setRecent] = useState<Item[] | null>(null);
  const [showSetup, setShowSetup] = useState(false);
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const live = useQrtzEvent('memo:session');
  const changed = useQrtzEvent('memo:changed');
  const [session, setSession] = useState<SessionView | null>(null);

  const load = useCallback(() => {
    qrtz.invoke('memo:list', { limit: 5 }).then(
      (r) => setRecent(r.slice(0, 5) as Item[]),
      (e: Error) => setError(e.message),
    );
    qrtz.invoke('permissions:get').then(setPerms, () => {});
    qrtz.invoke('memo:session').then(setSession, () => {});
  }, [qrtz]);
  useEffect(load, [load, changed]);
  const s = live ?? session;
  const recording = s?.phase === 'recording';

  const record = async () => {
    setBusy(true);
    try {
      await qrtz.invoke('memo:start', {});
      requestNavigate({ app: 'memo' });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const missing = perms ? PERMS.filter((p) => perms[p.id] !== 'granted') : [];
  // Once everything is granted and meetings exist, setup collapses to one line below the meetings.
  const settled = !!perms && missing.length === 0 && !!recent?.length;
  const setupCollapsed = settled && !showSetup;

  return (
    <div
      style={{ display: 'grid', gap: t.space[8], maxWidth: 760 }}
      data-testid="home"
    >
      <header style={{ display: 'grid', gap: t.space[3] }}>
        <Heading cursor style={{ fontSize: 44 }}>
          qrtz
        </Heading>
        <Text muted size="lg">
          Meeting notes that write themselves, on-device.
        </Text>
        <div
          style={{
            display: 'flex',
            gap: t.space[3],
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          {recording ? (
            <Button
              variant="danger"
              onClick={() => requestNavigate({ app: 'memo' })}
              data-testid="home-recording"
            >
              ● Recording · open
            </Button>
          ) : (
            <Button
              onClick={() => void record()}
              loading={busy}
              disabled={s?.helper === 'missing'}
              data-testid="home-record"
            >
              Record now
            </Button>
          )}
          <Text size="sm" muted>
            or join a Zoom / Google Meet call · <Kbd>⌘</Kbd> <Kbd>⇧</Kbd>{' '}
            <Kbd>R</Kbd> · <Kbd>⌘</Kbd> <Kbd>K</Kbd> search
          </Text>
        </div>
      </header>

      {s?.helper === 'missing' || s?.helper === 'down' ? (
        <Callout
          tone="danger"
          data-testid="helper-notice"
          title={
            s.helper === 'missing'
              ? 'Recording unavailable: capture helper missing'
              : 'Capture helper stopped'
          }
        >
          {s.helper === 'missing'
            ? (s.helperDetail ?? 'Reinstall qrtz to restore recording.')
            : `Recording paused; anything already captured was saved. qrtz is restarting it automatically.${s.helperDetail ? ` (${s.helperDetail})` : ''}`}
        </Callout>
      ) : null}

      {error ? (
        <Card padding={4} role="alert" data-testid="home-error">
          <Text size="sm">Something went wrong: {error}</Text>{' '}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setError(null);
              load();
            }}
          >
            Retry
          </Button>
        </Card>
      ) : null}

      <section
        aria-labelledby="perm-h"
        style={{ display: 'grid', gap: t.space[3], order: settled ? 2 : 0 }}
        data-testid="home-permissions"
      >
        <Text
          id="perm-h"
          mono
          size="xs"
          muted
          style={{ textTransform: 'uppercase', letterSpacing: '0.08em' }}
        >
          Setup{' '}
          {!perms
            ? ''
            : PERMS.every((p) => perms[p.id] === 'unsupported')
              ? '· capture needs macOS'
              : `· ${PERMS.length - missing.length}/${PERMS.length}`}
        </Text>
        {setupCollapsed ? (
          <Card
            padding={3}
            style={{ display: 'flex', alignItems: 'center', gap: t.space[3] }}
          >
            <span
              aria-hidden
              style={{
                width: 8,
                height: 8,
                borderRadius: 4,
                background: t.color.success,
              }}
            />
            <Text size="sm" style={{ flex: 1 }}>
              All set: qrtz can hear both sides of your calls.
            </Text>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setShowSetup(true)}
              data-testid="home-setup-expand"
            >
              Details
            </Button>
          </Card>
        ) : (
          <Card padding={3}>
            {PERMS.map((p) => {
              const st = perms?.[p.id];
              const ok = st === 'granted';
              return (
                <div
                  key={p.id}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: t.space[3],
                    padding: `${t.space[2]} ${t.space[2]}`,
                  }}
                >
                  <span
                    aria-hidden
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: 4,
                      background: ok
                        ? t.color.success
                        : st === 'unsupported'
                          ? t.color.textSubtle
                          : t.color.warning,
                    }}
                  />
                  <div style={{ flex: 1, display: 'grid' }}>
                    <Text size="sm">{p.label}</Text>
                    <Text size="xs" muted>
                      {p.why}
                    </Text>
                  </div>
                  {!perms ? (
                    <Spinner size={12} />
                  ) : ok ? (
                    <Badge tone="success">Granted</Badge>
                  ) : st === 'unsupported' ? (
                    <Badge tone="neutral">Mac only</Badge>
                  ) : (
                    <Button
                      size="sm"
                      variant="secondary"
                      aria-label={`Grant ${p.label}`}
                      onClick={() =>
                        void qrtz
                          .invoke('permissions:request', { permission: p.id })
                          .then(setPerms)
                      }
                    >
                      Grant
                    </Button>
                  )}
                </div>
              );
            })}
          </Card>
        )}
      </section>

      <section
        aria-labelledby="recent-h"
        style={{ display: 'grid', gap: t.space[3] }}
        data-testid="home-recent"
      >
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'baseline',
          }}
        >
          <Text
            id="recent-h"
            mono
            size="xs"
            muted
            style={{ textTransform: 'uppercase', letterSpacing: '0.08em' }}
          >
            Recent meetings
          </Text>
          {recent?.length ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => requestNavigate({ app: 'memo' })}
            >
              All meetings →
            </Button>
          ) : null}
        </div>
        {recent === null ? (
          <Spinner />
        ) : recent.length === 0 ? (
          <Card padding={4}>
            <EmptyState
              icon="◎"
              title="No meetings yet"
              body="Your first meeting shows up here with its summary and action items."
            />
          </Card>
        ) : (
          <div
            role="listbox"
            aria-label="Recent meetings"
            style={{ display: 'grid', gap: 2 }}
          >
            {recent.map(({ meeting: m }) => (
              <ListItem
                key={m.id}
                data-testid="home-meeting"
                onClick={() =>
                  requestNavigate({ app: 'memo', meetingId: m.id })
                }
                right={
                  <span
                    style={{ display: 'flex', gap: 8, alignItems: 'center' }}
                  >
                    {m.status !== 'done' ? (
                      <Badge
                        tone={m.status === 'failed' ? 'danger' : 'warning'}
                      >
                        {m.status}
                      </Badge>
                    ) : null}
                    <Text size="xs" mono muted>
                      {fmtDur(m.durationMs)}
                    </Text>
                  </span>
                }
              >
                <Text size="sm">{m.title}</Text>{' '}
                <Text size="xs" muted>
                  · {fmtWhen(m.startedAt)}
                </Text>
              </ListItem>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
