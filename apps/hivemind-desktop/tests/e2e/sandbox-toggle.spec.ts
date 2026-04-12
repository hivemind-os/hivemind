import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  waitForAppReady,
  openSettings,
  collectErrors,
} from '../helpers';

/**
 * Regression test: toggling a switch on the Security settings tab
 * must NOT cause the settings dialog to scroll off-screen.
 */
test.describe('Settings – Sandbox Toggle Scroll Bug', () => {
  test('toggling sandbox switch should not break the settings dialog layout', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Open settings
    await openSettings(page);
    const modal = page.locator('[data-testid="settings-modal"]');
    await modal.waitFor({ state: 'visible', timeout: 10_000 });

    // Navigate to Security tab
    const securityCategory = page.locator('[data-testid="settings-category-security"]');
    await securityCategory.click();
    await page.waitForTimeout(200);
    const securityTab = page.locator('[data-testid="settings-tab-security"]');
    await expect(securityTab).toBeVisible({ timeout: 2_000 });
    await securityTab.click();
    await page.waitForTimeout(300);

    // Verify the settings header is visible BEFORE toggle
    const header = modal.locator('.settings-header');
    await expect(header).toBeVisible();
    const headerBoxBefore = await header.boundingBox();
    expect(headerBoxBefore, 'header should have a bounding box before toggle').not.toBeNull();
    expect(headerBoxBefore!.y).toBeGreaterThanOrEqual(0);

    // Get the sidebar bounding box before toggle
    const sidebar = modal.locator('.settings-tabs');
    await expect(sidebar).toBeVisible();
    const sidebarBoxBefore = await sidebar.boundingBox();
    expect(sidebarBoxBefore, 'sidebar should have a bounding box before toggle').not.toBeNull();

    // Find the sandbox switch and click its label to toggle
    const sandboxSwitch = page.locator('label:has-text("Sandbox")').first();
    await expect(sandboxSwitch).toBeVisible({ timeout: 5_000 });
    await sandboxSwitch.click();
    await page.waitForTimeout(500);

    // Verify the header is STILL visible after toggle
    await expect(header).toBeVisible({ timeout: 2_000 });
    const headerBoxAfter = await header.boundingBox();
    expect(headerBoxAfter, 'header should still have a bounding box after toggle').not.toBeNull();
    // Header should still be near the top — not scrolled off-screen
    expect(headerBoxAfter!.y).toBeLessThan(200);

    // Verify the sidebar is STILL visible after toggle
    await expect(sidebar).toBeVisible({ timeout: 2_000 });
    const sidebarBoxAfter = await sidebar.boundingBox();
    expect(sidebarBoxAfter, 'sidebar should still have a bounding box after toggle').not.toBeNull();
    // Sidebar should not have moved significantly
    expect(Math.abs(sidebarBoxAfter!.y - sidebarBoxBefore!.y)).toBeLessThan(50);

    // Check that no ancestor scrolled unexpectedly
    const scrollState = await page.evaluate(() => {
      const modal = document.querySelector('[data-testid="settings-modal"]');
      if (!modal) return { error: 'modal not found' };
      const results: { tag: string; scrollTop: number; className: string }[] = [];
      let el: HTMLElement | null = modal as HTMLElement;
      while (el) {
        if (el.scrollTop !== 0) {
          results.push({
            tag: el.tagName,
            scrollTop: el.scrollTop,
            className: el.className?.substring(0, 50) || '',
          });
        }
        el = el.parentElement;
      }
      return { scrolled: results };
    });
    expect((scrollState as any).scrolled ?? []).toEqual([]);

    // Toggle it back and verify again
    await sandboxSwitch.click();
    await page.waitForTimeout(500);
    await expect(header).toBeVisible({ timeout: 2_000 });
    const headerBoxFinal = await header.boundingBox();
    expect(headerBoxFinal, 'header should be visible after second toggle').not.toBeNull();
    expect(headerBoxFinal!.y).toBeLessThan(200);

    // Filter out unrelated 404 errors from mock daemon API calls
    const relevantErrors = errors.filter(e => !e.includes('404') && !e.includes('Failed to load resource'));
    expect(relevantErrors).toEqual([]);
  });
});
