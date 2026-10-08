import { fileURLToPath } from 'node:url';
import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

// The deployment target (custom domain vs. a project GitHub Pages path) is an operator choice, so
// nothing here hard-codes a hostname or base path:
//   VITE_SITE_BASE    asset base path (default '/', correct for a custom domain served at root)
//   VITE_SITE_DOMAIN  when set, a CNAME file is emitted so Pages keeps the custom domain
//   VITE_REPO_URL     repository URL for source/release links (see src/site/config.ts)
// See vite.admin.config.ts for the `/src` alias rationale.
const base = process.env.VITE_SITE_BASE ?? '/';

/**
 * Emit a `CNAME` file into the built site when a custom domain is configured.
 *
 * GitHub Pages needs this file in the published artifact to keep serving a custom domain. It is
 * generated from configuration rather than committed, so the domain stays an operator setting
 * instead of a hard-coded source value.
 */
function cnamePlugin(): Plugin {
  return {
    name: 'iem-cast-cname',
    apply: 'build',
    generateBundle() {
      const domain = process.env.VITE_SITE_DOMAIN;
      if (domain && domain.trim().length > 0) {
        this.emitFile({ type: 'asset', fileName: 'CNAME', source: `${domain.trim()}\n` });
      }
    },
  };
}

export default defineConfig({
  root: 'site',
  base,
  plugins: [react(), tailwindcss(), cnamePlugin()],
  resolve: {
    alias: {
      '/src': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    outDir: '../dist/site',
    emptyOutDir: true,
  },
});
