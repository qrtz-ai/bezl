import { defineSubApp } from '@qrtz/core';

export default defineSubApp({
  manifest: { id: 'home', name: 'Home', icon: '◆', order: 0 },
  load: () => import('./HomeView'),
});
