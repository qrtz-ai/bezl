import { useEffect, useState } from 'react';
import {
  Button,
  Callout,
  Heading,
  Page,
  Progress,
  Select,
  SettingRow,
  SettingsSection,
  SpeechSetupPanel,
  Stack,
  Switch,
  Text,
} from '@qrtz/ui';
import type { IpcResponse } from '@qrtz/core';
import { requestNavigate } from '@qrtz/core';
import { useQrtz, useQrtzEvent, useSpeechSetup } from '@qrtz/core/react';

export default function SettingsView() {
  const qrtz = useQrtz();
  const [info, setInfo] = useState<IpcResponse<'app:get-info'> | null>(null);
  const [db, setDb] = useState<IpcResponse<'db:health'> | null>(null);
  const [helper, setHelper] = useState<IpcResponse<'helper:status'> | null>(
    null,
  );
  const [cfg, setCfg] = useState<Record<string, unknown> | null>(null);
  const live = useQrtzEvent('helper:status');
  const backupProgress = useQrtzEvent('data:backup-progress');
  const [error, setError] = useState<string | null>(null);
  const setup = useSpeechSetup();

  const refresh = () => {
    setError(null);
    // Load each section independently so one failing call (e.g. the DB worker restarting) doesn't blank the page.
    const fails: string[] = [];
    const load = <T,>(p: Promise<T>, set: (v: T) => void, what: string) =>
      p.then(set, (e: Error) => {
        fails.push(`${what}: ${e.message}`);
      });
    void Promise.all([
      load(qrtz.invoke('app:get-info'), setInfo, 'app info'),
      load(qrtz.invoke('db:health'), setDb, 'database'),
      load(qrtz.invoke('helper:status'), setHelper, 'helper'),
      load(qrtz.invoke('config:get'), setCfg, 'settings'),
    ]).then(() => {
      if (fails.length) setError(fails.join(' · '));
    });
  };
  useEffect(refresh, [qrtz]);
  const h = live ?? helper;
  const [backupMsg, setBackupMsg] = useState<string | null>(null);
  const [logsMsg, setLogsMsg] = useState<string | null>(null);
  // ADR 0083: the local logs (~/Library/Logs/qrtz), for sending to us when something breaks.
  const logs = (reveal: boolean) => {
    setLogsMsg(null);
    void qrtz.invoke('logs:export', { reveal }).then(
      (r) =>
        setLogsMsg(
          reveal ? null : r.savedTo ? `saved ${r.files} log files` : 'canceled',
        ),
      (e: Error) => setLogsMsg(e.message),
    );
  };
  const [backupBusy, setBackupBusy] = useState(false);
  const backup = (kind: 'export' | 'import') => {
    setBackupBusy(true);
    setBackupMsg(null);
    const run =
      kind === 'export'
        ? qrtz
            .invoke('data:export-backup', {})
            .then((r) =>
              r.savedTo
                ? `backed up ${r.meetings} meetings and ${r.audioFiles} recordings (${(r.bytes / 1e6).toFixed(1)} MB)`
                : 'canceled',
            )
        : qrtz
            .invoke('data:import-backup', {})
            .then((r) =>
              r.canceled
                ? 'canceled'
                : `verified and imported ${r.imported} meetings (${r.skipped} already here, ${r.audioFiles} recordings)`,
            );
    void run
      .then(setBackupMsg, (e: Error) => setBackupMsg(e.message))
      .finally(() => setBackupBusy(false));
  };
  const appCfg = (cfg?.app ?? {}) as {
    menuBarIcon?: boolean;
    launchAtLogin?: boolean;
  };
  const patchApp = (p: Record<string, unknown>) => {
    setCfg((prev) =>
      prev ? { ...prev, app: { ...(prev.app as object), ...p } } : prev,
    );
    return qrtz
      .invoke('config:set', { patch: { app: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));
  };
  const toggle = (
    label: string,
    testId: string,
    value: boolean | undefined,
    fallback: boolean,
    set: (v: boolean) => void,
  ) => (
    <SettingRow label={label}>
      <Switch
        label={label}
        checked={value ?? fallback}
        onChange={set}
        data-testid={testId}
      />
    </SettingRow>
  );
  return (
    <Page data-testid="settings">
      <Heading>settings</Heading>
      {error ? (
        <Callout
          tone="danger"
          title={error}
          data-testid="settings-error"
          action={
            <Button variant="secondary" size="sm" onClick={refresh}>
              retry
            </Button>
          }
        />
      ) : null}
      {/* ADR 0161: extensions are installed, switched on and off and removed in the marketplace; each one's own
          settings are in its view. Settings keeps only what spans all of qrtz. */}
      <SettingsSection title="extensions" data-testid="extensions-link">
        <Stack dir="row" gap={2} align="center" justify="between">
          <Text variant="caption" muted data-testid="modules-note">
            install, switch off and remove extensions in the marketplace. each
            one's own settings are in its view, behind the gear.
          </Text>
          <Button
            variant="secondary"
            size="sm"
            icon="package"
            onClick={() => requestNavigate({ app: 'marketplace' })}
            data-testid="open-marketplace"
          >
            marketplace
          </Button>
        </Stack>
      </SettingsSection>
      <SettingsSection title="speech" data-testid="speech-settings">
        {setup.error ? (
          <Callout
            tone="danger"
            title={setup.error}
            data-testid="speech-settings-error"
          />
        ) : null}
        <SpeechSetupPanel
          status={setup.speech}
          manage
          onDownload={setup.download}
          onPause={setup.pause}
          onResume={setup.resume}
          onDelete={setup.remove}
          onCheck={setup.check}
        />
        {toggle(
          'update speech automatically',
          'speech-auto-update',
          (cfg?.speech as { autoUpdate?: boolean } | undefined)?.autoUpdate,
          true,
          (v) => {
            setCfg((prev) =>
              prev
                ? {
                    ...prev,
                    speech: { ...(prev.speech as object), autoUpdate: v },
                  }
                : prev,
            );
            void qrtz
              .invoke('config:set', { patch: { speech: { autoUpdate: v } } })
              .then(setCfg)
              .catch((e: Error) => setError(e.message));
          },
        )}
        {toggle(
          'get speech ready right after download',
          'speech-warm-after-download',
          (cfg?.speech as { warmAfterDownload?: boolean } | undefined)
            ?.warmAfterDownload,
          true,
          (v) => {
            setCfg((prev) =>
              prev
                ? {
                    ...prev,
                    speech: {
                      ...(prev.speech as object),
                      warmAfterDownload: v,
                    },
                  }
                : prev,
            );
            void qrtz
              .invoke('config:set', {
                patch: { speech: { warmAfterDownload: v } },
              })
              .then(setCfg)
              .catch((e: Error) => setError(e.message));
          },
        )}
      </SettingsSection>
      <SettingsSection title="privacy" data-testid="privacy-card">
        <Text variant="caption" muted data-testid="offline-note">
          qrtz runs entirely on this computer: audio, transcripts and notes
          never leave it. it only goes online when you ask: to check for and
          download signed extensions, models and speech updates.
        </Text>
      </SettingsSection>
      <SettingsSection title="app">
        <SettingRow label="theme">
          <Select
            value={String(cfg?.theme ?? 'dark')}
            onChange={(e) =>
              void qrtz
                .invoke('config:set', { patch: { theme: e.target.value } })
                .then(setCfg)
                .catch((err: Error) => setError(err.message))
            }
            data-testid="theme"
          >
            <option value="dark">dark</option>
            <option value="light">light</option>
            <option value="system">system</option>
          </Select>
        </SettingRow>
        {toggle(
          'menu bar icon',
          'menu-bar-icon',
          appCfg.menuBarIcon,
          true,
          (v) => void patchApp({ menuBarIcon: v }),
        )}
        {toggle(
          'open at login',
          'launch-at-login',
          appCfg.launchAtLogin,
          false,
          (v) => void patchApp({ launchAtLogin: v }),
        )}
      </SettingsSection>
      <SettingsSection title="backup" data-testid="backup-card">
        <Stack dir="row" gap={2}>
          <Button
            variant="secondary"
            disabled={backupBusy}
            onClick={() => backup('export')}
            data-testid="backup-export"
          >
            export
          </Button>
          <Button
            variant="ghost"
            disabled={backupBusy}
            onClick={() => backup('import')}
            data-testid="backup-import"
          >
            import
          </Button>
        </Stack>
        {backupBusy && backupProgress && backupProgress.phase !== 'done' ? (
          <Stack gap={1} data-testid="backup-progress">
            <Progress
              value={backupProgress.done / Math.max(1, backupProgress.total)}
              label={`backup ${backupProgress.kind} progress`}
            />
            <Text variant="caption" muted>
              {
                {
                  checksum: 'computing checksums…',
                  write: 'writing zip…',
                  verify: 'verifying backup…',
                  import: 'importing meetings…',
                  done: '',
                }[backupProgress.phase]
              }{' '}
              {backupProgress.phase === 'import'
                ? ''
                : `${Math.round((backupProgress.done / Math.max(1, backupProgress.total)) * 100)}%`}
            </Text>
          </Stack>
        ) : null}
        {backupMsg ? (
          <Text
            variant="caption"
            muted
            data-testid="backup-result"
            role="status"
          >
            {backupMsg}
          </Text>
        ) : null}
      </SettingsSection>
      <SettingsSection title="system">
        <Text variant="caption" muted>
          {info
            ? `qrtz ${info.version} · ${info.platform}/${info.arch} · ${info.electron}`
            : '…'}
        </Text>
        <Text variant="caption" muted>
          {db
            ? `database: ${db.ok ? 'ok' : 'error'} · schema v${db.schemaVersion} · fts5 ${db.fts5 ? 'on' : 'off'}`
            : ''}
        </Text>
        <Text variant="caption" muted>
          {h ? `native helper: ${h.state} (restarts ${h.restarts})` : ''}
        </Text>
        <Stack dir="row" gap={2}>
          <Button
            variant="ghost"
            size="sm"
            onClick={refresh}
            data-testid="settings-refresh"
          >
            refresh
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => logs(false)}
            data-testid="logs-export"
          >
            export logs
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => logs(true)}
            data-testid="logs-show"
          >
            show logs
          </Button>
        </Stack>
        {logsMsg ? (
          <Text variant="caption" muted data-testid="logs-result" role="status">
            {logsMsg}
          </Text>
        ) : null}
      </SettingsSection>
    </Page>
  );
}
