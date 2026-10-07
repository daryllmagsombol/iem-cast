import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  root: 'site',
  base: '/iem-cast/',
  plugins: [react(), tailwindcss()],
  build: {
    outDir: '../dist/site',
    emptyOutDir: true,
  },
});
