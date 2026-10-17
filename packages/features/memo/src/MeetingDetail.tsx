import { useEffect, useMemo, useRef, useState } from 'react';
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
} from '@qrtz/ui';
import type { MeetingDetail as Detail } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import { PLATFORM_LABEL, fmtClock, fmtDate } from './format';

export function MeetingDetail({
  id,
  onDeleted,
}: {
  id: string;
  onDeleted: () => void;
}) {
  const qrtz = useQrtz();
  const t = useTheme();
  const [d, setD] = useState<Detail | null>(null);
  const [tab, setTab] = useState<'summary' | 'transcript'>('summary');
  const [now, setNow] = useState(0);
  const [exported, setExported] = useState<string | null>(null);
  const audio = useRef<HTMLAudioElement>(null);
  const changed = useQrtzEvent('memo:changed');

  useEffect(() => {
    let live = true;
    qrtz
      .invoke('memo:get', { id })
      .then((r) => live && setD(r))
      .catch(() => live && setD(null));
    return () => {
      live = false;
    };
  }, [qrtz, id, changed]);

  const current = useMemo(() => {
    if (!d) return -1;
    let idx = -1;
    d.segments.forEach((s, i) => {
      if (s.startMs <= now) idx = i;
    });
    return idx;
  }, [d, now]);

  if (!d) return <Spinner />;
  const { meeting, segments, summary } = d;
  const seek = (ms: number) => {
    if (audio.current) {
      audio.current.currentTime = ms / 1000;
      void audio.current.play().catch(() => {});
    }
    setNow(ms);
  };
  const doExport = async (save: boolean) => {
    const r = await qrtz.invoke('memo:export', { id, save });
    if (!save) await navigator.clipboard?.writeText(r.markdown).catch(() => {});
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
        <Button size="sm" variant="ghost" onClick={() => void doExport(false)}>
          Copy Markdown
        </Button>
        <div style={{ flex: 1 }} />
        <Button
          size="sm"
          variant="ghost"
          onClick={() =>
            void qrtz.invoke('memo:delete', { id }).then(onDeleted)
          }
          style={{ color: t.color.danger }}
        >
          Delete
        </Button>
      </div>
      {exported ? (
        <Text size="sm" muted data-testid="export-result">
          {exported}
        </Text>
      ) : null}
      <Tabs
        tabs={[
          { id: 'summary', label: 'Summary' },
          { id: 'transcript', label: `Transcript (${segments.length})` },
        ]}
        value={tab}
        onChange={setTab}
      />
      {tab === 'summary' ? (
        summary ? (
          <div
            style={{ display: 'grid', gap: 16 }}
            data-testid="detail-summary"
          >
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
          style={{ display: 'grid', gap: 2 }}
        >
          {segments.map((s, i) => (
            <div
              key={s.id}
              data-testid="transcript-segment"
              data-active={i === current || undefined}
              onClick={() => seek(s.startMs)}
              style={{
                display: 'grid',
                gridTemplateColumns: '48px 1fr',
                gap: 8,
                padding: '6px 8px',
                borderRadius: t.radius.md,
                cursor: 'pointer',
                background:
                  i === current ? t.color.surfaceHover : 'transparent',
              }}
            >
              <Text size="xs" muted mono>
                {fmtClock(s.startMs)}
              </Text>
              <div>
                <Text size="sm" weight={600}>
                  {s.speakerName ?? s.speaker ?? 'Speaker'}
                </Text>
                <Text as="p" size="sm">
                  {s.text}
                </Text>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
