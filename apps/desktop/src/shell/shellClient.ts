import { createClient, type QrtzTransport } from '@qrtz/core';

// Lives here, not in SubAppHost, so the HUD and pill pages don't bundle the sub app host (ADR 0088).
export const noTransport: QrtzTransport = {
  invoke: async (e) => ({
    ok: false,
    id: e.id,
    error: {
      name: 'AppError',
      code: 'E_UNAVAILABLE',
      message: 'Not running inside qrtz',
      retryable: false,
    },
  }),
  on: () => () => {},
};

/** IPC client for shell-owned UI (⌘K palette, floating HUD), scoped to the shell manifest's capabilities. */
export const shellClient = () =>
  createClient(window.qrtzTransport ?? noTransport, 'shell');
