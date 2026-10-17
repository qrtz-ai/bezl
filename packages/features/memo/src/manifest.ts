import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'memo',
  name: 'Memo',
  icon: 'mic',
  order: 10,
  capabilities: ['memo.read', 'memo.record', 'memo.write', 'os.permissions'],
  permissions: ['accessibility', 'screen-recording', 'microphone'],
});
