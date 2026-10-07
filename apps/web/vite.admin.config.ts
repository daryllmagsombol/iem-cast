import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  root: 'admin',
  base: '/',
  plugins: [react(), tailwindcss()],
  build: {
    outDir: '../dist/admin',
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1',
    port: 5173,
  },
});
