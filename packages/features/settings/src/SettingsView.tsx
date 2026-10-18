import { useEffect, useState } from 'react';
import {
  Button,
  Callout,
  Field,
  Heading,
  Input,
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
import { SUMMARY_TEMPLATES, type IpcResponse } from '@qrtz/core';
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
  const noteCfg = (cfg?.note ?? {}) as {
    retainAudioDays?: number;
    autoRecord?: boolean;
    autoDetectMeetings?: boolean;
    detectPrompt?: boolean;
    notifyNoAudio?: boolean;
    floatingHud?: boolean;
    autoSummarize?: boolean;
    defaultTemplate?: string;
  };
  const privacy = (cfg?.privacy ?? {}) as {
    excludedApps?: string[];
    pauseHotkey?: string;
  };
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
  const patchPrivacy = (p: Record<string, unknown>) => {
    // Optimistic so the checkbox reflects the click immediately.
    setCfg((prev) =>
      prev ? { ...prev, privacy: { ...(prev.privacy as object), ...p } } : prev,
    );
    return qrtz
      .invoke('config:set', { patch: { privacy: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));
  };
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
  // ADR 0146: the global switches that control every module; each module's own settings stay in its section.
  const modulesCfg = (cfg?.modules ?? {}) as {
    enabled?: boolean;
    echo?: { enabled?: boolean };
    nano?: { enabled?: boolean };
    ctrl?: { enabled?: boolean };
  };
  const patchModules = (p: Record<string, unknown>) => {
    setCfg((prev) =>
      prev ? { ...prev, modules: { ...(prev.modules as object), ...p } } : prev,
    );
    return qrtz
      .invoke('config:set', { patch: { modules: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));
  };
  const patchNote = (p: Record<string, unknown>) => {
    setCfg((prev) =>
      prev ? { ...prev, note: { ...(prev.note as object), ...p } } : prev,
    );
    return qrtz
      .invoke('config:set', { patch: { note: p } })
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
  const days = noteCfg.retainAudioDays ?? 0;
  const talkCfg = (cfg?.talk ?? {}) as {
    enabled?: boolean;
    pushKey?: string;
    blockGlobeKey?: boolean;
    toggleHotkey?: string;
    warmMic?: boolean;
    autoCleanup?: boolean;
    autoInsert?: boolean;
    restoreClipboard?: boolean;
    showPill?: boolean;
    echoGate?: boolean;
    saveHistory?: boolean;
  };
  const patchTalk = (p: Record<string, unknown>) => {
    setCfg((prev) =>
      prev ? { ...prev, talk: { ...(prev.talk as object), ...p } } : prev,
    );
    return qrtz
      .invoke('config:set', { patch: { talk: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));
  };
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
      <SettingsSection title="modules" data-testid="modules-settings">
        {toggle(
          'all modules',
          'modules-all',
          modulesCfg.enabled,
          true,
          (v) => void patchModules({ enabled: v }),
        )}
        {toggle(
          'echo (dictation)',
          'module-echo',
          modulesCfg.echo?.enabled,
          true,
          (v) => void patchModules({ echo: { enabled: v } }),
        )}
        {toggle(
          'nano (meetings)',
          'module-nano',
          modulesCfg.nano?.enabled,
          true,
          (v) => void patchModules({ nano: { enabled: v } }),
        )}
        {toggle(
          'ctrl (tasks)',
          'module-ctrl',
          modulesCfg.ctrl?.enabled,
          true,
          (v) => void patchModules({ ctrl: { enabled: v } }),
        )}
        <Text variant="caption" muted data-testid="modules-note">
          a module that is off has no tab, no hotkeys and runs nothing. its own
          settings are kept.
        </Text>
      </SettingsSection>
      <SettingsSection title="recording">
        {toggle(
          'detect meetings',
          'auto-detect',
          noteCfg.autoDetectMeetings,
          true,
          (v) => void patchNote({ autoDetectMeetings: v }),
        )}
        {toggle(
          'record automatically',
          'auto-record',
          noteCfg.autoRecord,
          false,
          (v) => void patchNote({ autoRecord: v }),
        )}
        {toggle(
          'ask to record',
          'detect-prompt',
          noteCfg.detectPrompt,
          true,
          (v) => void patchNote({ detectPrompt: v }),
        )}
        {toggle(
          'warn when no audio',
          'notify-no-audio',
          noteCfg.notifyNoAudio,
          true,
          (v) => void patchNote({ notifyNoAudio: v }),
        )}
        {toggle(
          'floating hud',
          'floating-hud',
          noteCfg.floatingHud,
          true,
          (v) => void patchNote({ floatingHud: v }),
        )}
        {toggle(
          'delete old recordings',
          'retain-audio-on',
          days > 0,
          false,
          (v) => void patchNote({ retainAudioDays: v ? 30 : 0 }),
        )}
        {days > 0 ? (
          <SettingRow label="after">
            <Select
              value={String(days)}
              onChange={(e) =>
                void patchNote({ retainAudioDays: Number(e.target.value) })
              }
              data-testid="retain-audio"
            >
              <option value="7">7 days</option>
              <option value="30">30 days</option>
              <option value="90">90 days</option>
            </Select>
          </SettingRow>
        ) : null}
      </SettingsSection>
      <SettingsSection title="summaries">
        {toggle(
          'summarize after meetings',
          'auto-summarize',
          noteCfg.autoSummarize,
          true,
          (v) => void patchNote({ autoSummarize: v }),
        )}
        <SettingRow label="template">
          <Select
            value={noteCfg.defaultTemplate ?? 'general'}
            onChange={(e) =>
              void patchNote({ defaultTemplate: e.target.value })
            }
            data-testid="default-template"
          >
            {SUMMARY_TEMPLATES.map((tp) => (
              <option key={tp.id} value={tp.id}>
                {tp.label}
              </option>
            ))}
          </Select>
        </SettingRow>
        <Text variant="caption" muted data-testid="summary-availability">
          summaries are written on this mac by apple intelligence. nothing is
          sent anywhere.
        </Text>
      </SettingsSection>
      <SettingsSection title="echo" data-testid="talk-settings">
        {toggle(
          'dictation',
          'talk-enabled',
          talkCfg.enabled,
          true,
          (v) => void patchTalk({ enabled: v }),
        )}
        <SettingRow label="hold to dictate">
          <Select
            value={talkCfg.pushKey ?? 'fn'}
            onChange={(e) => void patchTalk({ pushKey: e.target.value })}
            data-testid="talk-push-key"
          >
            <option value="fn">fn</option>
            <option value="right-option">right ⌥</option>
            <option value="right-command">right ⌘</option>
            <option value="right-control">right ⌃</option>
            <option value="">off</option>
          </Select>
        </SettingRow>
        {(talkCfg.pushKey ?? 'fn') === 'fn'
          ? toggle(
              'fn only for echo (no input switch or emoji)',
              'talk-block-globe',
              talkCfg.blockGlobeKey,
              true,
              (v) => void patchTalk({ blockGlobeKey: v }),
            )
          : null}
        <Field label="toggle shortcut">
          <Input
            key={talkCfg.toggleHotkey ?? ''}
            defaultValue={
              talkCfg.toggleHotkey ?? 'CommandOrControl+Shift+Space'
            }
            data-testid="talk-toggle-hotkey"
            onBlur={(e) =>
              void patchTalk({ toggleHotkey: e.target.value.trim() })
            }
          />
        </Field>
        {toggle(
          'warm mic',
          'talk-warm-mic',
          talkCfg.warmMic,
          false,
          (v) => void patchTalk({ warmMic: v }),
        )}
        {toggle(
          'clean up text',
          'talk-auto-cleanup',
          talkCfg.autoCleanup,
          true,
          (v) => void patchTalk({ autoCleanup: v }),
        )}
        {toggle(
          'insert into app',
          'talk-auto-insert',
          talkCfg.autoInsert,
          true,
          (v) => void patchTalk({ autoInsert: v }),
        )}
        {toggle(
          'restore clipboard',
          'talk-restore-clipboard',
          talkCfg.restoreClipboard,
          true,
          (v) => void patchTalk({ restoreClipboard: v }),
        )}
        {toggle(
          'floating pill',
          'talk-show-pill',
          talkCfg.showPill,
          true,
          (v) => void patchTalk({ showPill: v }),
        )}
        {toggle(
          'ignore the speakers',
          'talk-echo-gate',
          talkCfg.echoGate,
          true,
          (v) => void patchTalk({ echoGate: v }),
        )}
        {toggle(
          'keep history',
          'talk-save-history',
          talkCfg.saveHistory,
          true,
          (v) => void patchTalk({ saveHistory: v }),
        )}
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
          never leave it. it only goes online to download signed speech updates.
        </Text>
        <Field label="never record in">
          <Input
            key={(privacy.excludedApps ?? []).join(',')}
            defaultValue={(privacy.excludedApps ?? []).join(', ')}
            placeholder="safari, zoom"
            data-testid="excluded-apps"
            onBlur={(e) =>
              void patchPrivacy({
                excludedApps: e.target.value
                  .split(',')
                  .map((x) => x.trim())
                  .filter(Boolean),
              })
            }
          />
        </Field>
        <Field label="pause shortcut">
          <Input
            key={privacy.pauseHotkey ?? ''}
            defaultValue={privacy.pauseHotkey ?? 'CommandOrControl+Shift+P'}
            data-testid="pause-hotkey"
            onBlur={(e) =>
              void patchPrivacy({ pauseHotkey: e.target.value.trim() })
            }
          />
        </Field>
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
