import { useCallback, useEffect, useState } from 'react';
import { Badge, Card, EmptyState, Input, Text, useTheme } from '@qrtz/ui';
import {
  AppError,
  SessionView,
  onNavigateRequest,
  takePendingMeeting,
  type PermissionsStatus,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { Hud } from './Hud';
import { MeetingDetail } from './MeetingDetail';
import { Onboarding, needsOnboarding } from './Onboarding';
import { fmtDate } from './format';

export default function MemoView() {
  const qrtz = useQrtz();
  const t = useTheme();
  const [session, setSession] = useState<SessionView>(
    SessionView.parse({ phase: 'idle' }),
  );
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [onboarded, setOnboarded] = useState(false);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<
    {
      meeting: { id: string; title: string; startedAt: number; status: string };
      hit?: { snippet: string };
    }[]
  >([]);
  const [selected, setSelected] = useState<string | null>(
    () => takePendingMeeting() ?? null,
  );
  useEffect(
    () =>
      onNavigateRequest((r) => {
        if (r.app === 'memo' && r.meetingId)
          setSelected(takePendingMeeting() ?? null);
      }),
    [],
  );
  const [error, setError] = useState<string | null>(null);
  const liveSession = useQrtzEvent('memo:session');
  const livePerms = useQrtzEvent('permissions:changed');
  const changed = useQrtzEvent('memo:changed');

  useEffect(() => {
    void qrtz.invoke('memo:session').then(setSession);
    void qrtz.invoke('permissions:get').then(setPerms);
  }, [qrtz]);
  useEffect(() => {
    if (liveSession) setSession(liveSession);
  }, [liveSession]);
  useEffect(() => {
    if (livePerms) setPerms(livePerms);
  }, [livePerms]);

  const refresh = useCallback(() => {
    qrtz
      .invoke('memo:list', { query })
      .then(setItems)
      .catch((e: AppError) => setError(e.message));
  }, [qrtz, query]);
  useEffect(() => {
    const h = setTimeout(refresh, query ? 150 : 0);
    return () => clearTimeout(h);
  }, [refresh, changed, query]);

  const act = (p: Promise<SessionView>) =>
    p.then(setSession).catch((e: AppError) => setError(e.message));

  if (perms && needsOnboarding(perms) && !onboarded)
    return (
      <Onboarding
        status={perms}
        onRequest={(p) =>
          void qrtz
            .invoke('permissions:request', { permission: p })
            .then(setPerms)
        }
        onDone={() => setOnboarded(true)}
      />
    );

  return (
    <div
      style={{
        display: 'grid',
        gap: 16,
        height: '100%',
        gridTemplateRows: 'auto 1fr',
      }}
    >
      <div style={{ display: 'grid', gap: 8 }}>
        <Hud
          s={session}
          onRecord={() => void act(qrtz.invoke('memo:start', {}))}
          onStop={() => void act(qrtz.invoke('memo:stop'))}
          onDismiss={() => void act(qrtz.invoke('memo:dismiss'))}
          onOpen={(id) => setSelected(id)}
        />
        {session.phase === 'idle' || session.phase === 'done' ? (
          <Text size="sm" muted>
            Memo starts automatically when it detects a Zoom or Google Meet
            call.{' '}
            <a
              href="#"
              data-testid="memo-record-manual"
              onClick={(e) => {
                e.preventDefault();
                void act(qrtz.invoke('memo:start', {}));
              }}
              style={{ color: t.color.text }}
            >
              Record now
            </a>
          </Text>
        ) : null}
        {error ? <Text style={{ color: t.color.danger }}>{error}</Text> : null}
      </div>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: '300px 1fr',
          gap: 24,
          minHeight: 0,
        }}
      >
        <div
          style={{
            display: 'grid',
            gap: 8,
            alignContent: 'start',
            overflow: 'auto',
          }}
        >
          <Input
            placeholder="Search meetings…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            data-testid="memo-search"
            aria-label="Search meetings"
          />
          {items.length === 0 ? (
            <EmptyState
              icon="●"
              title={query ? 'No matches' : 'No meetings yet'}
              body={
                query
                  ? undefined
                  : 'Join a Zoom or Google Meet call to get started.'
              }
            />
          ) : (
            items.map(({ meeting, hit }) => (
              <Card
                key={meeting.id}
                padding={3}
                interactive
                selected={selected === meeting.id}
                onClick={() => setSelected(meeting.id)}
                data-testid="meeting-item"
              >
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                  <Text
                    weight={600}
                    style={{
                      flex: 1,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {meeting.title}
                  </Text>
                  {meeting.status !== 'done' ? (
                    <Badge
                      tone={meeting.status === 'failed' ? 'danger' : 'warning'}
                    >
                      {meeting.status}
                    </Badge>
                  ) : null}
                </div>
                <Text size="xs" muted>
                  {fmtDate(meeting.startedAt)}
                </Text>
                {hit ? (
                  <Text
                    as="div"
                    size="xs"
                    muted
                    data-testid="search-snippet"
                    style={{ marginTop: 4 }}
                  >
                    {hit.snippet}
                  </Text>
                ) : null}
              </Card>
            ))
          )}
        </div>
        <div style={{ overflow: 'auto', minHeight: 0 }}>
          {selected ? (
            <MeetingDetail id={selected} onDeleted={() => setSelected(null)} />
          ) : (
            <EmptyState
              icon="_"
              title="Select a meeting"
              body="Transcripts, summaries and action items show up here."
            />
          )}
        </div>
      </div>
    </div>
  );
}
