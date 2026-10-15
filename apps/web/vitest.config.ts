import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// SonarQube resolves lcov `SF:` paths against `sonar.projectBaseDir` (the repo
// root), so the report must carry repo-relative paths (`apps/web/src/...`)
// rather than the default `src/...`. Vitest's lcov reporter `projectRoot`
// controls that prefix; compute the monorepo root from this config file
// (`apps/web/vitest.config.ts` -> `../..`).
const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    exclude: [
      '**/node_modules/**',
      '**/dist/**',
      'e2e/**',
    ],
    coverage: {
      provider: 'v8',
      reporter: [['lcov', { projectRoot: repoRoot }], 'text'],
      reportsDirectory: './coverage',
    },
  },
});
