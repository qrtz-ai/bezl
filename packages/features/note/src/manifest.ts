import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'note',
  name: 'Note',
  icon: 'mic',
  order: 10,
  capabilities: ['note.read', 'note.record', 'note.write', 'os.permissions'],
  permissions: ['accessibility', 'screen-recording', 'microphone'],
});
