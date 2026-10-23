import { useEffect, useState } from 'react';
import {
  Button,
  Dialog,
  ErrorCallout,
  LongText,
  Page,
  Progress,
  Select,
  SettingRow,
  SettingsSection,
  Stack,
  Switch,
  Text,
  userError,
  type UserError,
} from '@qrtz/ui';
import type { IpcResponse } from '@qrtz/core';
import { requestNavigate } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';
import AppUpdateRow from './AppUpdateRow';

export default function SettingsView() {
  const qrtz = useQrtz();
  const [info, setInfo] = useState<IpcResponse<'app:get-info'> | null>(null);
  const [db, setDb] = useState<IpcResponse<'db:health'> | null>(null);
  const [helper, setHelper] = useState<IpcResponse<'helper:status'> | null>(
    null,
  );
  const [cfg, setCfg] = useState<Record<string, unknown> | null>(null);
  const live = useQrtzEvent('helper:status');
  // The host's config pushes (a module installed or removed elsewhere changes the permissions note, ADR 0184).
  const liveCfg = useQrtzEvent('config:changed');
  useEffect(() => {
    if (liveCfg) setCfg(liveCfg);
  }, [liveCfg]);
  const backupProgress = useQrtzEvent('data:backup-progress');
  const [error, setError] = useState<UserError | null>(null);
  /** QRT-311: a failure says what couldn't happen; the raw reason stays under details. */
  const fail = (what: string) => (e: unknown) => setError(userError(e, what));

  const refresh = () => {
    setError(null);
    // Load each section independently so one failing call (e.g. the DB worker restarting) doesn't blank the page.
    const fails: { what: string; raw: string }[] = [];
    const load = <T,>(p: Promise<T>, set: (v: T) => void, what: string) =>
      p.then(set, (e: unknown) => {
        fails.push({ what, raw: userError(e, what).detail ?? String(e) });
      });
    void Promise.all([
      load(qrtz.invoke('app:get-info'), setInfo, 'app info'),
      load(qrtz.invoke('db:health'), setDb, 'database'),
      load(qrtz.invoke('helper:status'), setHelper, 'helper'),
      load(qrtz.invoke('config:get'), setCfg, 'settings'),
    ]).then(() => {
      if (fails.length)
        setError({
          title: `couldn't load ${fails.map((f) => f.what).join(', ')}`,
          next: 'the rest of opts still works. retry in a moment.',
          detail: fails.map((f) => `${f.what}: ${f.raw}`).join('\n'),
        });
    });
  };
  useEffect(refresh, [qrtz]);
  const h = live ?? helper;
  const [backupMsg, setBackupMsg] = useState<string | null>(null);
  const [logsMsg, setLogsMsg] = useState<string | null>(null);
  const [backupErr, setBackupErr] = useState<UserError | null>(null);
  const [logsErr, setLogsErr] = useState<UserError | null>(null);
  // Audit P0-4: the third-party notices built into this qrtz (THIRD_PARTY_NOTICES.md), loaded only when opened.
  const [licenses, setLicenses] = useState<string | null>(null);
  const openLicenses = () =>
    void qrtz
      .invoke('app:licenses')
      .then((r) => setLicenses(r.text), fail('open the licenses'));
  // ADR 0083: the local logs (~/Library/Logs/qrtz), for sending to us when something breaks.
  const logs = (reveal: boolean) => {
    setLogsMsg(null);
    setLogsErr(null);
    void qrtz.invoke('logs:export', { reveal }).then(
      (r) =>
        setLogsMsg(
          reveal ? null : r.savedTo ? `saved ${r.files} log files` : 'canceled',
        ),
      (e: unknown) => setLogsErr(userError(e, 'export the logs')),
    );
  };
  // Audit P1-3: the logs plus macOS's crash reports for qrtz and its helpers, in one file the user sends.
  const diagnostics = () => {
    setLogsMsg(null);
    setLogsErr(null);
    void qrtz.invoke('logs:export', { diagnostics: true }).then(
      (r) =>
        setLogsMsg(
          r.savedTo
            ? `saved ${r.files} log files and ${r.crashReports ?? 0} crash reports`
            : 'canceled',
        ),
      (e: unknown) => setLogsErr(userError(e, 'save the diagnostics')),
    );
  };
  const [backupBusy, setBackupBusy] = useState(false);
  const backup = (kind: 'export' | 'import') => {
    setBackupBusy(true);
    setBackupMsg(null);
    setBackupErr(null);
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
      .then(setBackupMsg, (e: unknown) =>
        setBackupErr(
          userError(
            e,
            kind === 'export' ? 'back up your data' : 'import the backup',
          ),
        ),
      )
      .finally(() => setBackupBusy(false));
  };
  // ADR 0184 amendment: the host's `modules.nano.installed` (ADR 0169); unknown until config loads.
  const nanoInstalled =
    (cfg?.modules as { nano?: { installed?: boolean } } | undefined)?.nano
      ?.installed === true;
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
      .catch(fail('save that setting'));
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
    // The opts page: qrtz's own opts, pinned to the bottom of the sidebar under apps.
    <Page data-testid="settings">
      {error ? (
        <ErrorCallout
          error={error}
          data-testid="settings-error"
          action={
            <Button variant="secondary" size="sm" onClick={refresh}>
              retry
            </Button>
          }
        />
      ) : null}
      {/* ADR 0181: qrtz itself: its version and its own update. */}
      <SettingsSection title="qrtz" data-testid="about-qrtz">
        <AppUpdateRow />
      </SettingsSection>
      <SettingsSection title="app">
        <SettingRow label="theme">
          <Select
            value={String(cfg?.theme ?? 'dark')}
            onChange={(e) =>
              void qrtz
                .invoke('config:set', { patch: { theme: e.target.value } })
                .then(setCfg)
                .catch(fail('change the theme'))
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
              .catch(fail('save that setting'));
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
              .catch(fail('save that setting'));
          },
        )}
      </SettingsSection>
      <SettingsSection title="permissions" data-testid="permissions-card">
        <SettingRow
          as="div"
          descriptionTestId="permissions-note"
          description={
            <>
              {/* ADR 0184 amendment: system audio is listed only with nano installed. */}
              {nanoInstalled
                ? 'microphone, system audio and accessibility.'
                : 'microphone and accessibility.'}{' '}
              each app also asks the first time it needs one.
            </>
          }
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={() => requestNavigate({ app: 'qrtz', onboarding: true })}
            data-testid="onboarding-reopen"
          >
            set up
          </Button>
        </SettingRow>
      </SettingsSection>
      <SettingsSection title="backup" data-testid="backup-card">
        <>
          <SettingRow
            as="div"
            description="export your meetings and recordings to one file, or import one."
          >
            <Button
              variant="secondary"
              size="sm"
              disabled={backupBusy}
              onClick={() => backup('export')}
              data-testid="backup-export"
            >
              export
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={backupBusy}
              onClick={() => backup('import')}
              data-testid="backup-import"
            >
              import
            </Button>
          </SettingRow>
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
          <ErrorCallout
            error={backupErr}
            onDismiss={() => setBackupErr(null)}
            data-testid="backup-error"
          />
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
        </>
      </SettingsSection>
      <SettingsSection title="system" data-testid="system-card">
        <SettingRow
          as="div"
          label="this computer"
          descriptionTestId="system-info"
          description={
            <>
              {info
                ? `qrtz ${info.version} · ${info.platform}/${info.arch} · ${info.electron}`
                : '…'}
              {db
                ? ` · database ${db.ok ? 'ok' : 'error'}, schema v${db.schemaVersion}, fts5 ${db.fts5 ? 'on' : 'off'}`
                : ''}
              {h ? ` · helper ${h.state} (restarts ${h.restarts})` : ''}
            </>
          }
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={refresh}
            data-testid="settings-refresh"
          >
            refresh
          </Button>
        </SettingRow>
        <SettingRow
          as="div"
          label="logs"
          description="for sending to us when something breaks."
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={() => logs(true)}
            data-testid="logs-show"
          >
            show
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => logs(false)}
            data-testid="logs-export"
          >
            export
          </Button>
        </SettingRow>
        <SettingRow
          as="div"
          label="diagnostics"
          description="logs plus crash reports, in one file."
        >
          <Button
            variant="secondary"
            size="sm"
            onClick={diagnostics}
            data-testid="diagnostics-export"
          >
            export
          </Button>
        </SettingRow>
        <SettingRow as="div" label="open-source licenses">
          <Button
            variant="secondary"
            size="sm"
            onClick={openLicenses}
            data-testid="licenses-open"
          >
            view
          </Button>
        </SettingRow>
        {logsErr || logsMsg ? (
          <>
            <ErrorCallout
              error={logsErr}
              onDismiss={() => setLogsErr(null)}
              data-testid="logs-error"
            />
            {logsMsg ? (
              <Text
                variant="caption"
                muted
                data-testid="logs-result"
                role="status"
              >
                {logsMsg}
              </Text>
            ) : null}
          </>
        ) : null}
      </SettingsSection>
      <SettingsSection title="privacy" data-testid="privacy-card">
        <Text variant="caption" muted data-testid="offline-note">
          qrtz runs entirely on this computer: audio, transcripts and notes
          never leave it. it goes online to check for a new qrtz (at launch and
          every 6 hours; nothing about you is sent) and when you ask: to
          download signed apps, models and speech updates.
        </Text>
      </SettingsSection>
      <Dialog
        open={licenses !== null}
        onClose={() => setLicenses(null)}
        title="open-source licenses"
        width={760}
      >
        <LongText data-testid="licenses-text">{licenses}</LongText>
      </Dialog>
    </Page>
  );
}
