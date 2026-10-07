import { defineConfig, devices } from '@playwright/test';

// Automated browser gate for the reusable macOS POC (plan Task 9).
// Uses mocks only; it is NOT fulfillment of the real wireless requirement.
export default defineConfig({
  testDir: './apps/web/e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4173',
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Pixel 7'] },
    },
    {
      name: 'webkit',
      use: { ...devices['iPhone 14'] },
    },
  ],
  webServer: {
    command: 'npm run preview:site --workspace apps/web',
    url: 'http://127.0.0.1:4173/iem-cast/',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
