import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Workspace packages ship TypeScript source; Vite compiles them directly.
export default defineConfig({
  plugins: [react()],
});
