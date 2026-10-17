import { useEffect, useState } from 'react';
import {
  Badge,
  Button,
  Callout,
  Card,
  Field,
  Heading,
  Input,
  Select,
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
  const memoCfg = (cfg?.memo ?? {}) as {
    llmProvider?: ProviderId | 'none';
    llmModel?: string;
    retainAudioDays?: number;
    autoRecord?: boolean;
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
                ? `Backed up ${r.meetings} meetings and ${r.audioFiles} recordings (${(r.bytes / 1e6).toFixed(1)} MB) to ${r.savedTo}.`
                : 'Backup canceled.',
            )
        : qrtz
            .invoke('data:import-backup', {})
            .then((r) =>
              r.canceled
                ? 'Import canceled.'
                : `Verified and imported ${r.imported} meetings (${r.skipped} already here, ${r.audioFiles} recordings).`,
            );
    void run
      .then(setBackupMsg, (e: Error) => setBackupMsg(e.message))
      .finally(() => setBackupBusy(false));
  };
  const provider = memoCfg.llmProvider ?? 'openrouter';
  const secretName =
    provider === 'none' ? undefined : PROVIDER_DEFAULTS[provider].secret;
  const hasKey =
    !!secretName &&
    secrets.some((s) => s === secretName || s.startsWith('QRTZ_SECRET_'));

  const patch = (p: Record<string, unknown>) =>
    qrtz
      .invoke('config:set', { patch: { memo: p } })
      .then(setCfg)
      .catch((e: Error) => setError(e.message));

  const saveKey = async (value: string | null) => {
    if (!secretName) return;
    try {
      await qrtz.invoke('secrets:set', { name: secretName, value });
      setKey('');
      setMsg(
        value
          ? 'Key saved to the macOS Keychain-backed store.'
          : 'Key removed.',
      );
      setSecrets(await qrtz.invoke('secrets:list'));
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <div style={{ display: 'grid', gap: 20, maxWidth: 640 }}>
      <Heading>Settings</Heading>
      <Card padding={4} style={{ display: 'grid', gap: 12 }}>
        <Text weight={600}>Recordings</Text>
        <Field
          label="When a meeting starts"
          hint="qrtz detects Zoom and Google Meet calls without a calendar."
        >
          <Select
            value={memoCfg.autoRecord ? 'auto' : 'ask'}
            onChange={(e) =>
              void patch({ autoRecord: e.target.value === 'auto' })
            }
            data-testid="auto-record"
          >
            <option value="ask">Ask me to record</option>
            <option value="auto">Always record automatically</option>
          </Select>
        </Field>
        <Field
          label="Keep recordings"
          hint="Transcripts and summaries are kept. Only the audio is deleted."
        >
          <Select
            value={String(memoCfg.retainAudioDays ?? 0)}
            onChange={(e) =>
              void patch({ retainAudioDays: Number(e.target.value) })
            }
            data-testid="retain-audio"
          >
            <option value="0">Forever</option>
            <option value="7">7 days</option>
            <option value="30">30 days</option>
            <option value="90">90 days</option>
          </Select>
        </Field>
        <Field
          label="Default summary template"
          hint="Change it per meeting in the meeting's Summary tab."
        >
          <Select
            value={memoCfg.defaultTemplate ?? 'general'}
            onChange={(e) => void patch({ defaultTemplate: e.target.value })}
            data-testid="default-template"
          >
            {SUMMARY_TEMPLATES.map((tp) => (
              <option key={tp.id} value={tp.id}>
                {tp.label}
              </option>
            ))}
          </Select>
        </Field>
      </Card>
      <Card
        padding={4}
        style={{ display: 'grid', gap: 12 }}
        data-testid="privacy-card"
      >
        <Text weight={600}>Privacy</Text>
        <label style={{ display: 'flex', gap: 8, alignItems: 'flex-start' }}>
          <input
            type="checkbox"
            checked={!!privacy.localOnly}
            onChange={(e) => void patchPrivacy({ localOnly: e.target.checked })}
            data-testid="local-only"
          />
          <span>
            <Text>Never send audio or transcripts to the cloud</Text>
            <br />
            <Text size="sm" muted>
              Summaries and chat use the on-device summarizer (or Ollama on this
              Mac). Cloud API keys are not used while this is on.
            </Text>
          </span>
        </label>
        {privacy.localOnly ? (
          <Badge tone="success">
            <span data-testid="local-only-badge">Local-only mode</span>
          </Badge>
        ) : null}
        <Field
          label="Never record meetings in"
          hint="Comma-separated apps or platforms, e.g. Safari, zoom, google-meet."
        >
          <Input
            key={(privacy.excludedApps ?? []).join(',')}
            defaultValue={(privacy.excludedApps ?? []).join(', ')}
            placeholder="None"
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
        <Field
          label="Pause capture shortcut"
          hint="Global shortcut that pauses and resumes capture while recording. Leave empty to turn off."
        >
          <Input
            key={privacy.pauseHotkey ?? ''}
            defaultValue={privacy.pauseHotkey ?? 'CommandOrControl+Shift+P'}
            data-testid="pause-hotkey"
            onBlur={(e) =>
              void patchPrivacy({ pauseHotkey: e.target.value.trim() })
            }
          />
        </Field>
      </Card>
      <Card
        padding={4}
        style={{ display: 'grid', gap: 12 }}
        data-testid="backup-card"
      >
        <Text weight={600}>Backup</Text>
        <Text muted size="sm">
          Export everything (database and recordings) to one zip with integrity
          checksums. Importing verifies the zip and adds meetings that
          aren&apos;t already here; nothing is overwritten.
        </Text>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button
            variant="secondary"
            disabled={backupBusy}
            onClick={() => backup('export')}
            data-testid="backup-export"
          >
            Export backup…
          </Button>
          <Button
            variant="ghost"
            disabled={backupBusy}
            onClick={() => backup('import')}
            data-testid="backup-import"
          >
            Import backup…
          </Button>
        </div>
        {backupMsg ? (
          <Text size="sm" muted data-testid="backup-result" role="status">
            {backupMsg}
          </Text>
        ) : null}
      </Card>
      <Card padding={4} style={{ display: 'grid', gap: 12 }}>
        <Text weight={600}>AI summaries</Text>
        <Text muted size="sm">
          After a meeting, Memo sends the transcript to this provider for a
          summary, action items and Q&amp;A. Without a key it uses the on-device
          extractive summarizer.
        </Text>
        <Field label="Provider">
          <Select
            value={provider}
            onChange={(e) => void patch({ llmProvider: e.target.value })}
            data-testid="llm-provider"
          >
            {PROVIDERS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </Select>
        </Field>
        {provider !== 'none' ? (
          <Field
            label="Model"
            hint={`Default: ${PROVIDER_DEFAULTS[provider].model}`}
          >
            <Input
              defaultValue={memoCfg.llmModel ?? ''}
              placeholder={PROVIDER_DEFAULTS[provider].model}
              onBlur={(e) => void patch({ llmModel: e.target.value.trim() })}
            />
          </Field>
        ) : null}
        {secretName ? (
          <Field
            label="API key"
            hint={
              provider === 'openrouter'
                ? 'Or set QRTZ_SECRET_OPENROUTER in the environment.'
                : undefined
            }
          >
            <div style={{ display: 'flex', gap: 8 }}>
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
            </div>
          </Field>
        ) : null}
        <div>
          {hasKey ? (
            <Badge tone="success">Key configured</Badge>
          ) : (
            <Badge tone="neutral">Using extractive fallback</Badge>
          )}
        </div>
        {msg ? (
          <Text size="sm" muted>
            {msg}
          </Text>
        ) : null}
      </Card>
      <Card padding={4} style={{ display: 'grid', gap: 6 }}>
        <Text weight={600}>System</Text>
        <Text muted size="sm">
          {info
            ? `qrtz ${info.version} · ${info.platform}/${info.arch} · Electron ${info.electron}`
            : 'Loading…'}
        </Text>
        <Text muted size="sm">
          {db
            ? `Database: ${db.ok ? 'ok' : 'error'} · schema v${db.schemaVersion} · FTS5 ${db.fts5 ? 'on' : 'off'}`
            : ''}
        </Text>
        <Text muted size="sm">
          {h ? `Native helper: ${h.state} (restarts ${h.restarts})` : ''}
        </Text>
        <div>
          <Button
            variant="ghost"
            size="sm"
            onClick={refresh}
            data-testid="settings-refresh"
          >
            Refresh
          </Button>
        </div>
      </Card>
      {error ? (
        <Callout
          tone="danger"
          title="Some settings couldn't be loaded or saved"
          data-testid="settings-error"
          action={
            <Button variant="secondary" size="sm" onClick={refresh}>
              Retry
            </Button>
          }
        >
          {error}
        </Callout>
      ) : null}
    </div>
  );
}
