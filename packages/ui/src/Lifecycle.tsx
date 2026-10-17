import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTheme } from './theme';
import { Button } from './Button';
import { Badge, type BadgeTone } from './Badge';

/**
 * Recording-lifecycle presentation (docs/recording-lifecycle.md). Pure views: all state comes from props,
 * which features derive from main's typed IPC (`memo:session`, `memo:pipeline`, `memo:transcript`).
 */

export interface StreamSegment {
  id: string;
  text: string;
  startMs: number;
  speaker?: string;
}

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
};

/**
 * The transcript, front and center. Finalized segments plus the volatile partial (rendered distinctly and
 * replaced in place). Follows the bottom while live; scrolling up pauses following and shows "Jump to live".
 */
export function StreamingTranscript({
  segments,
  partial,
  live,
  empty,
  maxHeight,
  testId = 'live-transcript',
}: {
  segments: StreamSegment[];
  partial?: { id: string; text: string; startMs?: number } | null;
  live: boolean;
  empty?: ReactNode;
  maxHeight?: string | number;
  testId?: string;
}) {
  const t = useTheme();
  const ref = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  const last = segments.at(-1)?.id;
  useLayoutEffect(() => {
    const el = ref.current;
    if (el && follow) el.scrollTop = el.scrollHeight;
  }, [last, partial?.text, follow, segments.length]);
  const onScroll = () => {
    const el = ref.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    if (atBottom !== follow) setFollow(atBottom);
  };
  const row = (
    key: string,
    ms: number,
    speaker: string | undefined,
    body: ReactNode,
    extra: object,
  ) => (
    <div
      key={key}
      {...extra}
      style={{
        display: 'grid',
        gridTemplateColumns: '44px 1fr',
        gap: t.space[3],
        padding: `${t.space[1]} 0`,
      }}
    >
      <span
        style={{
          color: t.color.textSubtle,
          fontSize: t.font.size.xs,
          fontFamily: t.font.mono,
          paddingTop: 2,
        }}
      >
        {clock(ms)}
      </span>
      <span
        style={{
          fontSize: t.font.size.md,
          lineHeight: 1.55,
          color: t.color.text,
        }}
      >
        {speaker ? (
          <span
            style={{
              fontWeight: t.font.weight.semibold,
              marginRight: t.space[2],
            }}
          >
            {speaker}
          </span>
        ) : null}
        {body}
      </span>
    </div>
  );
  return (
    <div style={{ position: 'relative' }}>
      <div
        ref={ref}
        onScroll={onScroll}
        data-testid={testId}
        data-live={live ? '' : undefined}
        data-following={follow ? '' : undefined}
        aria-live={live ? 'polite' : undefined}
        style={{
          overflowY: 'auto',
          maxHeight: maxHeight ?? '60vh',
          paddingRight: t.space[2],
        }}
      >
        {segments.length === 0 && !partial?.text
          ? (empty ?? null)
          : segments.map((s) =>
              row(s.id, s.startMs, s.speaker, s.text, {
                'data-testid': 'live-segment',
                'data-id': s.id,
              }),
            )}
        {live && partial?.text
          ? row(
              `partial-${partial.id}`,
              partial.startMs ?? segments.at(-1)?.startMs ?? 0,
              undefined,
              <span style={{ color: t.color.textMuted, fontStyle: 'italic' }}>
                {partial.text}
              </span>,
              { 'data-testid': 'live-partial', 'data-id': partial.id },
            )
          : null}
      </div>
      {live && !follow ? (
        <div
          style={{
            position: 'absolute',
            bottom: t.space[2],
            left: 0,
            right: 0,
            textAlign: 'center',
          }}
        >
          <Button
            size="sm"
            variant="secondary"
            data-testid="jump-to-live"
            onClick={() => {
              setFollow(true);
              const el = ref.current;
              if (el) el.scrollTop = el.scrollHeight;
            }}
          >
            Jump to live
          </Button>
        </div>
      ) : null}
    </div>
  );
}

export type PipelineStepStatus =
  | 'pending'
  | 'running'
  | 'retrying'
  | 'done'
  | 'failed'
  | 'skipped';
export interface PipelineStepView {
  id: string;
  label: string;
  status: PipelineStepStatus;
  attempts: number;
  maxAttempts: number;
  error?: string;
  fallback?: boolean;
}

const STATUS: Record<PipelineStepStatus, { tone: BadgeTone; label: string }> = {
  pending: { tone: 'neutral', label: 'waiting' },
  running: { tone: 'primary', label: 'running' },
  retrying: { tone: 'warning', label: 'retrying' },
  done: { tone: 'success', label: 'done' },
  failed: { tone: 'danger', label: 'failed' },
  skipped: { tone: 'neutral', label: 'skipped' },
};

/** Post-processing progress, one row per step, with a Retry on failed steps. */
export function PipelineSteps({
  steps,
  onRetry,
  testId = 'pipeline',
}: {
  steps: PipelineStepView[];
  onRetry?: (id: string) => void;
  testId?: string;
}) {
  const t = useTheme();
  return (
    <div
      data-testid={testId}
      style={{ display: 'flex', flexDirection: 'column', gap: t.space[1] }}
    >
      {steps.map((s) => (
        <div
          key={s.id}
          data-testid={`pipeline-step-${s.id}`}
          data-status={s.status}
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: t.space[2],
            fontSize: t.font.size.sm,
            minHeight: 28,
          }}
        >
          <span style={{ minWidth: 120, color: t.color.text }}>{s.label}</span>
          <Badge tone={STATUS[s.status].tone}>
            {s.status === 'failed' && s.fallback
              ? 'on-device fallback'
              : STATUS[s.status].label}
            {s.status === 'retrying' ? ` ${s.attempts}/${s.maxAttempts}` : ''}
          </Badge>
          {s.error && (s.status === 'failed' || s.status === 'retrying') ? (
            <span
              title={s.error}
              style={{
                color: t.color.textMuted,
                fontSize: t.font.size.xs,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
                flex: 1,
              }}
            >
              {s.error}
            </span>
          ) : (
            <span style={{ flex: 1 }} />
          )}
          {s.status === 'failed' && onRetry ? (
            <Button
              size="sm"
              variant="secondary"
              data-testid={`pipeline-retry-${s.id}`}
              onClick={() => onRetry(s.id)}
            >
              Retry
            </Button>
          ) : null}
        </div>
      ))}
    </div>
  );
}

/** Capture health while recording: reconnecting, degraded modes, or a refused start. */
export function CaptureStatus({
  reconnecting,
  degraded,
  error,
  onDismiss,
}: {
  reconnecting?: { attempt: number; secondsLeft: number } | null;
  degraded: { id: string; label: string }[];
  error?: string | null;
  onDismiss?: () => void;
}) {
  const t = useTheme();
  if (!reconnecting && !degraded.length && !error) return null;
  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        alignItems: 'center',
        gap: t.space[2],
      }}
    >
      {reconnecting ? (
        <span data-testid="lifecycle-reconnecting">
          <Badge tone="warning">
            Reconnecting · attempt {reconnecting.attempt} ·{' '}
            {reconnecting.secondsLeft}s
          </Badge>
        </span>
      ) : null}
      {degraded.map((d) => (
        <span key={d.id} data-testid={`degraded-${d.id}`}>
          <Badge tone="warning">{d.label}</Badge>
        </span>
      ))}
      {error ? (
        <span
          data-testid="lifecycle-error"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: t.space[2],
            color: t.color.danger,
            fontSize: t.font.size.sm,
          }}
        >
          {error}
          {onDismiss ? (
            <Button
              size="sm"
              variant="ghost"
              data-testid="lifecycle-error-dismiss"
              onClick={onDismiss}
            >
              Dismiss
            </Button>
          ) : null}
        </span>
      ) : null}
    </div>
  );
}
