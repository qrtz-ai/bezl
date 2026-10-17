import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Heading,
  Spinner,
  Tabs,
  Text,
  useTheme,
  Callout,
  Select,
  Dialog,
} from '@qrtz/ui';
import {
  SUMMARY_TEMPLATES,
  type MeetingDetail as Detail,
  type Segment,
  type SummaryTemplateId,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { PLATFORM_LABEL, fmtClock, fmtDate } from './format';
import { ChatPanel } from './ChatPanel';
import { Speakers } from './Speakers';
import { Notes } from './Notes';

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
  const t = useTheme();
  const [d, setD] = useState<Detail | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);
  const [tab, setTab] = useState<'summary' | 'notes' | 'transcript' | 'ask'>(
    'summary',
  );
  const [now, setNow] = useState(0);
  const [exported, setExported] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const changed = useQrtzEvent('memo:changed');

  useEffect(() => {
    let live = true;
    qrtz
      .invoke('memo:get', { id })
      .then((r) => {
        if (!live) return;
        setD(r);
        setLoadError(null);
      })
      .catch((e: Error) => {
        if (!live) return;
        setD(null);
        setLoadError(e.message || 'Could not load this meeting.');
      });
    return () => {
      live = false;
    };
  }, [qrtz, id, changed, reloadKey]);

  const retrySummary = useCallback(() => {
    setRetrying(true);
    setRetryError(null);
    qrtz
      .invoke('memo:resummarize', { id })
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
        .invoke('memo:set-template', { id, template })
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
        title="Couldn't load this meeting"
        data-testid="detail-load-error"
        action={
          <Button
            variant="secondary"
            onClick={() => setReloadKey((k) => k + 1)}
          >
            Try again
          </Button>
        }
      >
        {loadError}
      </Callout>
    );
  if (!d) return <Spinner />;
  const { meeting, segments, summary } = d;
  const doExport = async (save: boolean) => {
    const r = await qrtz.invoke('memo:export', { id, save });
    setExported(
      save
        ? r.savedTo
          ? `Saved to ${r.savedTo}`
          : 'Export canceled'
        : `Copied ${r.filename} to clipboard`,
    );
  };
  const audioName = meeting.audioPath?.split(/[\\/]/).pop();

  return (
    <div
      data-testid="meeting-detail"
      style={{ display: 'grid', gap: 16, alignContent: 'start' }}
    >
      <div style={{ display: 'grid', gap: 6 }}>
        <Heading level={2}>{meeting.title}</Heading>
        <div
          style={{
            display: 'flex',
            gap: 8,
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          <Badge>{PLATFORM_LABEL[meeting.platform]}</Badge>
          <Text muted size="sm">
            {fmtDate(meeting.startedAt)}
            {meeting.durationMs ? ` · ${fmtClock(meeting.durationMs)}` : ''}
            {meeting.participants.length
              ? ` · ${meeting.participants.join(', ')}`
              : ''}
          </Text>
          {meeting.status !== 'done' ? (
            <Badge tone="warning">{meeting.status}</Badge>
          ) : null}
        </div>
      </div>
      {audioName && meeting.status === 'done' ? (
        <audio
          ref={audio}
          controls
          src={`qrtz-media://rec/${audioName}`}
          onTimeUpdate={(e) => setNow(e.currentTarget.currentTime * 1000)}
          style={{ width: '100%', height: 32 }}
        />
      ) : null}
      <div style={{ display: 'flex', gap: 8 }}>
        <Button
          size="sm"
          variant="secondary"
          onClick={() => void doExport(true)}
          data-testid="export-save"
        >
          Export .md
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => void doExport(false)}
          data-testid="export-copy"
        >
          Copy Markdown
        </Button>
        <div style={{ flex: 1 }} />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => setConfirmDelete(true)}
          style={{ color: t.color.danger }}
          data-testid="meeting-delete"
        >
          Delete
        </Button>
      </div>
      <Dialog
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title="Delete this meeting?"
        label="Confirm delete"
      >
        <div style={{ display: 'grid', gap: 16 }} data-testid="confirm-delete">
          <Text size="sm" muted>
            “{meeting.title}” and its transcript, notes and recording will be
            deleted. You can undo for a few seconds.
          </Text>
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
            <Button
              variant="ghost"
              onClick={() => setConfirmDelete(false)}
              data-testid="confirm-delete-cancel"
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConfirmDelete(false);
                onDelete(meeting.title);
              }}
              data-testid="confirm-delete-ok"
            >
              Delete
            </Button>
          </div>
        </div>
      </Dialog>
      {exported ? (
        <Text size="sm" muted data-testid="export-result">
          {exported}
        </Text>
      ) : null}
      <Speakers meetingId={id} segments={segments} />
      <Tabs
        tabs={[
          { id: 'summary', label: 'Summary' },
          { id: 'notes', label: `Notes (${d?.notes?.length ?? 0})` },
          { id: 'transcript', label: `Transcript (${segments.length})` },
          { id: 'ask', label: 'Ask' },
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
          <div
            style={{ display: 'grid', gap: 16 }}
            data-testid="detail-summary"
          >
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <label htmlFor="summary-template">
                <Text size="sm" muted>
                  Template
                </Text>
              </label>
              <Select
                id="summary-template"
                data-testid="summary-template"
                value={d?.meeting.template ?? 'general'}
                disabled={retrying}
                onChange={(e) =>
                  changeTemplate(e.target.value as SummaryTemplateId)
                }
                style={{ width: 180 }}
              >
                {SUMMARY_TEMPLATES.map((tp) => (
                  <option key={tp.id} value={tp.id}>
                    {tp.label}
                  </option>
                ))}
              </Select>
              <Button
                size="sm"
                variant="ghost"
                onClick={retrySummary}
                disabled={retrying}
                data-testid="summary-rerun"
              >
                {retrying ? 'Summarizing…' : 'Re-run'}
              </Button>
              {retryError && !summary.llmError ? (
                <Text size="sm" style={{ color: t.color.danger }} role="alert">
                  {retryError}
                </Text>
              ) : null}
            </div>
            {summary.llmError ? (
              <Callout
                tone="warning"
                title="AI summary failed. Showing the on-device summary instead."
                data-testid="llm-error"
                action={
                  <Button
                    variant="secondary"
                    onClick={retrySummary}
                    disabled={retrying}
                    data-testid="llm-retry"
                  >
                    {retrying ? 'Retrying…' : 'Retry'}
                  </Button>
                }
              >
                {retryError ?? summary.llmError}
              </Callout>
            ) : null}
            <Card padding={4}>
              <Text as="p" style={{ lineHeight: 1.6 }}>
                {summary.summary}
              </Text>
            </Card>
            <div style={{ display: 'grid', gap: 8 }}>
              <Text weight={600}>Action items</Text>
              {summary.actionItems.length ? (
                summary.actionItems.map((a, i) => (
                  <div
                    key={i}
                    data-testid="action-item"
                    style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}
                  >
                    <span aria-hidden>☐</span>
                    <Text style={{ flex: 1 }}>{a.text}</Text>
                    {a.owner ? <Badge tone="neutral">{a.owner}</Badge> : null}
                  </div>
                ))
              ) : (
                <Text muted>None found.</Text>
              )}
            </div>
            {summary.qa.length ? (
              <div style={{ display: 'grid', gap: 8 }}>
                <Text weight={600}>Q&amp;A</Text>
                {summary.qa.map((q, i) => (
                  <Card key={i} padding={3}>
                    <Text weight={500}>Q: {q.question}</Text>
                    <br />
                    <Text muted>A: {q.answer}</Text>
                  </Card>
                ))}
              </div>
            ) : null}
            <Text size="xs" muted>
              Summary by{' '}
              {summary.provider === 'extractive'
                ? 'on-device extractive summarizer (add an LLM key in Settings for AI summaries)'
                : summary.provider}
            </Text>
          </div>
        ) : (
          <EmptyState
            icon="…"
            title="No summary yet"
            body="It will appear here when processing finishes."
          />
        )
      ) : (
        <div
          data-testid="detail-transcript"
          role="list"
          aria-label="Transcript"
          style={{ display: 'grid', gap: 2 }}
        >
          {segments.map((s, i) => (
            <SegmentRow key={s.id} s={s} active={i === current} onSeek={seek} />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One transcript line. Memoized so audio timeupdates re-render only the two rows whose `active` flips, and
 * `content-visibility: auto` lets Chromium skip layout/paint for off-screen rows (native virtualization that
 * keeps variable-height wrapped text and Ctrl+F working on 2-hour meetings).
 */
const SegmentRow = memo(function SegmentRow({
  s,
  active,
  onSeek,
}: {
  s: Segment;
  active: boolean;
  onSeek: (ms: number) => void;
}) {
  const t = useTheme();
  const who = s.speakerName ?? s.speaker ?? 'Speaker';
  return (
    <div
      role="listitem"
      tabIndex={0}
      aria-current={active || undefined}
      aria-label={`${fmtClock(s.startMs)} ${who}: ${s.text}`}
      data-testid="transcript-segment"
      data-active={active || undefined}
      onClick={() => onSeek(s.startMs)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onSeek(s.startMs);
        }
      }}
      style={{
        display: 'grid',
        gridTemplateColumns: '48px 1fr',
        gap: 8,
        padding: '6px 8px',
        borderRadius: t.radius.md,
        cursor: 'pointer',
        background: active ? t.color.surfaceHover : 'transparent',
        contentVisibility: 'auto',
        containIntrinsicSize: 'auto 56px',
      }}
    >
      <Text size="xs" muted mono>
        {fmtClock(s.startMs)}
      </Text>
      <div>
        <Text size="sm" weight={600}>
          {who}
        </Text>
        <Text as="p" size="sm">
          {s.text}
        </Text>
      </div>
    </div>
  );
});
