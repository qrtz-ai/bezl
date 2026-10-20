import { tokens } from './tokens';
import { useTheme } from './theme';
import { Icon } from './Icon';
import { Button } from './Button';
import { Callout } from './Callout';
import { Stack } from './Layout';
import { Progress } from './Progress';
import { Spinner } from './Spinner';
import { Text } from './Text';
import type { ReactNode } from 'react';
import {
  ModelRequired,
  ModelRequirement,
  formatBytes,
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
 * The speech module: size, download progress with pause / resume, retry on error, and (with `manage`) update,
 * re-download and delete. Each app's own models are managed in its detail on the qrtz page (ModelsPanel).
 */
export function SpeechSetupPanel({
  status,
  manage = false,
  onDownload,
  onPause,
  onResume,
  onDelete,
  onCheck,
  'data-testid': testId = 'speech-setup',
}: {
  status: SpeechSetupView | null;
  /** Settings: show update / re-download / delete for an installed module. */
  manage?: boolean;
  onDownload: (force?: boolean) => void;
  onPause: () => void;
  onResume: () => void;
  onDelete?: () => void;
  /** Settings: "check for updates", started by the user (ADR 0078: nothing checks on its own). */
  onCheck?: () => void;
  'data-testid'?: string;
}) {
  if (!status)
    return (
      <Stack dir="row" gap={2} align="center" data-testid={testId}>
        <Spinner label="loading speech" />
      </Stack>
    );
  if (!status.configured)
    return (
      <Text muted data-testid={testId}>
        speech isn&apos;t available in this build
      </Text>
    );
  const d = status.download;
  const size = status.latestBytes ?? d.total;
  const update =
    status.installed && status.latest && status.latest !== status.installed
      ? status.latest
      : null;
  return (
    <Stack gap={3} data-testid={testId} aria-live="polite">
      {status.listExpired ? (
        <Callout
          tone="warning"
          title="connect to the internet so speech can check for revocations"
          data-testid="speech-list-expired"
        />
      ) : null}
      {BUSY.has(d.state) ? (
        <Stack gap={2}>
          <Progress
            value={d.total ? d.done / d.total : 0}
            label="speech download"
            data-testid="speech-progress"
          />
          <Stack dir="row" gap={3} align="center" justify="between">
            <Text variant="caption" muted data-testid="speech-progress-text">
              {d.state === 'checking'
                ? 'checking…'
                : d.state === 'installing'
                  ? 'installing…'
                  : `${d.state === 'paused' ? 'paused · ' : ''}${formatBytes(d.done)} of ${formatBytes(d.total || size || 0)}`}
            </Text>
            {d.state === 'paused' ? (
              <Button
                size="sm"
                variant="secondary"
                icon="play"
                onClick={onResume}
                data-testid="speech-resume"
              >
                resume
              </Button>
            ) : d.state === 'downloading' ? (
              <Button
                size="sm"
                variant="secondary"
                icon="pause"
                onClick={onPause}
                data-testid="speech-pause"
              >
                pause
              </Button>
            ) : null}
          </Stack>
        </Stack>
      ) : null}
      {d.state === 'error' ? (
        <Callout
          tone="danger"
          title={d.error ?? "speech couldn't be installed"}
          data-testid="speech-error"
          action={
            <Button
              size="sm"
              variant="secondary"
              icon="refresh"
              onClick={() => onDownload(false)}
              data-testid="speech-retry"
            >
              retry
            </Button>
          }
        />
      ) : null}
      {d.state === 'idle' && !status.installed ? (
        <Stack dir="row" gap={3} align="center" justify="between">
          <Text data-testid="speech-size">
            {size ? `speech · ${formatBytes(size)}` : 'speech'}
          </Text>
          <Button
            variant="primary"
            icon="download"
            onClick={() => onDownload(false)}
            data-testid="speech-download"
          >
            download
          </Button>
        </Stack>
      ) : null}
      {status.installed && !BUSY.has(d.state) ? (
        <Stack gap={2}>
          <Stack dir="row" gap={3} align="center" justify="between">
            <Text data-testid="speech-installed">
              {`speech ${status.installed} · ${formatBytes(status.bytes)}`}
            </Text>
            {update ? (
              <Button
                variant="primary"
                size="sm"
                icon="download"
                onClick={() => onDownload(false)}
                data-testid="speech-update"
              >
                {`update to ${update}`}
              </Button>
            ) : null}
          </Stack>
          {status.waiting ? (
            <Text variant="caption" muted data-testid="speech-waiting">
              {`${status.waiting} installs when this session ends`}
            </Text>
          ) : null}
          {manage ? (
            <Stack dir="row" gap={2}>
              <Button
                size="sm"
                variant="secondary"
                icon="refresh"
                onClick={() => onDownload(true)}
                data-testid="speech-redownload"
              >
                re-download
              </Button>
              {onCheck ? (
                <Button
                  size="sm"
                  variant="secondary"
                  icon="refresh"
                  onClick={onCheck}
                  data-testid="speech-check"
                >
                  check for updates
                </Button>
              ) : null}
              {onDelete ? (
                <Button
                  size="sm"
                  variant="danger"
                  icon="trash"
                  onClick={onDelete}
                  data-testid="speech-delete"
                >
                  delete
                </Button>
              ) : null}
            </Stack>
          ) : null}
        </Stack>
      ) : null}
    </Stack>
  );
}

/**
 * Note and talk: speech isn't ready yet (downloading, paused, failed or not downloaded). Nothing once it's
 * installed, or in a build without a speech module.
 */
export function SpeechNotice({
  status,
  onOpen,
}: {
  status: SpeechSetupView | null;
  /** Opens this app's detail on the qrtz page, where its models are managed. */
  onOpen: () => void;
}) {
  if (!status?.configured) return null;
  const d = status.download;
  const busy = BUSY.has(d.state);
  if (!busy && d.state !== 'error' && status.installed) return null;
  return (
    <ModelRequirement
      name="speech model"
      purpose="turns what you say into text, on this mac"
      sizeBytes={status.latestBytes ?? (d.total || null)}
      state={busy ? 'downloading' : d.state === 'error' ? 'error' : 'missing'}
      {...(busy ? { progress: speechProgress(d) } : {})}
      error={d.error ?? null}
      {...(busy ? {} : { onOpen })}
      data-testid="speech-notice"
      testIds={{ open: 'speech-notice-open' }}
    />
  );
}

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
  error?: string | null;
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
            minHeight: 28,
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
        <Callout tone="danger" title={error} data-testid={`${testId}-error`} />
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
