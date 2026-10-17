import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTheme } from './theme';
import { Button } from './Button';
import { Text, type TextTone } from './Text';

/**
 * Recording-lifecycle presentation (docs/recording-lifecycle.md). Pure views: all state comes from props,
 * which features derive from main's typed IPC (`*:session`, `*:pipeline`, `*:transcript`).
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

function StreamRow({
  ms,
  speaker,
  children,
  partial,
  ...rest
}: {
  ms: number;
  speaker?: string | undefined;
  children: ReactNode;
  partial?: boolean;
  'data-testid': string;
  'data-id': string;
}) {
  const t = useTheme();
  return (
    <div
      {...rest}
      style={{
        display: 'grid',
        gridTemplateColumns: '56px 1fr',
        gap: t.space[2],
        padding: `${t.space[1]} ${t.space[2]}`,
      }}
    >
      <Text variant="mono" muted>
        {clock(ms)}
      </Text>
      <div>
        {speaker ? (
          <Text variant="label" tone="muted">
            {speaker}
          </Text>
        ) : null}
        <Text as="p" {...(partial ? { tone: 'muted' as TextTone } : {})}>
          {children}
        </Text>
      </div>
    </div>
  );
}

/**
 * The live transcript, front and center. Finalized segments plus the volatile partial (rendered quieter and
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
          display: 'grid',
          gap: 2,
          alignContent: 'start',
        }}
      >
        {segments.length === 0 && !partial?.text
          ? (empty ?? null)
          : segments.map((s) => (
              <StreamRow
                key={s.id}
                ms={s.startMs}
                speaker={s.speaker}
                data-testid="live-segment"
                data-id={s.id}
              >
                {s.text}
              </StreamRow>
            ))}
        {live && partial?.text ? (
          <StreamRow
            key={`partial-${partial.id}`}
            ms={partial.startMs ?? segments.at(-1)?.startMs ?? 0}
            partial
            data-testid="live-partial"
            data-id={partial.id}
          >
            {partial.text}
          </StreamRow>
        ) : null}
      </div>
      {live && !follow ? (
        <div
          style={{
            position: 'absolute',
            bottom: t.space[2],
            left: 0,
            right: 0,
            display: 'flex',
            justifyContent: 'center',
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

const STATUS: Record<PipelineStepStatus, { tone: TextTone; label: string }> = {
  pending: { tone: 'muted', label: 'waiting' },
  running: { tone: 'default', label: 'running' },
  retrying: { tone: 'warning', label: 'retrying' },
  done: { tone: 'success', label: 'done' },
  failed: { tone: 'danger', label: 'failed' },
  skipped: { tone: 'muted', label: 'skipped' },
};

/** Post-processing progress, one quiet row per step, with Retry on failed steps. */
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
            gap: t.space[3],
            minHeight: 28,
          }}
        >
          <Text style={{ minWidth: 120 }}>{s.label}</Text>
          <Text variant="caption" tone={STATUS[s.status].tone}>
            {s.status === 'failed' && s.fallback
              ? 'on-device fallback'
              : STATUS[s.status].label}
            {s.status === 'retrying' ? ` ${s.attempts}/${s.maxAttempts}` : ''}
          </Text>
          <div style={{ flex: 1, minWidth: 0 }}>
            {s.error && (s.status === 'failed' || s.status === 'retrying') ? (
              <Text variant="caption" muted truncate title={s.error}>
                {s.error}
              </Text>
            ) : null}
          </div>
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

/** Capture health while recording: reconnecting, degraded modes, or a refused start. One line each. */
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
        gap: t.space[3],
      }}
    >
      {reconnecting ? (
        <Text
          variant="caption"
          tone="warning"
          data-testid="lifecycle-reconnecting"
        >
          Reconnecting · attempt {reconnecting.attempt} ·{' '}
          {reconnecting.secondsLeft}s
        </Text>
      ) : null}
      {degraded.map((d) => (
        <Text
          key={d.id}
          variant="caption"
          tone="warning"
          data-testid={`degraded-${d.id}`}
        >
          {d.label}
        </Text>
      ))}
      {error ? (
        <span
          data-testid="lifecycle-error"
          role="alert"
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: t.space[2],
          }}
        >
          <Text tone="danger">{error}</Text>
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
