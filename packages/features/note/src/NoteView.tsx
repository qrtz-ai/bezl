import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Callout,
  CaptureStatus,
  EmptyState,
  IconButton,
  Input,
  MeetingRow,
  SplitView,
  Stack,
  Toolbar,
  useToast,
  SpeechNotice,
} from '@qrtz/ui';
import {
  AppError,
  SessionView,
  onNavigateRequest,
  takePendingMeeting,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent, useSpeechSetup } from '@qrtz/core/react';
import { requestNavigate } from '@qrtz/core';
import { Hud } from './Hud';
import { isCapturingPhase, useSession } from './useRecording';
import { MeetingDetail } from './MeetingDetail';
import { fmtDate } from './format';

const UNDO_MS = 6000;

const PAGE = 100;

export default function NoteView() {
  const qrtz = useQrtz();
  const speechSetup = useSpeechSetup({ check: false });
  // Lifecycle state comes only from main (note:session); the view never guesses.
  const session = useSession();
  const [query, setQuery] = useState('');
  const [searching, setSearching] = useState(false);
  // Paging: render PAGE meetings at a time; "Show more" grows the window (list stays cheap with hundreds).
  const [pages, setPages] = useState(1);
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
    toast(`deleted “${title}”`, 'info', {
      durationMs: UNDO_MS,
      action: {
        label: 'undo',
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
  const changed = useQrtzEvent('note:changed');

  // A new recording opens its meeting so the live transcript is front and center.
  const liveId = isCapturingPhase(session.phase)
    ? session.meetingId
    : undefined;
  useEffect(() => {
    if (liveId) setSelected(liveId);
  }, [liveId]);

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
                  label="search meetings"
                  pressed={searching}
                  data-testid="note-search-toggle"
                  onClick={() => {
                    if (searching) setQuery('');
                    setSearching(!searching);
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
                record
              </Button>
            )}
          </Toolbar>
          {searching ? (
            <Input
              autoFocus
              placeholder="search"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setPages(1);
              }}
              data-testid="note-search"
              aria-label="search meetings"
            />
          ) : null}
          <SpeechNotice
            status={speechSetup.speech}
            onOpenSettings={() => requestNavigate({ app: 'settings' })}
          />
          <Stack as="ul" gap={0} scroll grow aria-label="meetings">
            {visible.length === 0 ? (
              <Stack as="li">
                <EmptyState title={query ? 'no matches' : 'no meetings'} />
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
                  onSelect={() => setSelected(meeting.id)}
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
                  more
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
                ? 'recording unavailable: capture helper missing'
                : 'capture helper stopped, restarting'
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
                dismiss
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
        {selected ? (
          <MeetingDetail
            id={selected}
            onDelete={(title) => requestDelete(selected, title)}
          />
        ) : isCapturingPhase(session.phase) ? null : (
          <EmptyState title="no meeting selected" />
        )}
      </Stack>
    </SplitView>
  );
}
