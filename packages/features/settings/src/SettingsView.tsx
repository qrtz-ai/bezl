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
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

const PROVIDERS: { id: ProviderId | 'none'; label: string }[] = [
  { id: 'openrouter', label: 'OpenRouter (recommended)' },
  { id: 'openai', label: 'OpenAI' },
  { id: 'anthropic', label: 'Anthropic' },
  { id: 'google', label: 'Google Gemini' },
  { id: 'ollama', label: 'Ollama (local, no key)' },
  { id: 'none', label: 'Off (on-device extractive only)' },
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
    floatingHud?: boolean;
    autoSummarize?: boolean;
    backgroundIndexing?: boolean;
    defaultTemplate?: string;
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
                ? `Backed up ${r.meetings} meetings and ${r.audioFiles} recordings (${(r.bytes / 1e6).toFixed(1)} MB)`
                : 'Canceled',
            )
        : qrtz
            .invoke('data:import-backup', {})
            .then((r) =>
              r.canceled
                ? 'Canceled'
                : `Verified and imported ${r.imported} meetings (${r.skipped} already here, ${r.audioFiles} recordings)`,
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
      setMsg(value ? 'Key saved' : 'Key removed');
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
  const asrSecret =
    asrEngine === 'deepgram' || asrEngine === 'elevenlabs'
      ? `asr.${asrEngine}`
      : undefined;
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
              Retry
            </Button>
          }
        />
      ) : null}
      <SettingsSection title="recording">
        {toggle(
          'Detect meetings',
          'auto-detect',
          noteCfg.autoDetectMeetings,
          true,
          (v) => void patchNote({ autoDetectMeetings: v }),
        )}
        {toggle(
          'Record automatically',
          'auto-record',
          noteCfg.autoRecord,
          false,
          (v) => void patchNote({ autoRecord: v }),
        )}
        {toggle(
          'Ask to record',
          'detect-prompt',
          noteCfg.detectPrompt,
          true,
          (v) => void patchNote({ detectPrompt: v }),
        )}
        {toggle(
          'Floating HUD',
          'floating-hud',
          noteCfg.floatingHud,
          true,
          (v) => void patchNote({ floatingHud: v }),
        )}
        {toggle(
          'Delete old recordings',
          'retain-audio-on',
          days > 0,
          false,
          (v) => void patchNote({ retainAudioDays: v ? 30 : 0 }),
        )}
        {days > 0 ? (
          <SettingRow label="After">
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
          'Summarize after meetings',
          'auto-summarize',
          noteCfg.autoSummarize,
          true,
          (v) => void patchNote({ autoSummarize: v }),
        )}
        {toggle(
          'Index for search',
          'background-indexing',
          noteCfg.backgroundIndexing,
          true,
          (v) => void patchNote({ backgroundIndexing: v }),
        )}
        <SettingRow label="Template">
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
        <SettingRow label="Provider">
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
          <Field label="Model">
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
          <Field label="API key">
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
                Save
              </Button>
              {hasKey ? (
                <Button variant="ghost" onClick={() => void saveKey(null)}>
                  Remove
                </Button>
              ) : null}
            </Stack>
          </Field>
        ) : null}
        <Text variant="caption" muted>
          {hasKey ? 'Key configured' : 'Using extractive fallback'}
        </Text>
        {msg ? (
          <Text variant="caption" muted>
            {msg}
          </Text>
        ) : null}
      </SettingsSection>
      <SettingsSection title="talk" data-testid="talk-settings">
        {toggle(
          'Dictation',
          'talk-enabled',
          talkCfg.enabled,
          true,
          (v) => void patchTalk({ enabled: v }),
        )}
        <SettingRow label="Hold to talk">
          <Select
            value={talkCfg.pushKey ?? 'fn'}
            onChange={(e) => void patchTalk({ pushKey: e.target.value })}
            data-testid="talk-push-key"
          >
            <option value="fn">fn</option>
            <option value="right-option">Right ⌥</option>
            <option value="right-command">Right ⌘</option>
            <option value="right-control">Right ⌃</option>
            <option value="">Off</option>
          </Select>
        </SettingRow>
        <Field label="Toggle shortcut">
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
        <SettingRow label="Speech engine">
          <Select
            value={asrEngine}
            onChange={(e) => void patchTalk({ engine: e.target.value })}
            data-testid="talk-engine"
          >
            <option value="auto">Auto</option>
            <option value="elevenlabs">ElevenLabs Scribe</option>
            <option value="deepgram">Deepgram Nova-3</option>
            <option value="on-device">On-device</option>
          </Select>
        </SettingRow>
        {asrSecret ? (
          <Field label="Engine key">
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
                Save
              </Button>
              {hasAsrKey ? (
                <Button variant="ghost" onClick={() => void saveAsrKey(null)}>
                  Remove
                </Button>
              ) : null}
            </Stack>
          </Field>
        ) : null}
        {toggle(
          'Fall back to on-device',
          'talk-auto-failover',
          talkCfg.autoFailover,
          true,
          (v) => void patchTalk({ autoFailover: v }),
        )}
        {toggle(
          'Warm mic',
          'talk-warm-mic',
          talkCfg.warmMic,
          false,
          (v) => void patchTalk({ warmMic: v }),
        )}
        {toggle(
          'Clean up text',
          'talk-auto-cleanup',
          talkCfg.autoCleanup,
          true,
          (v) => void patchTalk({ autoCleanup: v }),
        )}
        {toggle(
          'AI cleanup',
          'talk-ai-cleanup',
          talkCfg.aiCleanup,
          true,
          (v) => void patchTalk({ aiCleanup: v }),
        )}
        {toggle(
          'Insert into app',
          'talk-auto-insert',
          talkCfg.autoInsert,
          true,
          (v) => void patchTalk({ autoInsert: v }),
        )}
        {toggle(
          'Restore clipboard',
          'talk-restore-clipboard',
          talkCfg.restoreClipboard,
          true,
          (v) => void patchTalk({ restoreClipboard: v }),
        )}
        {toggle(
          'Floating pill',
          'talk-show-pill',
          talkCfg.showPill,
          true,
          (v) => void patchTalk({ showPill: v }),
        )}
        {toggle(
          'Keep history',
          'talk-save-history',
          talkCfg.saveHistory,
          true,
          (v) => void patchTalk({ saveHistory: v }),
        )}
      </SettingsSection>
      <SettingsSection title="privacy" data-testid="privacy-card">
        {toggle(
          'Local only',
          'local-only',
          privacy.localOnly,
          false,
          (v) => void patchPrivacy({ localOnly: v }),
        )}
        <Field label="Never record in">
          <Input
            key={(privacy.excludedApps ?? []).join(',')}
            defaultValue={(privacy.excludedApps ?? []).join(', ')}
            placeholder="Safari, zoom"
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
        <Field label="Pause shortcut">
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
        <SettingRow label="Theme">
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
            <option value="dark">Dark</option>
            <option value="light">Light</option>
            <option value="system">System</option>
          </Select>
        </SettingRow>
        {toggle(
          'Menu bar icon',
          'menu-bar-icon',
          appCfg.menuBarIcon,
          true,
          (v) => void patchApp({ menuBarIcon: v }),
        )}
        {toggle(
          'Open at login',
          'launch-at-login',
          appCfg.launchAtLogin,
          false,
          (v) => void patchApp({ launchAtLogin: v }),
        )}
        {toggle(
          'Check for updates',
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
            Export
          </Button>
          <Button
            variant="ghost"
            disabled={backupBusy}
            onClick={() => backup('import')}
            data-testid="backup-import"
          >
            Import
          </Button>
        </Stack>
        {backupBusy && backupProgress && backupProgress.phase !== 'done' ? (
          <Stack gap={1} data-testid="backup-progress">
            <Progress
              value={backupProgress.done / Math.max(1, backupProgress.total)}
              label={`Backup ${backupProgress.kind} progress`}
            />
            <Text variant="caption" muted>
              {
                {
                  checksum: 'Computing checksums…',
                  write: 'Writing zip…',
                  verify: 'Verifying backup…',
                  import: 'Importing meetings…',
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
            ? `qrtz ${info.version} · ${info.platform}/${info.arch} · Electron ${info.electron}`
            : '…'}
        </Text>
        <Text variant="caption" muted>
          {db
            ? `Database: ${db.ok ? 'ok' : 'error'} · schema v${db.schemaVersion} · FTS5 ${db.fts5 ? 'on' : 'off'}`
            : ''}
        </Text>
        <Text variant="caption" muted>
          {h ? `Native helper: ${h.state} (restarts ${h.restarts})` : ''}
        </Text>
        <Stack dir="row">
          <Button
            variant="ghost"
            size="sm"
            onClick={refresh}
            data-testid="settings-refresh"
          >
            Refresh
          </Button>
        </Stack>
      </SettingsSection>
    </Page>
  );
}
