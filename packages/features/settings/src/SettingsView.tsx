import { useEffect, useState } from 'react';
import { Button, Text } from '@qrtz/ui';
import type { IpcResponse } from '@qrtz/core';
import { useQrtz, useQrtzEvent } from '@qrtz/core/react';

export default function SettingsView() {
  const qrtz = useQrtz();
  const [info, setInfo] = useState<IpcResponse<'app:get-info'> | null>(null);
  const [db, setDb] = useState<IpcResponse<'db:health'> | null>(null);
  const [helper, setHelper] = useState<IpcResponse<'helper:status'> | null>(
    null,
  );
  const live = useQrtzEvent('helper:status');
  const [error, setError] = useState<string | null>(null);
  const refresh = () => {
    setError(null);
    Promise.all([
      qrtz.invoke('app:get-info'),
      qrtz.invoke('db:health'),
      qrtz.invoke('helper:status'),
    ])
      .then(([i, d, h]) => {
        setInfo(i);
        setDb(d);
        setHelper(h);
      })
      .catch((e: Error) => setError(e.message));
  };
  useEffect(refresh, [qrtz]);
  const h = live ?? helper;
  return (
    <div style={{ display: 'grid', gap: 12, justifyItems: 'start' }}>
      <Text as="h1" size="xl" weight={600}>
        Settings
      </Text>
      <Text muted>
        {info
          ? `qrtz ${info.version} · ${info.platform}/${info.arch} · Electron ${info.electron}`
          : 'Loading…'}
      </Text>
      <Text muted>
        {db
          ? `Database: ${db.ok ? 'ok' : 'error'} · schema v${db.schemaVersion} · FTS5 ${db.fts5 ? 'on' : 'off'}`
          : ''}
      </Text>
      <Text muted>
        {h ? `Native helper: ${h.state} (restarts ${h.restarts})` : ''}
      </Text>
      {error ? <Text style={{ color: '#f87171' }}>{error}</Text> : null}
      <Button variant="ghost" onClick={refresh} data-testid="settings-refresh">
        Refresh
      </Button>
    </div>
  );
}
