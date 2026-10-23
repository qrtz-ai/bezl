import { tokens } from './tokens';
import { useTheme } from './theme';
import { Icon } from './Icon';
import { Button } from './Button';
import { ErrorCallout, type UserError } from './UserError';
import { Stack } from './Layout';
import { Spinner } from './Spinner';
import { Text } from './Text';
import type { ReactNode } from 'react';
import {
  ModelRequired,
  ModelRequirement,
  type ModelRequirementProps,
  type ModelRequirementState,
} from './ModelRequirement';

/** Structural copy of `SpeechSetupStatus` (@qrtz/core): a version, a size and a download state, never an engine. */
export interface SpeechSetupView {
  configured: boolean;
  installed: string | null;
  bytes: number;
  latest: string | null;
  latestBytes: number | null;
  waiting: string | null;
  listExpired: boolean;
  download: {
    state:
      | 'idle'
      | 'checking'
      | 'downloading'
      | 'paused'
      | 'installing'
      | 'error';
    done: number;
    total: number;
    error?: string;
  };
}

const BUSY = new Set(['checking', 'downloading', 'paused', 'installing']);

/**
 * Echo and nano can't work without their speech model: until it's installed, the app's page body shows the
 * ModelRequired banner and the rest of the page blurred and inert. `status` null (still reading): the page shows
 * as is, so an installed model never flashes the gate.
 */
export function SpeechRequired({
  app,
  status,
  note,
  onDownload,
  onCancel,
  onOpen,
  children,
  'data-testid': testId,
  testIds,
}: {
  app: string;
  status: SpeechSetupView | null;
  note?: ReactNode;
  /** Only an app that may download (echo holds speech.manage); others open their models on the qrtz page. */
  onDownload?: () => void;
  onCancel?: () => void;
  onOpen?: () => void;
  children?: ReactNode;
  'data-testid'?: string;
  testIds?: ModelRequirementProps['testIds'];
}) {
  if (!status) return <>{children}</>;
  const d = status.download;
  const busy = BUSY.has(d.state);
  const state: ModelRequirementState = !status.configured
    ? 'unavailable'
    : busy
      ? 'downloading'
      : d.state === 'error'
        ? 'error'
        : status.installed
          ? 'ready'
          : 'missing';
  return (
    <ModelRequired
      app={app}
      name="speech model"
      purpose="turns what you say into text, on this mac"
      sizeBytes={status.latestBytes ?? (d.total || null)}
      state={state}
      {...(busy ? { progress: speechProgress(d) } : {})}
      error={d.error ?? null}
      {...(note ? { note } : {})}
      {...(onDownload ? { onDownload, onRetry: onDownload } : {})}
      {...(busy && onCancel ? { onCancel } : {})}
      {...(!busy && onOpen && !onDownload ? { onOpen } : {})}
      {...(testId ? { 'data-testid': testId } : {})}
      {...(testIds ? { testIds } : {})}
    >
      {children}
    </ModelRequired>
  );
}

/** A speech download's bytes and phase, for ModelRequirement. */
function speechProgress(d: SpeechSetupView['download']) {
  return {
    done: d.done,
    total: d.total,
    phase:
      d.state === 'idle' || d.state === 'error'
        ? ('downloading' as const)
        : d.state,
  };
}

/** One of an app's model sets, and whether deleting this app's models would keep it (another app uses it). */
export interface ModelSetView {
  name: string;
  /** "kept, used by nano": a switched-on app still needs it, so delete leaves it. */
  keptFor?: readonly string[];
}

/**
 * An app's models, in its detail on the qrtz page: each model set (and which ones another app keeps), their size
 * on disk and state, download / pause / resume / retry while they're missing, and "delete models" (danger; the
 * feature asks to confirm). `deleteBlocked`: nothing this app alone uses is on disk, so there is nothing to delete.
 */
export function ModelsPanel({
  status,
  sets,
  error,
  onDownload,
  onPause,
  onResume,
  onDelete,
  deleteBlocked,
  'data-testid': testId = 'models',
}: {
  status: SpeechSetupView | null;
  sets: readonly ModelSetView[];
  /** QRT-311: what couldn't happen and the next step; the raw reason sits under details. */
  error?: UserError | null;
  onDownload: () => void;
  onPause: () => void;
  onResume: () => void;
  onDelete: () => void;
  deleteBlocked?: boolean;
  'data-testid'?: string;
}) {
  const t = useTheme();
  const lines = (
    <ul
      aria-label="model sets"
      style={{ margin: 0, padding: 0, display: 'grid', gap: tokens.space[0.5] }}
    >
      {sets.map((m) => (
        <li
          key={m.name}
          data-testid={`${testId}-set-${m.name}`}
          style={{
            listStyle: 'none',
            display: 'flex',
            alignItems: 'center',
            gap: t.space[3],
            minHeight: tokens.size.row.sm,
            minWidth: 0,
          }}
        >
          <Icon name="model" size="sm" tone="muted" />
          <Text truncate>{m.name.toLowerCase()}</Text>
          {m.keptFor?.length ? (
            <span style={{ marginLeft: 'auto', flexShrink: 0 }}>
              <Text
                variant="caption"
                muted
                data-testid={`${testId}-kept-${m.name}`}
              >
                {`kept, used by ${m.keptFor.join(', ')}`}
              </Text>
            </span>
          ) : null}
        </li>
      ))}
    </ul>
  );
  if (!status)
    return (
      <Stack gap={2} data-testid={testId}>
        {lines}
        <Spinner label="loading models" />
      </Stack>
    );
  if (!status.configured)
    return (
      <Stack gap={2} data-testid={testId}>
        {lines}
        <Text variant="caption" muted>
          models aren&apos;t available in this build.
        </Text>
      </Stack>
    );
  const d = status.download;
  const size = status.latestBytes ?? d.total;
  return (
    <Stack gap={3} data-testid={testId} aria-live="polite">
      {lines}
      {error ? (
        <ErrorCallout error={error} data-testid={`${testId}-error`} />
      ) : null}
      <ModelRequirement
        layout="row"
        name="models"
        sizeBytes={status.installed ? status.bytes : size || null}
        state={
          BUSY.has(d.state)
            ? 'downloading'
            : d.state === 'error'
              ? 'error'
              : status.installed
                ? 'ready'
                : 'missing'
        }
        {...(BUSY.has(d.state) ? { progress: speechProgress(d) } : {})}
        error={d.error ?? null}
        onDownload={onDownload}
        onRetry={onDownload}
        actions={
          d.state === 'paused' ? (
            <Button
              size="sm"
              variant="secondary"
              icon="play"
              onClick={onResume}
              data-testid={`${testId}-resume`}
            >
              resume
            </Button>
          ) : d.state === 'downloading' ? (
            <Button
              size="sm"
              variant="secondary"
              icon="pause"
              onClick={onPause}
              data-testid={`${testId}-pause`}
            >
              pause
            </Button>
          ) : status.installed && !BUSY.has(d.state) && d.state !== 'error' ? (
            <Button
              size="sm"
              variant="danger"
              icon="trash"
              disabled={deleteBlocked}
              onClick={onDelete}
              data-testid={`${testId}-delete`}
            >
              delete models
            </Button>
          ) : null
        }
        data-testid={
          d.state === 'error' ? `${testId}-download-error` : `${testId}-status`
        }
        testIds={{
          download: `${testId}-download`,
          retry: `${testId}-retry`,
          status: `${testId}-state`,
          progress: `${testId}-progress`,
        }}
      />
    </Stack>
  );
}
