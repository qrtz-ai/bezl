import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Callout,
  CaptureStatus,
  EmptyState,
  Heading,
  IconButton,
  Input,
  MeetingRow,
  SplitView,
  Stack,
  Toolbar,
  useToast,
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
import { isCapturingPhase, useSession } from './useRecording';
import { MeetingDetail } from './MeetingDetail';
import { ChatPanel } from './ChatPanel';
import { Onboarding, needsOnboarding } from './Onboarding';
import { fmtDate } from './format';

const UNDO_MS = 6000;

const PAGE = 100;

export default function NoteView() {
  const qrtz = useQrtz();
  // Lifecycle state comes only from main (note:session); the view never guesses.
  const session = useSession();
  const [perms, setPerms] = useState<PermissionsStatus | null>(null);
  const [onboarded, setOnboarded] = useState(false);
  const [query, setQuery] = useState('');
  const [searching, setSearching] = useState(false);
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
        if (r.app === 'note' && r.meetingId)
          setSelected(takePendingMeeting() ?? null);
      }),
    [],
  );
  const [error, setError] = useState<string | null>(null);
  const toast = useToast();
  // Main defers the delete for UNDO_MS behind a durable tombstone (completed on next launch if we quit first).
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
  const unhide = (id: string) =>
    setHidden((h) => {
      const n = new Set(h);
      n.delete(id);
      return n;
    });
  const requestDelete = (id: string, title: string) => {
    setHidden((h) => new Set(h).add(id));
    setSelected(null);
    qrtz
      .invoke('note:delete', { id, deferMs: UNDO_MS })
      .catch((e: AppError) => {
        unhide(id);
        setError(e.message);
      });
    toast(`Deleted “${title}”`, 'info', {
      durationMs: UNDO_MS,
      action: {
        label: 'Undo',
        onClick: () => {
          void qrtz.invoke('note:undo-delete', { id }).then((r) => {
            unhide(id);
            if (r.restored) setSelected(id);
          });
        },
      },
    });
  };
  const visible = items.filter((i) => !hidden.has(i.meeting.id));
  const livePerms = useQrtzEvent('permissions:changed');
  const changed = useQrtzEvent('note:changed');

  useEffect(() => {
    void qrtz.invoke('permissions:get').then(setPerms);
  }, [qrtz]);
  // A new recording opens its meeting so the live transcript is front and center.
  const liveId = isCapturingPhase(session.phase)
    ? session.meetingId
    : undefined;
  useEffect(() => {
    if (liveId) {
      setSelected(liveId);
      setAskAll(false);
    }
  }, [liveId]);
  useEffect(() => {
    if (livePerms) setPerms(livePerms);
  }, [livePerms]);

  const refresh = useCallback(() => {
    qrtz
      .invoke(
        'note:list',
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

  // Results arrive as note:session events. Refused starts (permissions, ack timeout) are lifecycle state
  // (`errorInfo`, shown by CaptureStatus); only unexpected failures land in the generic error callout.
  const act = (p: Promise<SessionView>) =>
    p.catch((e: AppError) => {
      if (e.code !== 'E_FORBIDDEN' && e.code !== 'E_TIMEOUT')
        setError(e.message);
    });

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

  const busy =
    isCapturingPhase(session.phase) ||
    session.phase === 'stopping' ||
    session.phase === 'processing';
  return (
    <SplitView
      data-testid="note-app"
      sidebar={
        <>
          <Toolbar
            end={
              <>
                <IconButton
                  icon="search"
                  label="Search meetings"
                  pressed={searching}
                  data-testid="note-search-toggle"
                  onClick={() => {
                    if (searching) setQuery('');
                    setSearching(!searching);
                  }}
                />
                <IconButton
                  icon="ask"
                  label="Ask all meetings"
                  pressed={askAll && !selected}
                  data-testid="note-ask-all"
                  onClick={() => {
                    setAskAll(true);
                    setSelected(null);
                  }}
                />
              </>
            }
          >
            {busy ? null : (
              <Button
                size="sm"
                onClick={() => void act(qrtz.invoke('note:start', {}))}
                data-testid="note-record-manual"
                icon="circle"
              >
                Record
              </Button>
            )}
          </Toolbar>
          {searching ? (
            <Input
              autoFocus
              placeholder="Search"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setPages(1);
              }}
              data-testid="note-search"
              aria-label="Search meetings"
            />
          ) : null}
          <Stack as="ul" gap={0} scroll grow aria-label="Meetings">
            {visible.length === 0 ? (
              <Stack as="li">
                <EmptyState title={query ? 'No matches' : 'No meetings'} />
              </Stack>
            ) : (
              visible.map(({ meeting, hit }) => (
                <MeetingRow
                  key={meeting.id}
                  title={meeting.title}
                  date={
                    meeting.status === 'done'
                      ? fmtDate(meeting.startedAt)
                      : `${fmtDate(meeting.startedAt)} · ${meeting.status}`
                  }
                  {...(hit ? { snippet: hit.snippet } : {})}
                  selected={selected === meeting.id}
                  onSelect={() => {
                    setSelected(meeting.id);
                    setSeekTo(null);
                  }}
                />
              ))
            )}
            {hasMore ? (
              <Stack as="li">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setPages((n) => n + 1)}
                  data-testid="note-show-more"
                >
                  More
                </Button>
              </Stack>
            ) : null}
          </Stack>
        </>
      }
    >
      <Stack gap={6}>
        {session?.helper === 'missing' || session?.helper === 'down' ? (
          <Callout
            tone="danger"
            data-testid="helper-notice"
            title={
              session.helper === 'missing'
                ? 'Recording unavailable: capture helper missing'
                : 'Capture helper stopped, restarting'
            }
          />
        ) : null}
        {error ? (
          <Callout
            tone="danger"
            title={error}
            data-testid="note-error"
            action={
              <Button variant="ghost" size="sm" onClick={() => setError(null)}>
                Dismiss
              </Button>
            }
          />
        ) : null}
        <CaptureStatus
          degraded={[]}
          error={session.errorInfo?.message ?? null}
          onDismiss={() => void act(qrtz.invoke('note:dismiss'))}
        />
        <Hud
          s={session}
          onRecord={() => void act(qrtz.invoke('note:start', {}))}
          onStop={() => void act(qrtz.invoke('note:stop'))}
          onPause={() => void act(qrtz.invoke('note:pause', {}))}
          onDismiss={() => void act(qrtz.invoke('note:dismiss'))}
          onAlways={() =>
            void act(
              qrtz
                .invoke('note:set-auto-record', { enabled: true })
                .then(() => qrtz.invoke('note:start', {})),
            )
          }
          onOpen={(id) => setSelected(id)}
        />
        {askAll && !selected ? (
          <Stack gap={3} data-testid="ask-all">
            <Heading level={2}>ask</Heading>
            <ChatPanel
              onOpenSource={(m, ms) => {
                setSelected(m);
                setSeekTo({ ms, nonce: Date.now() });
              }}
            />
          </Stack>
        ) : selected ? (
          <MeetingDetail
            id={selected}
            onDelete={(title) => requestDelete(selected, title)}
            {...(seekTo ? { seekTo } : {})}
          />
        ) : isCapturingPhase(session.phase) ? null : (
          <EmptyState title="no meeting selected" />
        )}
      </Stack>
    </SplitView>
  );
}
