import { useEffect, useState } from 'react';
import {
  Button,
  ErrorCallout,
  Progress,
  SettingRow,
  Stack,
  Text,
  userError,
  type UserError,
} from '@qrtz/ui';
import type { AppUpdateStatus } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

const mb = (b: number | null | undefined) =>
  b ? `${(b / 1e6).toFixed(1)} MB` : '';

/** What the row says under the version, one line per state. */
export function appUpdateLine(s: AppUpdateStatus): string {
  switch (s.state) {
    case 'not-set-up':
      return "updates aren't set up in this build yet";
    case 'idle':
      return '';
    case 'checking':
      return 'checking for updates…';
    case 'up-to-date':
      return `qrtz ${s.current} is up to date`;
    case 'available':
      return `qrtz ${s.latest} is available${s.sizeBytes ? ` (${mb(s.sizeBytes)})` : ''}`;
    case 'downloading':
      return `downloading qrtz ${s.latest}… ${mb(s.doneBytes)} of ${mb(s.sizeBytes)}`;
    case 'installing':
      return `checking and installing qrtz ${s.latest}…`;
    case 'ready':
      return `qrtz ${s.latest} is ready: restart to update`;
    case 'failed':
      return `${failedUpdate(s).title}: ${failedUpdate(s).next}`;
  }
}

/** QRT-311: a failed update says so plainly; a technical reason (codes, statuses) goes under details. */
function failedUpdate(s: AppUpdateStatus): UserError {
  return userError(s.error ?? '', 'update qrtz');
}

/**
 * ADR 0181: the "qrtz" row in settings: this build's version, "check for updates", "update" (with its progress)
 * and, once the new qrtz.app is in place, "restart to update" (ADR 0172's restart). The host checks quietly at
 * launch and every 6 hours; this row follows `app-update:state`.
 */
export default function AppUpdateRow() {
  const qrtz = useQrtz();
  const [status, setStatus] = useState<AppUpdateStatus | null>(null);
  const live = useQrtzEvent('app-update:state');
  const [error, setError] = useState<UserError | null>(null);
  useEffect(() => {
    void qrtz
      .invoke('app-update:status')
      .then(setStatus, (e: unknown) =>
        setError(userError(e, 'check the update status')),
      );
  }, [qrtz]);
  useEffect(() => {
    if (live) setStatus(live);
  }, [live]);
  const run = (p: Promise<AppUpdateStatus>) => {
    setError(null);
    void p.then(setStatus, (e: unknown) =>
      setError(userError(e, 'reach the update server')),
    );
  };
  const restart = () => {
    setError(null);
    void qrtz.invoke('app-update:restart').then(
      (r) => {
        if (!r.restarting)
          setError({
            title: "couldn't restart qrtz",
            next: 'quit and open it again.',
            detail: null,
          });
      },
      (e: unknown) =>
        setError(userError(e, 'restart qrtz', 'quit and open it again.')),
    );
  };
  const s = status;
  const line = s ? appUpdateLine(s) : '';
  const busy =
    s?.state === 'checking' ||
    s?.state === 'downloading' ||
    s?.state === 'installing';
  return (
    <Stack gap={2} data-testid="app-update">
      <SettingRow label={`qrtz ${s?.current ?? ''}`.trim()}>
        <Stack dir="row" gap={2}>
          {s?.state === 'ready' ? (
            <Button
              size="sm"
              icon="restart"
              onClick={restart}
              data-testid="app-update-restart"
            >
              restart to update
            </Button>
          ) : s?.state === 'available' ||
            (s?.state === 'failed' && s.latest) ? (
            <Button
              size="sm"
              icon="download"
              onClick={() => run(qrtz.invoke('app-update:download'))}
              data-testid="app-update-download"
            >
              update
            </Button>
          ) : null}
          <Button
            variant="ghost"
            size="sm"
            icon="refresh"
            disabled={
              !s || s.state === 'not-set-up' || busy || s.state === 'ready'
            }
            onClick={() => run(qrtz.invoke('app-update:check'))}
            data-testid="update-check"
          >
            check for updates
          </Button>
        </Stack>
      </SettingRow>
      {s?.state === 'downloading' ? (
        <Progress
          value={(s.doneBytes ?? 0) / Math.max(1, s.sizeBytes ?? 0)}
          label="qrtz update progress"
          data-testid="app-update-progress"
        />
      ) : null}
      {s?.state === 'failed' ? (
        <ErrorCallout error={failedUpdate(s)} data-testid="update-result" />
      ) : line ? (
        <Text variant="caption" muted role="status" data-testid="update-result">
          {line}
        </Text>
      ) : null}
      {s?.notes && (s.state === 'available' || s.state === 'ready') ? (
        <Text variant="caption" muted data-testid="app-update-notes">
          {s.notes.toLowerCase()}
        </Text>
      ) : null}
      <ErrorCallout
        error={error}
        onDismiss={() => setError(null)}
        data-testid="app-update-error"
      />
    </Stack>
  );
}
