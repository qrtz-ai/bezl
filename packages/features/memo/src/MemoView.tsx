import { useCallback, useEffect, useState } from 'react';
import {
  Badge,
  Button,
  Callout,
  Card,
  EmptyState,
  Input,
  Text,
  useTheme,
} from '@qrtz/ui';
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
import { ChatPanel } from './ChatPanel';
import { Onboarding, needsOnboarding } from './Onboarding';
import { fmtDate } from './format';

const PAGE = 100;

export default function MemoView() {
  const qrtz = useQrtz();
  const t = useTheme();
  const [session, setSession] = useState<SessionView>(
    SessionView.parse({ phase: 'idle' }),
  );
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [onboarded, setOnboarded] = useState(false);
  const [query, setQuery] = useState('');
  // Paging: render PAGE meetings at a time; "Show more" grows the window (list stays cheap with hundreds).
  const [pages, setPages] = useState(1);
  const [askAll, setAskAll] = useState(false);
  const [seekTo, setSeekTo] = useState<{ ms: number; nonce: number } | null>(
    null,
  );
  const [hasMore, setHasMore] = useState(false);
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
      .invoke(
        'memo:list',
        query ? { query, limit: PAGE * pages } : { limit: PAGE * pages },
      )
      .then((r) => {
        const more = r.length > PAGE * pages;
        setHasMore(more);
        setItems(more ? r.slice(0, PAGE * pages) : r);
      })
      .catch((e: AppError) => setError(e.message));
  }, [qrtz, query, pages]);
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
        {session?.helper === 'missing' || session?.helper === 'down' ? (
          <Callout
            tone="danger"
            data-testid="helper-notice"
            title={
              session.helper === 'missing'
                ? 'Recording unavailable: capture helper missing'
                : 'Capture helper stopped'
            }
          >
            {session.helper === 'missing'
              ? (session.helperDetail ?? 'Reinstall qrtz to restore recording.')
              : `Recording paused; anything already captured was saved. qrtz is restarting it automatically.${session.helperDetail ? ` (${session.helperDetail})` : ''}`}
          </Callout>
        ) : null}
        <Hud
          s={session}
          onRecord={() => void act(qrtz.invoke('memo:start', {}))}
          onStop={() => void act(qrtz.invoke('memo:stop'))}
          onPause={() => void act(qrtz.invoke('memo:pause', {}))}
          onDismiss={() => void act(qrtz.invoke('memo:dismiss'))}
          onAlways={() =>
            void act(
              qrtz
                .invoke('memo:set-auto-record', { enabled: true })
                .then(() => qrtz.invoke('memo:start', {})),
            )
          }
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
        {error ? (
          <Callout
            tone="danger"
            title="Something went wrong"
            data-testid="memo-error"
            action={
              <Button variant="ghost" size="sm" onClick={() => setError(null)}>
                Dismiss
              </Button>
            }
          >
            {error}
          </Callout>
        ) : null}
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
          <Button
            variant={askAll && !selected ? 'primary' : 'secondary'}
            size="sm"
            onClick={() => {
              setAskAll(true);
              setSelected(null);
            }}
            data-testid="memo-ask-all"
          >
            Ask all meetings
          </Button>
          <Input
            placeholder="Search meetings…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setPages(1);
            }}
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
                onClick={() => {
                  setSelected(meeting.id);
                  setSeekTo(null);
                }}
                data-testid="meeting-item"
                style={{
                  contentVisibility: 'auto',
                  containIntrinsicSize: 'auto 72px',
                }}
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
          {hasMore ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setPages((n) => n + 1)}
              data-testid="memo-show-more"
            >
              Show {PAGE} more
            </Button>
          ) : null}
        </div>
        <div style={{ overflow: 'auto', minHeight: 0 }}>
          {askAll && !selected ? (
            <div style={{ display: 'grid', gap: 12 }} data-testid="ask-all">
              <Text weight={600}>Ask across all meetings</Text>
              <ChatPanel
                onOpenSource={(m, ms) => {
                  setSelected(m);
                  setSeekTo({ ms, nonce: Date.now() });
                }}
              />
            </div>
          ) : selected ? (
            <MeetingDetail
              id={selected}
              onDeleted={() => setSelected(null)}
              {...(seekTo ? { seekTo } : {})}
            />
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
