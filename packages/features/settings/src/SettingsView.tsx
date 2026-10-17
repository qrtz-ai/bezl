import { useEffect, useState } from 'react';
import {
  Badge,
  Button,
  Card,
  Field,
  Heading,
  Input,
  Select,
  Text,
} from '@qrtz/ui';
import {
  PROVIDER_DEFAULTS,
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
    Promise.all([
      qrtz.invoke('app:get-info'),
      qrtz.invoke('db:health'),
      qrtz.invoke('helper:status'),
      qrtz.invoke('config:get'),
      qrtz.invoke('secrets:list'),
    ])
      .then(([i, d, h, c, s]) => {
        setInfo(i);
        setDb(d);
        setHelper(h);
        setCfg(c);
        setSecrets(s);
      })
      .catch((e: Error) => setError(e.message));
  };
  useEffect(refresh, [qrtz]);
  const h = live ?? helper;
  const memoCfg = (cfg?.memo ?? {}) as {
    llmProvider?: ProviderId | 'none';
    llmModel?: string;
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
      {error ? <Text style={{ color: '#f87171' }}>{error}</Text> : null}
    </div>
  );
}
