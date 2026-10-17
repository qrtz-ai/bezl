import { defineConfig } from 'vite';

// Main + worker bundles. node:sqlite is newer than Vite's builtin list, so mark it external.
export default defineConfig({
  // Auto-update feed baked in by scripts/release-mac.sh; empty/false = off (the default). See docs/release.md.
  define: {
    __QRTZ_UPDATE_FEED_URL__: JSON.stringify(
      process.env.QRTZ_UPDATE_FEED_URL ?? '',
    ),
    __QRTZ_AUTO_UPDATE__: JSON.stringify(process.env.QRTZ_AUTO_UPDATE === '1'),
  },
  build: {
    // transformers.js (+ onnxruntime-node native addon) stays external and is loaded lazily by the worker;
    // if it's absent at runtime, chat falls back to full-text retrieval.
    rollupOptions: {
      external: ['node:sqlite', '@huggingface/transformers'],
    },
  },
});
