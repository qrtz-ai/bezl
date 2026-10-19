import { SubAppManifest } from '@qrtz/core';

/** Agent Dash (ADR 0228): the read-only dashboard your agents write to. Behind the dev flag for now. */
export const manifest = SubAppManifest.parse({
  id: 'dash',
  name: 'dash',
  icon: 'grid',
  order: 40,
  capabilities: ['dash.read', 'dash.manage'],
});
