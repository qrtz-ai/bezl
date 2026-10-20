import type { ReactNode } from 'react';
import { useTheme } from './theme';
import { Button } from './Button';
import { Icon } from './Icon';
import { Progress } from './Progress';
import { Spinner } from './Spinner';
import { Text } from './Text';

/** 1.2 gb · 340 mb · 12 kb (lowercase, brand rule). */
export function formatBytes(n: number): string {
  if (n <= 0) return '0 mb';
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} gb`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} mb`;
  return `${Math.max(1, Math.round(n / 1e3))} kb`;
}

/**
 * Where a model an app needs stands. `unavailable`: there is nothing to download yet (not published, or not in
 * this build).
 */
export type ModelRequirementState =
  | 'missing'
  | 'downloading'
  | 'ready'
  | 'error'
  | 'unavailable';

export interface ModelRequirementProgress {
  done: number;
  total: number;
  /** `checking` / `installing` show a spinner instead of bytes; `paused` keeps the bar. */
  phase?: 'downloading' | 'paused' | 'checking' | 'installing';
}

type ActionId =
  | 'download'
  | 'cancel'
  | 'retry'
  | 'remove'
  | 'open'
  | 'status'
  | 'progress';

export interface ModelRequirementProps {
  /** "speech model", "decision model": lowercase, always ends in "model". */
  name: string;
  /** One short line on what the model does. */
  purpose?: string;
  /** Real size from the data; omitted when unknown (never hardcoded). */
  sizeBytes?: number | null;
  state: ModelRequirementState;
  /** downloading: bytes so far; omitted = an indeterminate spinner. */
  progress?: ModelRequirementProgress;
  /** error: the real reason. */
  error?: string | null;
  /** Optional model: what the app does without it ("ctrl decides with its built-in rules"). */
  fallback?: string;
  /** Extra muted line under the purpose (a consent note, which engine is deciding). */
  note?: ReactNode;
  layout?: 'card' | 'row';
  onDownload?: () => void;
  onCancel?: () => void;
  onRetry?: () => void;
  onRemove?: () => void;
  /** Opens where the app's models are managed. */
  onOpen?: () => void;
  downloadLabel?: string;
  cancelLabel?: string;
  removeLabel?: string;
  openLabel?: string;
  /** Extra controls after the standard ones (pause / resume). */
  actions?: ReactNode;
  'data-testid'?: string;
  /** Per-control test ids; default `${data-testid}-${id}`. */
  testIds?: Partial<Record<ActionId, string>>;
}

/** The status line: "not downloaded · 1.2 gb", "downloading · 42%", "on this mac · 1.2 gb". */
export function modelStatusLine({
  state,
  sizeBytes,
  progress,
  error,
}: Pick<
  ModelRequirementProps,
  'state' | 'sizeBytes' | 'progress' | 'error'
>): string {
  const size = sizeBytes ? ` · ${formatBytes(sizeBytes)}` : '';
  switch (state) {
    case 'missing':
      return `not downloaded${size}`;
    case 'ready':
      return `on this mac${size}`;
    case 'unavailable':
      return 'not published yet · nothing downloaded';
    case 'error':
      return `couldn't download${error ? ` · ${error}` : ''}`;
    case 'downloading': {
      const p = progress;
      if (p?.phase === 'checking') return 'checking…';
      if (p?.phase === 'installing') return 'installing…';
      const total = p?.total || sizeBytes || 0;
      if (!p || !total) return 'downloading…';
      const pct = Math.round((100 * p.done) / total);
      return `${p.phase === 'paused' ? 'paused' : 'downloading'} · ${pct}% · ${formatBytes(p.done)} of ${formatBytes(total)}`;
    }
  }
}

const ICON = {
  missing: 'download',
  downloading: 'download',
  ready: 'check',
  error: 'alert',
  unavailable: 'model',
} as const;

/**
 * The one view for "this app needs a model": missing (name, purpose, size, download), downloading (progress,
 * cancel), ready, error (the reason, retry) and optional (what happens without it). `card` sits inline in an
 * app's page; `row` fits a settings list or a side panel. Rows 12 apart inside, sections 24 apart outside.
 */
export function ModelRequirement(p: ModelRequirementProps) {
  const t = useTheme();
  const {
    name,
    purpose,
    state,
    layout = 'card',
    fallback,
    note,
    'data-testid': testId = 'model-requirement',
  } = p;
  const id = (k: ActionId) => p.testIds?.[k] ?? `${testId}-${k}`;
  const status = modelStatusLine(p);
  const busy = state === 'downloading';
  const pr = p.progress;
  const total = pr?.total || p.sizeBytes || 0;
  const determinate =
    busy && pr && total && pr.phase !== 'checking' && pr.phase !== 'installing';
  const tone =
    state === 'error' ? 'danger' : state === 'ready' ? 'success' : 'muted';

  const buttons = (
    <div
      style={{
        display: 'flex',
        gap: t.space[2],
        flexShrink: 0,
        alignItems: 'center',
      }}
    >
      {state === 'missing' && p.onDownload ? (
        <Button
          size="sm"
          icon="download"
          onClick={p.onDownload}
          data-testid={id('download')}
        >
          {p.downloadLabel ?? 'download'}
        </Button>
      ) : null}
      {state === 'error' && p.onRetry ? (
        <Button
          size="sm"
          variant="secondary"
          icon="refresh"
          onClick={p.onRetry}
          data-testid={id('retry')}
        >
          retry
        </Button>
      ) : null}
      {busy && p.onCancel ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={p.onCancel}
          data-testid={id('cancel')}
        >
          {p.cancelLabel ?? 'cancel'}
        </Button>
      ) : null}
      {p.actions}
      {p.onOpen ? (
        <Button
          size="sm"
          variant="secondary"
          icon="model"
          onClick={p.onOpen}
          data-testid={id('open')}
        >
          {p.openLabel ?? 'models'}
        </Button>
      ) : null}
      {!busy && p.onRemove ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={p.onRemove}
          data-testid={id('remove')}
        >
          {p.removeLabel ?? 'remove'}
        </Button>
      ) : null}
    </div>
  );

  const head = (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: t.space[3],
        minWidth: 0,
      }}
    >
      {busy && !determinate ? (
        <Spinner label={`${name} ${status}`} />
      ) : (
        <Icon name={ICON[state]} tone={tone} />
      )}
      <div style={{ display: 'grid', gap: 2, minWidth: 0, flex: '1 1 auto' }}>
        <Text truncate={layout === 'row'}>{name}</Text>
        <Text
          variant="caption"
          muted={state !== 'error'}
          tone={state === 'error' ? 'danger' : undefined}
          data-testid={id('status')}
          style={{ overflowWrap: 'anywhere' }}
        >
          {status}
        </Text>
      </div>
      {buttons}
    </div>
  );

  const bar = determinate ? (
    <Progress
      value={pr.done / total}
      label={`${name} download`}
      data-testid={id('progress')}
    />
  ) : null;

  const details =
    layout === 'card' &&
    (purpose || note || (fallback && state !== 'ready')) ? (
      <div style={{ display: 'grid', gap: t.space[1] }}>
        {purpose ? (
          <Text variant="bodySm" muted>
            {purpose}
          </Text>
        ) : null}
        {fallback && state !== 'ready' ? (
          <Text variant="bodySm" muted data-testid={`${testId}-fallback`}>
            {`without it, ${fallback}`}
          </Text>
        ) : null}
        {note ? (
          <Text variant="bodySm" muted>
            {note}
          </Text>
        ) : null}
      </div>
    ) : null;

  return (
    <div
      data-testid={testId}
      data-state={state}
      data-layout={layout}
      role={state === 'error' ? 'alert' : 'status'}
      aria-live="polite"
      style={
        layout === 'card'
          ? {
              display: 'grid',
              gap: t.space[3],
              padding: t.space[3],
              borderRadius: t.radius.md,
              border: `1px solid ${state === 'error' ? `color-mix(in srgb, ${t.color.danger} 35%, transparent)` : t.color.border}`,
              minWidth: 0,
            }
          : { display: 'grid', gap: t.space[2], minHeight: 32, minWidth: 0 }
      }
    >
      {head}
      {bar}
      {details}
    </div>
  );
}
