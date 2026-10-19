import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'note',
  name: 'nano',
  icon: 'edit-3',
  order: 20,
  capabilities: [
    'note.read',
    'note.record',
    'note.write',
    'calendar.read',
    'os.permissions',
    'speech.read',
    'module.settings',
  ],
  permissions: ['accessibility', 'screen-recording', 'microphone'],
});
