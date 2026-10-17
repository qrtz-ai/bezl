import { defineConfig } from 'vite';

// Main + worker bundles. node:sqlite is newer than Vite's builtin list, so mark it external.
export default defineConfig({
  build: {
    // transformers.js (+ onnxruntime-node native addon) stays external and is loaded lazily by the worker;
    // if it's absent at runtime, chat falls back to full-text retrieval.
    rollupOptions: {
      external: ['node:sqlite', '@huggingface/transformers'],
    },
  },
});
