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
    'doc.read',
    'os.permissions',
    'speech.read',
    'config.read',
    'config.write',
    'module.settings',
  ],
  permissions: ['accessibility', 'screen-recording', 'microphone'],
});
