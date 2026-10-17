import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'talk',
  name: 'echo',
  icon: 'mic',
  order: 20,
  capabilities: ['talk.read', 'talk.dictate', 'talk.write', 'speech.read'],
  permissions: ['microphone', 'accessibility'],
});
