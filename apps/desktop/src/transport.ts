// Installs `window.qrtzTransport` (the only platform seam the React app uses). Every page entry (index, hud)
// calls this first, then renders only its own view.
import { Channel, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createTauriTransport, type TauriBridge } from '@qrtz/core';

async function bridge(): Promise<TauriBridge> {
  // Mocked IPC (Playwright against the dev UI, `bun run dev:mock`): an in-page backend behind the same
  // Tauri bridge, so createTauriTransport + capability checks run for real. Never in production builds.
  if (
    import.meta.env.DEV &&
    (import.meta.env.VITE_QRTZ_MOCK_IPC === '1' ||
      new URLSearchParams(window.location.search).has('mockIpc'))
  ) {
    const { createMockBridge } = await import('./mock/backend');
    return createMockBridge();
  }
  return {
    invoke,
    listen,
    channel: (onmessage) => {
      const c = new Channel<unknown>();
      c.onmessage = onmessage;
      return c;
    },
  };
}

export async function installTransport(): Promise<void> {
  window.qrtzTransport = createTauriTransport(await bridge());
}
