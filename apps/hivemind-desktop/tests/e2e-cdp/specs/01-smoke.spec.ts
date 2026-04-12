/**
 * CDP Smoke Test — proves the full E2E chain works.
 *
 * This test launches the REAL HiveMind OS Tauri binary, connects via CDP,
 * and exercises actual Tauri IPC (not mocked/bridged).
 *
 * Chain under test:
 *   JS invoke() → Tauri command handler → Rust HTTP client → test_daemon → response → JS
 */
import { test, expect } from '@playwright/test';
import { connectToHiveMind, waitForAppReady, loadConfig, collectConsoleErrors } from '../helpers';

// Skip all tests on non-Windows platforms
test.beforeEach(async () => {
  const config = loadConfig();
  test.skip(config.skip, 'CDP tests only run on Windows (WebView2)');
});

test.describe('CDP Smoke Tests', () => {
  test('app launches and sidebar is visible', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      const errors = collectConsoleErrors(page);

      await waitForAppReady(page);

      // Sidebar should be rendered
      const sidebar = page.locator('[data-sidebar="sidebar"]');
      await expect(sidebar).toBeVisible({ timeout: 15_000 });

      // No fatal page errors (filter out benign resource loading issues)
      const fatalErrors = errors.filter(
        (e) => !e.includes('favicon') && !e.includes('net::ERR') &&
               !e.includes('Failed to load resource'),
      );
      expect(fatalErrors.length, `Unexpected page errors: ${fatalErrors.join('; ')}`).toBe(0);
    } finally {
      await browser.close();
    }
  });

  test('can create a session via real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      await waitForAppReady(page);

      // Click "New Session" — this triggers the new session dialog
      // The button is disabled until daemon is online, so wait for it to be enabled
      const newSessionBtn = page.locator('[data-testid="new-session-btn"]').first();
      await newSessionBtn.waitFor({ state: 'visible', timeout: 15_000 });
      // Wait for button to be enabled (daemon must be online)
      await page.waitForFunction(
        () => {
          const btn = document.querySelector('[data-testid="new-session-btn"]');
          return btn && !btn.hasAttribute('disabled');
        },
        { timeout: 15_000 },
      );
      await newSessionBtn.click();

      // The dialog should open showing modality options
      const dialog = page.locator('[role="dialog"]').first();
      await expect(dialog).toBeVisible({ timeout: 10_000 });

      // Pick "Classic Chat" modality to exercise real IPC (chat_create_session)
      const classicBtn = page.locator('[data-testid="modality-classic"]').first();
      if (await classicBtn.isVisible({ timeout: 5_000 }).catch(() => false)) {
        await classicBtn.click();
        await page.waitForTimeout(1_000);

        // Pick default workspace
        const defaultWorkspace = page.locator('[data-testid="workspace-default"]').first();
        if (await defaultWorkspace.isVisible({ timeout: 5_000 }).catch(() => false)) {
          await defaultWorkspace.click();
          await page.waitForTimeout(2_000);
        }
      }

      // A session should now appear in the sidebar
      const sessionItem = page.locator('[data-testid^="session-item-"]').first();
      await expect(sessionItem).toBeVisible({ timeout: 15_000 });
    } finally {
      await browser.close();
    }
  });

  test('can navigate between screens via real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      await waitForAppReady(page);

      // Navigate to Workflows — triggers invoke('workflow_list_definitions')
      const workflowsBtn = page.locator('button:has-text("Workflows")').first();
      if (await workflowsBtn.isVisible({ timeout: 5_000 }).catch(() => false)) {
        await workflowsBtn.click();
        await page.waitForTimeout(1_000);
      }

      // Navigate to Settings — triggers invoke('config_get')
      const settingsBtn = page.locator(
        '[data-testid="sidebar-settings-btn"], [aria-label="Settings"]'
      ).first();
      if (await settingsBtn.isVisible({ timeout: 5_000 }).catch(() => false)) {
        await settingsBtn.click();
        await page.waitForTimeout(1_000);
      }

      // If we got here without crashes, the real IPC is working
      // Verify no IPC-related console errors
      const errors = collectConsoleErrors(page);
      // Give a brief moment for any deferred errors
      await page.waitForTimeout(500);
      const ipcErrors = errors.filter(
        (e) => e.includes('invoke') || e.includes('IPC') || e.includes('deserialize'),
      );
      expect(ipcErrors.length, `IPC errors: ${ipcErrors.join('; ')}`).toBe(0);
    } finally {
      await browser.close();
    }
  });
});
