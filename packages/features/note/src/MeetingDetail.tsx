import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ActionMenu,
  AudioPlayer,
  Button,
  Callout,
  CaptureStatus,
  ConfirmDialog,
  PipelineSteps,
  StreamingTranscript,
  EmptyState,
  Heading,
  IconButton,
  Select,
  Spinner,
  Stack,
  SummaryBlock,
  Tabs,
  Text,
  Toolbar,
  TranscriptList,
  TranscriptRow,
} from '@qrtz/ui';
import {
  SUMMARY_TEMPLATES,
  type MeetingDetail as Detail,
  type Segment as SegmentT,
  type SummaryTemplateId,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { PLATFORM_LABEL, fmtClock, fmtDate } from './format';
import { ChatPanel } from './ChatPanel';
import { Speakers } from './Speakers';
import { Notes } from './Notes';
import {
  DEGRADED_LABELS,
  STEP_LABELS,
  isCapturingPhase,
  useLiveTranscript,
  usePipeline,
  useSession,
} from './useRecording';

export function MeetingDetail({
  id,
  onDelete,
  seekTo,
}: {
  id: string;
  /** Asks the parent to delete (after the user confirmed); the parent offers Undo. */
  onDelete: (title: string) => void;
  /** Open on the transcript at this moment (e.g. from a chat citation). */
  seekTo?: { ms: number; nonce: number };
}) {
  const qrtz = useQrtz();
  const [d, setD] = useState<Detail | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  // The transcript is front and center: it is the default view during and after a recording.
  const [tab, setTab] = useState<'summary' | 'notes' | 'transcript' | 'ask'>(
    'transcript',
  );
  const session = useSession();
  const isLive =
    session.meetingId === id &&
    (isCapturingPhase(session.phase) || session.phase === 'stopping');
  const live = useLiveTranscript(isLive ? id : null);
  const { pipeline, retry } = usePipeline(id);
  const [now, setNow] = useState(0);
  const [exported, setExported] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const changed = useQrtzEvent('note:changed');

  useEffect(() => {
    let live = true;
    qrtz
      .invoke('note:get', { id })
      .then((r) => {
        if (!live) return;
        setD(r);
        setLoadError(null);
      })
      .catch((e: Error) => {
        if (!live) return;
        setD(null);
        setLoadError(e.message || 'could not load this meeting.');
      });
    return () => {
      live = false;
    };
  }, [qrtz, id, changed, reloadKey]);

  const retrySummary = useCallback(() => {
    setRetrying(true);
    setRetryError(null);
    qrtz
      .invoke('note:resummarize', { id })
      .then((s) => setD((prev) => (prev ? { ...prev, summary: s } : prev)))
      .catch((e: Error) => setRetryError(e.message))
      .finally(() => setRetrying(false));
  }, [qrtz, id]);

  const changeTemplate = useCallback(
    (template: SummaryTemplateId) => {
      setRetrying(true);
      setRetryError(null);
      setD((prev) =>
        prev ? { ...prev, meeting: { ...prev.meeting, template } } : prev,
      );
      qrtz
        .invoke('note:set-template', { id, template })
        .then((s) => setD((prev) => (prev ? { ...prev, summary: s } : prev)))
        .catch((e: Error) => setRetryError(e.message))
        .finally(() => setRetrying(false));
    },
    [qrtz, id],
  );

  // Binary search: O(log n) per audio timeupdate even for multi-hour transcripts.
  const current = useMemo(() => {
    if (!d) return -1;
    let lo = 0;
    let hi = d.segments.length - 1;
    let idx = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (d.segments[mid]!.startMs <= now) {
        idx = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    return idx;
  }, [d, now]);

  const seek = useCallback((ms: number) => {
    if (audio.current) {
      audio.current.currentTime = ms / 1000;
      void audio.current.play().catch(() => {});
    }
    setNow(ms);
  }, []);

  // Jump to a cited moment once the meeting is loaded.
  const loaded = !!d;
  useEffect(() => {
    if (!seekTo || !loaded) return;
    setTab('transcript');
    setNow(seekTo.ms);
    requestAnimationFrame(() =>
      document
        .querySelector('[data-testid="transcript-segment"][data-active]')
        ?.scrollIntoView({ block: 'center' }),
    );
  }, [seekTo, loaded]);

  if (!d && loadError)
    return (
      <Callout
        tone="danger"
        title={loadError}
        data-testid="detail-load-error"
        action={
          <Button
            variant="secondary"
            onClick={() => setReloadKey((k) => k + 1)}
          >
            try again
          </Button>
        }
      />
    );
  if (isLive)
    return (
      <Stack gap={3} data-testid="meeting-detail" data-live>
        <Heading level={2} preserveCase>
          {d?.meeting.title ?? session.detected?.title ?? 'recording'}
        </Heading>
        <CaptureStatus
          reconnecting={
            session.reconnect
              ? {
                  attempt: session.reconnect.attempt,
                  secondsLeft: Math.max(
                    0,
                    Math.round(
                      (session.reconnect.deadlineAt - Date.now()) / 1000,
                    ),
                  ),
                }
              : null
          }
          degraded={session.degraded.map((f) => ({
            id: f,
            label: DEGRADED_LABELS[f],
          }))}
        />
        <StreamingTranscript
          live={session.phase !== 'stopping'}
          segments={live?.segments ?? []}
          partial={live?.partial ?? null}
          maxHeight="calc(100vh - 300px)"
          empty={<Text muted>listening…</Text>}
        />
      </Stack>
    );
  if (!d) return <Spinner />;
  const { meeting, segments, summary } = d;
  const doExport = async (save: boolean) => {
    const r = await qrtz.invoke('note:export', { id, save });
    setExported(
      save
        ? r.savedTo
          ? `saved to ${r.savedTo}`
          : 'export canceled'
        : `copied ${r.filename}`,
    );
  };
  const audioName = meeting.audioPath?.split(/[\\/]/).pop();
  const meta = [
    fmtDate(meeting.startedAt),
    meeting.durationMs ? fmtClock(meeting.durationMs) : '',
    meeting.participants.join(', '),
    meeting.status !== 'done' ? meeting.status : '',
  ]
    .filter(Boolean)
    .join(' · ');

  return (
    <Stack gap={5} data-testid="meeting-detail">
      <Stack gap={1}>
        <Toolbar
          end={
            <ActionMenu
              label="meeting actions"
              data-testid="meeting-menu"
              items={[
                {
                  label: 'export .md',
                  onSelect: () => void doExport(true),
                  testId: 'export-save',
                },
                {
                  label: 'copy markdown',
                  onSelect: () => void doExport(false),
                  testId: 'export-copy',
                },
                {
                  label: 'delete',
                  onSelect: () => setConfirmDelete(true),
                  danger: true,
                  testId: 'meeting-delete',
                },
              ]}
            />
          }
        >
          <Heading level={2} preserveCase>
            {meeting.title}
          </Heading>
        </Toolbar>
        <Text variant="caption" muted>
          {PLATFORM_LABEL[meeting.platform]} · {meta}
        </Text>
        {exported ? (
          <Text variant="caption" muted data-testid="export-result">
            {exported}
          </Text>
        ) : null}
      </Stack>
      <ConfirmDialog
        open={confirmDelete}
        title="delete this meeting?"
        body={meeting.title}
        confirmLabel="delete"
        data-testid="confirm-delete"
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => {
          setConfirmDelete(false);
          onDelete(meeting.title);
        }}
      />
      {pipeline &&
      !(
        ['done', 'skipped'].includes(pipeline.steps.finalize.status) &&
        ['done', 'skipped'].includes(pipeline.steps.summary.status) &&
        ['done', 'skipped'].includes(pipeline.steps.index.status)
      ) ? (
        <PipelineSteps
          steps={(['finalize', 'summary', 'index'] as const).map((k) => ({
            id: k,
            label: STEP_LABELS[k],
            status: pipeline.steps[k].status,
            attempts: pipeline.steps[k].attempts,
            maxAttempts: pipeline.steps[k].maxAttempts,
            ...(pipeline.steps[k].error
              ? { error: pipeline.steps[k].error!.message }
              : {}),
            ...(pipeline.steps[k].fallback ? { fallback: true } : {}),
          }))}
          onRetry={(step) => retry(step as 'finalize' | 'summary' | 'index')}
        />
      ) : null}
      <Speakers meetingId={id} segments={segments} />
      <Tabs
        tabs={[
          { id: 'transcript', label: 'transcript' },
          { id: 'summary', label: 'summary' },
          { id: 'notes', label: 'notes' },
          { id: 'ask', label: 'ask' },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === 'ask' ? (
        <ChatPanel
          meetingId={id}
          onOpenSource={(_m, ms) => {
            setTab('transcript');
            seek(ms);
          }}
        />
      ) : tab === 'notes' ? (
        <Notes meetingId={id} />
      ) : tab === 'summary' ? (
        summary ? (
          <SummaryBlock
            summary={summary.summary}
            actionItems={summary.actionItems.map((a) => ({
              text: a.text,
              ...(a.owner ? { owner: a.owner } : {}),
            }))}
            qa={summary.qa}
            source={
              summary.provider === 'extractive'
                ? 'on-device extractive'
                : summary.provider
            }
            toolbar={
              <Toolbar
                end={
                  <>
                    <Select
                      id="summary-template"
                      aria-label="template"
                      data-testid="summary-template"
                      value={d?.meeting.template ?? 'general'}
                      disabled={retrying}
                      onChange={(e) =>
                        changeTemplate(e.target.value as SummaryTemplateId)
                      }
                    >
                      {SUMMARY_TEMPLATES.map((tp) => (
                        <option key={tp.id} value={tp.id}>
                          {tp.label}
                        </option>
                      ))}
                    </Select>
                    <IconButton
                      icon="refresh"
                      label={retrying ? 'summarizing' : 're-run'}
                      onClick={retrySummary}
                      data-testid="summary-rerun"
                    />
                  </>
                }
              >
                {retryError && !summary.llmError ? (
                  <Text variant="caption" tone="danger" role="alert">
                    {retryError}
                  </Text>
                ) : null}
              </Toolbar>
            }
            notice={
              summary.llmError ? (
                <Callout
                  tone="warning"
                  title={retryError ?? summary.llmError}
                  data-testid="llm-error"
                  action={
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={retrySummary}
                      disabled={retrying}
                      data-testid="llm-retry"
                    >
                      {retrying ? 'retrying…' : 'retry'}
                    </Button>
                  }
                />
              ) : undefined
            }
          />
        ) : (
          <EmptyState
            title="no summary"
            {...(meeting.status === 'done'
              ? {
                  action: (
                    <Button
                      size="sm"
                      onClick={retrySummary}
                      disabled={retrying}
                      data-testid="summary-run"
                    >
                      summarize
                    </Button>
                  ),
                }
              : {})}
          />
        )
      ) : (
        <Stack gap={3}>
          {audioName && meeting.status === 'done' ? (
            <AudioPlayer
              ref={audio}
              src={`qrtz-media://rec/${audioName}`}
              onTime={setNow}
            />
          ) : null}
          <TranscriptList>
            {segments.map((s, i) => (
              <Segment key={s.id} s={s} active={i === current} onSeek={seek} />
            ))}
          </TranscriptList>
        </Stack>
      )}
    </Stack>
  );
}

function Segment({
  s,
  active,
  onSeek,
}: {
  s: SegmentT;
  active: boolean;
  onSeek: (ms: number) => void;
}) {
  const seekHere = useCallback(() => onSeek(s.startMs), [onSeek, s.startMs]);
  return (
    <TranscriptRow
      time={fmtClock(s.startMs)}
      speaker={s.speakerName ?? s.speaker ?? 'Speaker'}
      text={s.text}
      active={active}
      onSeek={seekHere}
    />
  );
}
