import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// See vite.admin.config.ts: the `/src` alias keeps the shared source reachable from the
// page-root HTML during development as well as during build.
export default defineConfig({
  root: 'musician',
  base: '/',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '/src': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    outDir: '../dist/musician',
    emptyOutDir: true,
  },
});
