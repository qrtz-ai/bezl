import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// react-router's package exports only name its development build (dev warnings, ~2x the size). Builds use
// the production one (ADR 0093).
const routerProd = join(
  dirname(createRequire(import.meta.url).resolve('react-router/package.json')),
  'dist/production/index.mjs',
);

// Same renderer as apps/desktop (workspace packages ship TS source; Vite compiles them directly).
// Port 1420 is what src-tauri/tauri.conf.json's devUrl points at.
export default defineConfig(({ command }) => ({
  plugins: [react()],
  resolve: {
    alias:
      command === 'build'
        ? [{ find: /^react-router$/, replacement: routerProd }]
        : [],
  },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    target: 'safari16',
    outDir: 'dist',
    emptyOutDir: true,
    // One page per window (ADR 0090): the HUD never loads the shell or its sub apps. echo's pill is native
    // (the helper draws it, ADR 0092).
    rollupOptions: {
      input: { main: 'index.html', hud: 'hud.html' },
    },
  },
}));
