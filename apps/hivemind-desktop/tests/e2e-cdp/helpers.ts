/**
 * Shared helpers for CDP-based E2E tests.
 *
 * These tests connect to the real running HiveMind OS app via Chrome DevTools Protocol,
 * so all interactions go through the actual Tauri IPC boundary.
 */
import { chromium, type Browser, type BrowserContext, type Page } from 'playwright';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

const CONFIG_PATH = path.join(os.tmpdir(), 'hivemind-cdp-test-config.json');

export interface CdpTestConfig {
  skip: boolean;
  cdpUrl: string;
  daemonBaseUrl: string;
  daemonAuthToken: string;
  appPid: number;
  daemonPid: number;
}

/** Load test config written by global-setup. */
export function loadConfig(): CdpTestConfig {
  const raw = fs.readFileSync(CONFIG_PATH, 'utf8');
  return JSON.parse(raw);
}

/** Connect to the HiveMind OS app via CDP and return the main app page. */
export async function connectToHiveMind(): Promise<{ browser: Browser; page: Page }> {
  const config = loadConfig();
  const browser = await chromium.connectOverCDP(config.cdpUrl);

  // Tauri creates one browser context with one page (the main window)
  const contexts = browser.contexts();
  if (contexts.length === 0) {
    throw new Error('No browser contexts found — is the HiveMind OS app running?');
  }
  const context = contexts[0];
  const pages = context.pages();
  if (pages.length === 0) {
    throw new Error('No pages found in the HiveMind OS browser context');
  }
  const page = pages[0];
  await page.waitForLoadState('domcontentloaded');

  return { browser, page };
}

/** Wait for the HiveMind OS app to finish initializing (loading overlay gone, UI rendered). */
export async function waitForAppReady(page: Page, timeoutMs = 30_000): Promise<void> {
  // Wait for the initializing overlay to disappear
  await page.waitForFunction(
    () => !document.querySelector('.initializing-overlay'),
    { timeout: timeoutMs },
  );

  // If the setup wizard is showing, complete it by setting setup_completed via IPC
  const hasWizard = await page.locator('[class*="wizard"], [class*="setup"], [data-testid="setup-wizard"]').first()
    .isVisible({ timeout: 3_000 }).catch(() => false);
  if (hasWizard) {
    console.log('[cdp-helpers] Setup wizard detected, attempting to mark setup completed');
    await page.evaluate(async () => {
      const internals = (window as any).__TAURI_INTERNALS__;
      if (internals?.invoke) {
        try {
          const config = await internals.invoke('config_get');
          config.setup_completed = true;
          await internals.invoke('config_update', { config });
        } catch { /* ignore — wizard may not block all tests */ }
      }
    });
    // Reload the page to pick up the config change
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForFunction(
      () => !document.querySelector('.initializing-overlay'),
      { timeout: timeoutMs },
    );
  }

  // Wait for the sidebar to be present (indicates full app render)
  await page.waitForSelector('[data-sidebar="sidebar"]', { timeout: 15_000 });
}

/** Make an API call to the test daemon. */
export async function queryDaemonApi(urlPath: string): Promise<unknown> {
  const config = loadConfig();
  const resp = await fetch(`${config.daemonBaseUrl}${urlPath}`, {
    headers: { Authorization: `Bearer ${config.daemonAuthToken}` },
  });
  if (!resp.ok) throw new Error(`Daemon API ${urlPath}: ${resp.status}`);
  return resp.json();
}

/** POST to the test daemon API. */
export async function postDaemonApi(urlPath: string, body: unknown): Promise<Response> {
  const config = loadConfig();
  return fetch(`${config.daemonBaseUrl}${urlPath}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${config.daemonAuthToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
}

/** Collect console errors from the page for assertion. */
export function collectConsoleErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      errors.push(msg.text());
    }
  });
  page.on('pageerror', (err) => {
    errors.push(`PAGE ERROR: ${err.message}`);
  });
  return errors;
}
