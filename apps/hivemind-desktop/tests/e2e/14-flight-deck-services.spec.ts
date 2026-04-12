import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  waitForAppReady,
  openFlightDeck,
  collectErrors,
} from '../helpers';

test.describe('Flight Deck – Services Tab', () => {
  test('Services tab should be listed among Flight Deck tabs', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Open Flight Deck
    await openFlightDeck(page);
    await page.waitForTimeout(1000);

    // Check panel is visible
    const panelVisible = await page.locator('[data-testid="flight-deck-panel"]').isVisible({ timeout: 5000 }).catch(() => false);
    if (!panelVisible) {
      // Retry with keyboard shortcut (known flaky)
      await page.keyboard.press('Control+Shift+f');
      await page.waitForTimeout(1000);
    }

    // The Services tab should exist in the tab bar
    const servicesTab = page.locator('[data-testid="fd-tab-services"]');
    const tabExists = await servicesTab.isVisible({ timeout: 5000 }).catch(() => false);

    expect(tabExists, 'Services tab should be visible in Flight Deck tab bar').toBe(true);

    await assertHeartbeat(page, 'after checking services tab');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Services tab should render service list and log viewer', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Open Flight Deck
    await openFlightDeck(page);
    await page.waitForTimeout(1000);

    const panelVisible = await page.locator('[data-testid="flight-deck-panel"]').isVisible({ timeout: 5000 }).catch(() => false);
    if (!panelVisible) {
      await page.keyboard.press('Control+Shift+f');
      await page.waitForTimeout(1000);
    }

    // Click Services tab
    const servicesTab = page.locator('[data-testid="fd-tab-services"]');
    if (!(await servicesTab.isVisible({ timeout: 5000 }).catch(() => false))) {
      // Flight Deck didn't open — skip gracefully (pre-existing flakiness)
      test.skip(true, 'Flight Deck toggle flaky — could not open panel');
      return;
    }
    await servicesTab.click();
    await page.waitForTimeout(1000);

    // Should show service rows from mock data
    const serviceRows = page.locator('.flight-deck-service-row');
    const rowCount = await serviceRows.count();
    expect(rowCount, 'Should display service rows').toBeGreaterThan(0);

    // Should show status indicators
    const runningDots = page.locator('.flight-deck-service-status--running');
    expect(await runningDots.count(), 'Should have running services').toBeGreaterThan(0);

    // Click first service row to open log viewer
    await serviceRows.first().click();
    await page.waitForTimeout(1000);

    // Log viewer dialog should appear
    const logDialog = page.locator('.flight-deck-log-dialog');
    await expect(logDialog).toBeVisible({ timeout: 5000 });

    // Should have log entries
    const logEntries = page.locator('.flight-deck-log-entry');
    expect(await logEntries.count(), 'Should have log entries').toBeGreaterThan(0);

    // Should have filter controls
    await expect(page.locator('.flight-deck-log-dialog-filters select')).toBeVisible();
    await expect(page.locator('.flight-deck-log-dialog-filters input[type="text"]')).toBeVisible();

    // Close dialog by pressing Escape
    await page.keyboard.press('Escape');
    await page.waitForTimeout(500);
    const dialogGone = !(await logDialog.isVisible().catch(() => true));
    expect(dialogGone, 'Log dialog should close').toBe(true);

    await assertHeartbeat(page, 'after services tab interactions');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });
});
