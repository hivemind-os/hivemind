import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e-integration',
  outputDir: './integration-test-results/artifacts',
  timeout: 300_000, // 5 minutes per test (real backend is slower)
  retries: 1,
  workers: 1, // serial — all tests share a single daemon
  reporter: [
    ['list'],
    ['json', { outputFile: 'integration-test-results/results.json' }],
    ['html', { open: 'never', outputFolder: 'integration-test-results/html-report' }],
  ],
  use: {
    baseURL: 'http://localhost:3002',
    headless: true,
    viewport: { width: 1280, height: 800 },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'npx vite --config tests/vite.integration.config.ts --port 3002',
    port: 3002,
    timeout: 30_000,
    reuseExistingServer: true,
  },
  globalSetup: './tests/integration/global-setup.ts',
  globalTeardown: './tests/integration/global-teardown.ts',
});
