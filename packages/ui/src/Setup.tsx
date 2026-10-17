import type { ReactNode } from 'react';
import { Button } from './Button';
import { Callout } from './Callout';
import { Heading } from './Heading';
import { Page, Stack } from './Layout';
import { Progress } from './Progress';
import { Spinner } from './Spinner';
import { Text } from './Text';

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

export interface SearchModelView {
  state: 'missing' | 'downloading' | 'loading' | 'ready' | 'error';
  error?: string;
}

/** 1.2 gb · 340 mb · 12 kb (lowercase, brand rule). */
export function formatBytes(n: number): string {
  if (n <= 0) return '0 mb';
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} gb`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} mb`;
  return `${Math.max(1, Math.round(n / 1e3))} kb`;
}

const BUSY = new Set(['checking', 'downloading', 'paused', 'installing']);

/**
 * The speech module: size, download progress with pause / resume, retry on error, and (in settings) update,
 * re-download and delete. Onboarding and settings both render this.
 */
export function SpeechSetupPanel({
  status,
  manage = false,
  onDownload,
  onPause,
  onResume,
  onDelete,
  'data-testid': testId = 'speech-setup',
}: {
  status: SpeechSetupView | null;
  /** Settings: show update / re-download / delete for an installed module. */
  manage?: boolean;
  onDownload: (force?: boolean) => void;
  onPause: () => void;
  onResume: () => void;
  onDelete?: () => void;
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

/** The on-device search model: download when missing, retry on error. */
export function SearchModelRow({
  status,
  onDownload,
}: {
  status: SearchModelView | null;
  onDownload: () => void;
}) {
  const s = status?.state;
  return (
    <Stack
      dir="row"
      gap={3}
      align="center"
      justify="between"
      data-testid="search-model"
    >
      <Text>search model</Text>
      {s === 'downloading' || s === 'loading' ? (
        <Stack dir="row" gap={2} align="center">
          <Spinner label={s === 'loading' ? 'loading' : 'downloading'} />
          <Text variant="caption" muted>
            {s === 'loading' ? 'loading…' : 'downloading…'}
          </Text>
        </Stack>
      ) : s === 'ready' ? (
        <Text variant="caption" muted data-testid="search-model-ready">
          ready
        </Text>
      ) : s ? (
        <Stack dir="row" gap={2} align="center">
          {s === 'error' ? (
            <Text
              variant="caption"
              tone="danger"
              data-testid="search-model-error"
            >
              {status?.error ?? "couldn't download the search model"}
            </Text>
          ) : null}
          <Button
            size="sm"
            variant="secondary"
            icon={s === 'error' ? 'refresh' : 'download'}
            onClick={onDownload}
            data-testid="search-model-download"
          >
            {s === 'error' ? 'retry' : 'download'}
          </Button>
        </Stack>
      ) : null}
    </Stack>
  );
}

/** One first-run step: a step count, a title, its content and the actions. */
export function OnboardingStep({
  step,
  steps,
  title,
  children,
  actions,
  'data-testid': testId,
}: {
  step: number;
  steps: number;
  title: string;
  children?: ReactNode;
  actions: ReactNode;
  'data-testid'?: string;
}) {
  return (
    <Page width={480} data-testid={testId}>
      <Stack gap={2}>
        <Text variant="caption" muted>{`${step} of ${steps}`}</Text>
        <Heading cursor>{title}</Heading>
      </Stack>
      {children}
      <Stack dir="row" gap={2} justify="end">
        {actions}
      </Stack>
    </Page>
  );
}

/**
 * Note and talk: speech isn't ready yet (downloading, paused, failed or not downloaded). Nothing once it's
 * installed, or in a build without a speech module.
 */
export function SpeechNotice({
  status,
  onOpenSettings,
}: {
  status: SpeechSetupView | null;
  onOpenSettings: () => void;
}) {
  if (!status?.configured) return null;
  const d = status.download;
  const open = (
    <Button
      size="sm"
      variant="secondary"
      icon="settings"
      onClick={onOpenSettings}
      data-testid="speech-notice-settings"
    >
      settings
    </Button>
  );
  if (BUSY.has(d.state))
    return (
      <Stack gap={1} data-testid="speech-notice" aria-live="polite">
        <Text variant="caption" muted>
          {d.state === 'paused'
            ? 'speech download paused'
            : d.state === 'installing'
              ? 'installing speech…'
              : `downloading speech · ${d.total ? Math.round((100 * d.done) / d.total) : 0}%`}
        </Text>
        <Progress
          value={d.total ? d.done / d.total : 0}
          label="speech download"
        />
      </Stack>
    );
  if (d.state === 'error')
    return (
      <Callout
        tone="warning"
        title={d.error ?? "speech couldn't be installed"}
        action={open}
        data-testid="speech-notice"
      />
    );
  if (!status.installed)
    return (
      <Callout
        title="speech isn't downloaded"
        action={open}
        data-testid="speech-notice"
      />
    );
  return null;
}
