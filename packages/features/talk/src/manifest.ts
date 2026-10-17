import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'talk',
  name: 'Talk',
  icon: 'mic',
  order: 20,
  capabilities: ['talk.read', 'talk.dictate', 'talk.write'],
  permissions: ['microphone', 'accessibility'],
});
