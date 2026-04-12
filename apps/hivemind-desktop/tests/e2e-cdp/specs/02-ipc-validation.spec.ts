/**
 * CDP IPC Validation Tests — specifically targets the Tauri IPC boundary.
 *
 * These tests verify that the real invoke() → Rust command handler path
 * works correctly, catching issues that mocked tests cannot detect:
 *   - Parameter serialization (snake_case keys)
 *   - Response deserialization (camelCase JSON)
 *   - Error handling across the IPC boundary
 *   - Native plugin commands
 */
import { test, expect } from '@playwright/test';
import {
  connectToHiveMind,
  waitForAppReady,
  loadConfig,
  collectConsoleErrors,
  queryDaemonApi,
  postDaemonApi,
} from '../helpers';

// Skip all tests on non-Windows platforms
test.beforeEach(async () => {
  const config = loadConfig();
  test.skip(config.skip, 'CDP tests only run on Windows (WebView2)');
});

test.describe('IPC Contract Validation', () => {
  test('session create/list round-trip through real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      await waitForAppReady(page);

      // Wait for daemon to be online (new-session button enabled)
      await page.waitForFunction(
        () => {
          const btn = document.querySelector('[data-testid="new-session-btn"]');
          return btn && !btn.hasAttribute('disabled');
        },
        { timeout: 15_000 },
      );

      // Count initial sessions visible in sidebar
      const initialSessions = await page.locator('[data-testid^="session-item-"]').count();

      // Create a session via the real Tauri IPC (invoke chat_create_session)
      const result = await page.evaluate(async () => {
        const internals = (window as any).__TAURI_INTERNALS__;
        if (!internals?.invoke) return { error: 'No Tauri internals found' };
        try {
          const session = await internals.invoke('chat_create_session', {
            modality: 'linear',
          });
          return { ok: true, sessionId: session?.id || session?.sessionId };
        } catch (e: any) {
          return { error: e?.message || String(e) };
        }
      });
      expect(result).toHaveProperty('ok', true);

      // Force the app to refresh its session list via IPC
      await page.evaluate(async () => {
        const internals = (window as any).__TAURI_INTERNALS__;
        if (internals?.invoke) {
          // Trigger a session list refresh by invoking the list command
          await internals.invoke('chat_list_sessions');
        }
      });

      // Wait a moment for React/Solid to re-render, then check sidebar
      await page.waitForTimeout(3_000);

      // Verify the session appears in the sidebar by checking list count
      // The app may auto-refresh, or we may need to check the API directly
      const config = loadConfig();
      const sessionsResp = await fetch(`${config.daemonBaseUrl}/api/v1/chat/sessions`, {
        headers: { Authorization: `Bearer ${config.daemonAuthToken}` },
      });
      expect(sessionsResp.ok).toBeTruthy();
      const sessions = await sessionsResp.json();
      expect(sessions.length).toBeGreaterThan(initialSessions);
    } finally {
      await browser.close();
    }
  });

  test('config_get returns valid config through real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      const errors = collectConsoleErrors(page);
      await waitForAppReady(page);

      // Open settings — triggers config_get through real Tauri IPC
      const settingsBtn = page.locator(
        '[data-testid="sidebar-settings-btn"], [aria-label="Settings"]'
      ).first();
      await settingsBtn.waitFor({ state: 'visible', timeout: 15_000 });
      await settingsBtn.click();

      // Wait for settings content to load
      await page.waitForTimeout(2_000);

      // Settings should render without IPC errors
      // Check for any config deserialization errors
      const configErrors = errors.filter(
        (e) =>
          e.includes('config_get') ||
          e.includes('deserialize') ||
          e.includes('snake_case') ||
          e.includes('camelCase'),
      );
      expect(
        configErrors.length,
        `Config IPC errors: ${configErrors.join('; ')}`,
      ).toBe(0);

      // Verify some settings UI rendered (proves config data made it through IPC)
      const settingsContent = page.locator(
        '[class*="settings"], [role="dialog"], [class*="modal"]'
      ).first();
      await expect(settingsContent).toBeVisible({ timeout: 5_000 });
    } finally {
      await browser.close();
    }
  });

  test('no IPC serialization errors during full app usage', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      const errors = collectConsoleErrors(page);
      await waitForAppReady(page);

      // Exercise multiple IPC commands by navigating the whole app
      const screens = [
        { name: 'Sessions', selector: 'button:has-text("Sessions")' },
        { name: 'Bots', selector: 'button:has-text("Bots")' },
        { name: 'Workflows', selector: 'button:has-text("Workflows")' },
        { name: 'Scheduler', selector: 'button:has-text("Scheduler")' },
      ];

      for (const screen of screens) {
        const btn = page.locator(screen.selector).first();
        if (await btn.isVisible({ timeout: 3_000 }).catch(() => false)) {
          await btn.click();
          // Wait for the IPC call to complete and UI to settle
          await page.waitForTimeout(1_500);
        }
      }

      // Open and close settings
      const settingsBtn = page.locator(
        '[data-testid="sidebar-settings-btn"], [aria-label="Settings"]'
      ).first();
      if (await settingsBtn.isVisible({ timeout: 3_000 }).catch(() => false)) {
        await settingsBtn.click();
        await page.waitForTimeout(1_500);
        // Close settings
        const closeBtn = page.locator(
          '[role="dialog"] button:has-text("Cancel"), [role="dialog"] button:has-text("Close"), button[aria-label="Close"]'
        ).first();
        if (await closeBtn.isVisible({ timeout: 2_000 }).catch(() => false)) {
          await closeBtn.click();
          await page.waitForTimeout(500);
        }
      }

      // Collect all IPC-related errors
      const ipcErrors = errors.filter((e) => {
        const lower = e.toLowerCase();
        return (
          lower.includes('invoke') ||
          lower.includes('ipc') ||
          lower.includes('deserializ') ||
          lower.includes('serializ') ||
          lower.includes('expected') ||
          lower.includes('type error')
        );
      });

      expect(
        ipcErrors.length,
        `IPC serialization errors detected during app navigation:\n${ipcErrors.join('\n')}`,
      ).toBe(0);
    } finally {
      await browser.close();
    }
  });

  test('daemon_status command works through real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      await waitForAppReady(page);

      // Execute invoke('daemon_status') directly in the page context
      // This tests the raw IPC path
      const result = await page.evaluate(async () => {
        const internals = (window as any).__TAURI_INTERNALS__;
        if (!internals?.invoke) return { error: 'No Tauri internals found' };
        try {
          const status = await internals.invoke('daemon_status');
          return { ok: true, status };
        } catch (e: any) {
          return { error: e?.message || String(e) };
        }
      });

      // The daemon should be running (we started test_daemon)
      expect(result).toHaveProperty('ok', true);
      // Status might be null if the daemon uses a different status endpoint,
      // but the invoke itself should not throw
      expect(result).not.toHaveProperty('error');
    } finally {
      await browser.close();
    }
  });

  test('app_context returns valid paths through real IPC', async () => {
    const { browser, page } = await connectToHiveMind();
    try {
      await waitForAppReady(page);

      // Test app_context which returns local filesystem paths
      const result = await page.evaluate(async () => {
        const internals = (window as any).__TAURI_INTERNALS__;
        if (!internals?.invoke) return { error: 'No Tauri internals' };
        try {
          const ctx = await internals.invoke('app_context');
          return { ok: true, ctx };
        } catch (e: any) {
          return { error: e?.message || String(e) };
        }
      });

      expect(result).toHaveProperty('ok', true);
      const ctx = (result as any).ctx;
      // app_context should return real paths on the system
      expect(ctx).toBeDefined();
      // The daemon_url should be set (pointing to our test daemon)
      if (ctx.daemon_url) {
        expect(ctx.daemon_url).toContain('http');
      }
    } finally {
      await browser.close();
    }
  });
});
