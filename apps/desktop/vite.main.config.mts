import { defineConfig } from 'vite';

// Main + worker bundles. node:sqlite is newer than Vite's builtin list, so mark it external.
export default defineConfig({
  build: {
    rollupOptions: { external: ['node:sqlite'] },
  },
});
