import { useEffect, useState } from 'react';
import { Button, Text } from '@qrtz/ui';
import type { IpcResponse } from '@qrtz/core';

export default function SettingsView() {
  const [info, setInfo] = useState<IpcResponse<'app:get-info'> | null>(null);
  const refresh = () => {
    void window.qrtz?.invoke('app:get-info').then(setInfo);
  };
  useEffect(refresh, []);
  return (
    <div style={{ display: 'grid', gap: 12, justifyItems: 'start' }}>
      <Text as="h1" size="xl" weight={600}>
        Settings
      </Text>
      <Text muted>
        {info
          ? `qrtz ${info.version} on ${info.platform}`
          : 'Loading app info…'}
      </Text>
      <Button variant="ghost" onClick={refresh}>
        Refresh
      </Button>
    </div>
  );
}
