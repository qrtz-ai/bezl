import { SubAppManifest } from '@qrtz/core';

export const manifest = SubAppManifest.parse({
  id: 'talk',
  name: 'echo',
  icon: 'mic',
  order: 10,
  capabilities: [
    'talk.read',
    'talk.dictate',
    'talk.write',
    'speech.read',
    // ADR 0174: echo is bundled; its view holds its switch (config:set of modules.echo.enabled only, enforced by
    // the host) and asks to download Parakeet when switched on.
    'speech.manage',
    'config.read',
    'config.write',
    'module.settings',
  ],
  permissions: ['microphone', 'accessibility'],
});
