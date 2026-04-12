/**
 * Playwright config for true E2E tests against the real Tauri binary via CDP.
 *
 * These tests launch the actual HiveMind OS desktop application and connect to its
 * WebView2 via Chrome DevTools Protocol. This exercises the full chain:
 *   JS invoke() → Tauri IPC → Rust command handlers → daemon HTTP → response
 *
 * Platform support: Windows only (WebView2 is Chromium-based, exposing CDP).
 * macOS/Linux are skipped — WKWebView does not support CDP.
 */
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e-cdp/specs',
  outputDir: './cdp-test-results/artifacts',
  timeout: 120_000, // 2 min — real app startup can be slow in CI
  retries: 1,
  workers: 1, // serial — all tests share a single app instance
  reporter: [
    ['list'],
    ['json', { outputFile: 'cdp-test-results/results.json' }],
    ['html', { open: 'never', outputFolder: 'cdp-test-results/html-report' }],
  ],
  use: {
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    video: 'retain-on-failure',
  },
  // No webServer — we launch the Tauri binary and connect via CDP in globalSetup
  globalSetup: './tests/e2e-cdp/global-setup.ts',
  globalTeardown: './tests/e2e-cdp/global-teardown.ts',
});
