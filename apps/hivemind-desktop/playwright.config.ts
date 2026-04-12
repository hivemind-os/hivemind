import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 180_000, // 3 minutes per test
  retries: 1,
  workers: 2, // limit parallelism to avoid Vite server contention
  reporter: [
    ['list'],
    ['json', { outputFile: 'test-results/results.json' }],
    ['html', { open: 'never', outputFolder: 'test-results/html-report' }],
  ],
  use: {
    baseURL: 'http://localhost:3001',
    headless: true,
    viewport: { width: 1280, height: 800 },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'npx vite --config tests/vite.test.config.ts --port 3001',
    port: 3001,
    timeout: 30_000,
    reuseExistingServer: true,
  },
});
