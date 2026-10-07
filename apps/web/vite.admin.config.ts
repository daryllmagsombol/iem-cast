import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// The HTML lives in `admin/`, but the shared source lives in `src/` one level up. Because the
// dev server serves `admin/` at `/`, a relative `../src/...` script URL would be collapsed by the
// browser to `/src/...` and resolved against the wrong root. The `/src` alias pins it to the real
// directory for both dev and build.
export default defineConfig({
  root: 'admin',
  base: '/',
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '/src': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    outDir: '../dist/admin',
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
  },
});
