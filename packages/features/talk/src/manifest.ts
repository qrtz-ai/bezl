import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'talk',
  name: 'talk',
  icon: 'mic',
  order: 20,
  capabilities: ['talk.read', 'talk.dictate', 'talk.write'],
  permissions: ['microphone', 'accessibility'],
});
