import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Same renderer as apps/desktop (workspace packages ship TS source; Vite compiles them directly).
// Port 1420 is what src-tauri/tauri.conf.json's devUrl points at.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'safari16',
    outDir: 'dist',
    emptyOutDir: true,
    // One page per window (ADR 0088): the HUD and the pill never load the shell or its sub apps.
    rollupOptions: {
      input: { main: 'index.html', hud: 'hud.html', pill: 'pill.html' },
    },
  },
});
