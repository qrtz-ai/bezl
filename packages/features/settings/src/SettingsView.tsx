import { useEffect, useState } from 'react';
import {
  Button,
  Callout,
  Field,
  Heading,
  Input,
  Page,
  Progress,
  SearchModelRow,
  Select,
  SettingRow,
  SettingsSection,
  SpeechSetupPanel,
  Stack,
  Switch,
  Text,
} from '@qrtz/ui';
import {
  PROVIDER_DEFAULTS,
  SUMMARY_TEMPLATES,
  type IpcResponse,
  type ProviderId,
} from '@qrtz/core';
import { useQrtz, useQrtzEvent, useSpeechSetup } from '@qrtz/core/react';

const PROVIDERS: { id: ProviderId | 'none'; label: string }[] = [
  { id: 'openrouter', label: 'openrouter (recommended)' },
  { id: 'openai', label: 'openai' },
  { id: 'anthropic', label: 'anthropic' },
  { id: 'google', label: 'google gemini' },
  { id: 'ollama', label: 'ollama (local, no key)' },
  { id: 'none', label: 'off (on-device extractive only)' },
];

export default function SettingsView() {
  const qrtz = useQrtz();
  const [info, setInfo] = useState<IpcResponse<'app:get-info'> | null>(null);
  const [db, setDb] = useState<IpcResponse<'db:health'> | null>(null);
  const [helper, setHelper] = useState<IpcResponse<'helper:status'> | null>(
    null,
  );
  const [cfg, setCfg] = useState<Record<string, unknown> | null>(null);
  const [secrets, setSecrets] = useState<string[]>([]);
  const [key, setKey] = useState('');
  const [msg, setMsg] = useState<string | null>(null);
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
      load(qrtz.invoke('secrets:list'), setSecrets, 'keys'),
    ]).then(() => {
      if (fails.length) setError(fails.join(' · '));
    });
  };
  useEffect(refresh, [qrtz]);
  const h = live ?? helper;
  const noteCfg = (cfg?.note ?? {}) as {
    llmProvider?: ProviderId | 'none';
    llmModel?: string;
    retainAudioDays?: number;
    autoRecord?: boolean;
    autoDetectMeetings?: boolean;
    detectPrompt?: boolean;
    notifyNoAudio?: boolean;
    floatingHud?: boolean;
    autoSummarize?: boolean;
    backgroundIndexing?: boolean;
    defaultTemplate?: string;
    searchModelDownload?: boolean;
  };
  const privacy = (cfg?.privacy ?? {}) as {
    localOnly?: boolean;
    excludedApps?: string[];
    pauseHotkey?: string;
  };
  const [backupMsg, setBackupMsg] = useState<string | null>(null);
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
  const provider = noteCfg.llmProvider ?? 'openrouter';
  const secretName =
    provider === 'none' ? undefined : PROVIDER_DEFAULTS[provider].secret;
  const hasKey =
    !!secretName &&
    secrets.some((s) => s === secretName || s.startsWith('QRTZ_SECRET_'));

  const patch = (p: Record<string, unknown>) =>
    qrtz
      .invoke('config:set', { patch: { note: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));

  const saveKey = async (value: string | null) => {
    if (!secretName) return;
    try {
      await qrtz.invoke('secrets:set', { name: secretName, value });
      setKey('');
      setMsg(value ? 'key saved' : 'key removed');
      setSecrets(await qrtz.invoke('secrets:list'));
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const appCfg = (cfg?.app ?? {}) as {
    menuBarIcon?: boolean;
    launchAtLogin?: boolean;
    autoUpdateCheck?: boolean;
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
  const patchNote = (p: Record<string, unknown>) => {
    setCfg((prev) =>
      prev ? { ...prev, note: { ...(prev.note as object), ...p } } : prev,
    );
    return patch(p);
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
    engine?: string;
    autoFailover?: boolean;
    autoCleanup?: boolean;
    aiCleanup?: boolean;
    autoInsert?: boolean;
    restoreClipboard?: boolean;
    showPill?: boolean;
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
  const asrEngine = talkCfg.engine ?? 'auto';
  const asrSecret = asrEngine === 'elevenlabs' ? 'asr.elevenlabs' : undefined;
  const [asrKey, setAsrKey] = useState('');
  const hasAsrKey = !!asrSecret && secrets.includes(asrSecret);
  const saveAsrKey = async (value: string | null) => {
    if (!asrSecret) return;
    try {
      await qrtz.invoke('secrets:set', { name: asrSecret, value });
      setAsrKey('');
      setSecrets(await qrtz.invoke('secrets:list'));
    } catch (e) {
      setError((e as Error).message);
    }
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
        {toggle(
          'index for search',
          'background-indexing',
          noteCfg.backgroundIndexing,
          true,
          (v) => void patchNote({ backgroundIndexing: v }),
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
        <SettingRow label="provider">
          <Select
            value={provider}
            onChange={(e) => void patchNote({ llmProvider: e.target.value })}
            data-testid="llm-provider"
          >
            {PROVIDERS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </Select>
        </SettingRow>
        {provider !== 'none' ? (
          <Field label="model">
            <Input
              defaultValue={noteCfg.llmModel ?? ''}
              placeholder={PROVIDER_DEFAULTS[provider].model}
              onBlur={(e) =>
                void patchNote({ llmModel: e.target.value.trim() })
              }
            />
          </Field>
        ) : null}
        {secretName ? (
          <Field label="api key">
            <Stack dir="row" gap={2}>
              <Input
                type="password"
                value={key}
                onChange={(e) => setKey(e.target.value)}
                placeholder={hasKey ? '•••••••• (saved)' : 'sk-…'}
                data-testid="llm-key"
              />
              <Button
                onClick={() => void saveKey(key.trim())}
                disabled={!key.trim()}
              >
                save
              </Button>
              {hasKey ? (
                <Button variant="ghost" onClick={() => void saveKey(null)}>
                  remove
                </Button>
              ) : null}
            </Stack>
          </Field>
        ) : null}
        <Text variant="caption" muted>
          {hasKey ? 'key configured' : 'using extractive fallback'}
        </Text>
        {msg ? (
          <Text variant="caption" muted>
            {msg}
          </Text>
        ) : null}
      </SettingsSection>
      <SettingsSection title="talk" data-testid="talk-settings">
        {toggle(
          'dictation',
          'talk-enabled',
          talkCfg.enabled,
          true,
          (v) => void patchTalk({ enabled: v }),
        )}
        <SettingRow label="hold to talk">
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
              'fn only for talk (no input switch or emoji)',
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
        <SettingRow label="speech engine">
          <Select
            value={asrEngine}
            onChange={(e) => void patchTalk({ engine: e.target.value })}
            data-testid="talk-engine"
          >
            <option value="auto">auto</option>
            <option value="elevenlabs">elevenlabs scribe</option>
            <option value="on-device">on-device</option>
          </Select>
        </SettingRow>
        {asrSecret ? (
          <Field label="engine key">
            <Stack dir="row" gap={2}>
              <Input
                type="password"
                value={asrKey}
                onChange={(e) => setAsrKey(e.target.value)}
                placeholder={hasAsrKey ? '•••••••• (saved)' : 'key'}
                data-testid="talk-engine-key"
              />
              <Button
                onClick={() => void saveAsrKey(asrKey.trim())}
                disabled={!asrKey.trim()}
              >
                save
              </Button>
              {hasAsrKey ? (
                <Button variant="ghost" onClick={() => void saveAsrKey(null)}>
                  remove
                </Button>
              ) : null}
            </Stack>
          </Field>
        ) : null}
        {toggle(
          'fall back to on-device',
          'talk-auto-failover',
          talkCfg.autoFailover,
          true,
          (v) => void patchTalk({ autoFailover: v }),
        )}
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
          'ai cleanup',
          'talk-ai-cleanup',
          talkCfg.aiCleanup,
          true,
          (v) => void patchTalk({ aiCleanup: v }),
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
        <SearchModelRow status={setup.model} onDownload={setup.downloadModel} />
        {toggle(
          'download the search model',
          'search-model-auto',
          noteCfg.searchModelDownload,
          true,
          (v) => void patchNote({ searchModelDownload: v }),
        )}
      </SettingsSection>
      <SettingsSection title="privacy" data-testid="privacy-card">
        {toggle(
          'local only',
          'local-only',
          privacy.localOnly,
          false,
          (v) => void patchPrivacy({ localOnly: v }),
        )}
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
        {toggle(
          'check for updates',
          'auto-update-check',
          appCfg.autoUpdateCheck,
          true,
          (v) => void patchApp({ autoUpdateCheck: v }),
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
        <Stack dir="row">
          <Button
            variant="ghost"
            size="sm"
            onClick={refresh}
            data-testid="settings-refresh"
          >
            refresh
          </Button>
        </Stack>
      </SettingsSection>
    </Page>
  );
}
