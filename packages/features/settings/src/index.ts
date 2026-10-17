import { defineSubApp } from '@qrtz/core';

export default defineSubApp({
  manifest: { id: 'settings', name: 'Settings', icon: '⚙', order: 1000 },
  load: () => import('./SettingsView'),
});
