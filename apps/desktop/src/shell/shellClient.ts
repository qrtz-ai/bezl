import { createClient } from '@qrtz/core';
import { noTransport } from './SubAppHost';

/** IPC client for shell-owned UI (⌘K palette, floating HUD), scoped to the shell manifest's capabilities. */
export const shellClient = () =>
  createClient(window.qrtzTransport ?? noTransport, 'shell');
