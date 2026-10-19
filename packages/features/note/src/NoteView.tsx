import { lazy, Suspense, useCallback, useEffect, useState } from 'react';
import {
  Button,
  Callout,
  CaptureStatus,
  EmptyState,
  IconButton,
  Input,
  MeetingRow,
  Page,
  SplitView,
  Spinner,
  Stack,
  useToast,
  SpeechNotice,
  ModuleSettingsButton,
  ModuleSettingsPage,
} from '@qrtz/ui';
import {
  AppError,
  SessionView,
  meetingPath,
  parseMeetingPath,
  requestNavigate,
  SETTINGS_PATH,
  type MeetingTab,
} from '@qrtz/core';
import {
  useQrtz,
  useQrtzEvent,
  useSpeechSetup,
  useSubAppRoute,
} from '@qrtz/core/react';
import { Hud } from './Hud';
import { isCapturingPhase, useSession } from './useRecording';
import { fmtDate } from './format';
import NoteSettings from './NoteSettings';

// The detail (transcript, summary, notes, player) isn't needed to paint the list: its own chunk, preloaded
// as soon as the list mounts so opening a meeting never waits on it (ADR 0093).
const loadDetail = () => import('./MeetingDetail');
const MeetingDetail = lazy(() =>
  loadDetail().then((m) => ({ default: m.MeetingDetail })),
);

const UNDO_MS = 6000;

const PAGE = 100;

export default function NoteView() {
  // ADR 0146 M1.4: this module's own settings open here, not in the global settings.
  const [settingsOpen, setSettingsOpen] = useState(false);
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
  // The open meeting and its tab are the route (#/note/meetings/:id/:tab?, ADR 0091), so deep links, the
  // palette and notifications open them and the tab bar returns to them.
  useEffect(() => {
    void loadDetail();
  }, []);
  const route = useSubAppRoute();
  const { meetingId: selected, tab } = parseMeetingPath(route.path);
  // Opening a meeting (list, palette, hud) closes nano's settings; `settings` (the module manager, ADR 0161) opens
  // them.
  useEffect(() => setSettingsOpen(route.path === SETTINGS_PATH), [route.path]);
  const { navigate } = route;
  const setSelected = useCallback(
    (id: string | null, opts?: { replace?: boolean }) =>
      navigate(id ? meetingPath(id) : '', opts),
    [navigate],
  );
  const setTab = useCallback(
    (t: MeetingTab) => selected && navigate(meetingPath(selected, t)),
    [navigate, selected],
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
    if (liveId) setSelected(liveId, { replace: true });
  }, [liveId, setSelected]);

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
    <Page
      data-testid="note-app"
      title="nano"
      actions={
        <>
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
          <ModuleSettingsButton
            module="nano"
            onClick={() => setSettingsOpen(true)}
            data-testid="note-settings-open"
          />
        </>
      }
    >
      <SplitView
        sidebar={
          <>
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
              onOpen={() => requestNavigate({ app: 'qrtz', item: 'nano' })}
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
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setError(null)}
                >
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
          {settingsOpen ? (
            // ADR 0146 M1.4: nano's settings open in the detail pane, so the meeting list, the hud and the record
            // prompt stay where they are.
            <ModuleSettingsPage
              module="nano"
              inset
              onClose={() => {
                setSettingsOpen(false);
                if (route.path === SETTINGS_PATH)
                  route.navigate('', { replace: true });
              }}
              data-testid="note-settings"
            >
              <NoteSettings />
            </ModuleSettingsPage>
          ) : selected ? (
            <Suspense fallback={<Spinner />}>
              <MeetingDetail
                key={selected}
                id={selected}
                tab={tab}
                onTab={setTab}
                onDelete={(title) => requestDelete(selected, title)}
              />
            </Suspense>
          ) : isCapturingPhase(session.phase) ? null : (
            <EmptyState title="no meeting selected" />
          )}
        </Stack>
      </SplitView>
    </Page>
  );
}
