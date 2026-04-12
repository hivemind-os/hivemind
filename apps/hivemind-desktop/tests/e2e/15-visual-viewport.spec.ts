/**
 * E2E tests for visual regression and responsive viewport behavior.
 * Uses Playwright's toHaveScreenshot() for visual comparisons and
 * tests the app at different viewport sizes.
 */
import { test, expect } from '@playwright/test';
import { APP_HARNESS_URL, waitForAppReady, selectFirstSession, navigateToScreen, openSettings, openFlightDeck, collectErrors } from '../helpers';

test.describe('15 — Visual Regression', () => {
  test.beforeEach(async ({ page }) => {
    collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
  });

  test('111. Empty state screenshot', async ({ page }) => {
    await page.waitForTimeout(500);
    await expect(page).toHaveScreenshot('empty-state.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('112. Session loaded with chat messages', async ({ page }) => {
    await selectFirstSession(page);
    await page.waitForTimeout(1000);
    await expect(page).toHaveScreenshot('session-loaded.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('113. Settings modal open', async ({ page }) => {
    await openSettings(page);
    await page.waitForTimeout(500);
    const modal = page.locator('[data-testid="settings-modal"]');
    if (await modal.isVisible({ timeout: 3000 }).catch(() => false)) {
      await expect(page).toHaveScreenshot('settings-open.png', {
        maxDiffPixelRatio: 0.05,
        timeout: 10_000,
      });
    }
  });

  test('114. Workflows page', async ({ page }) => {
    await navigateToScreen(page, 'workflows');
    await page.waitForTimeout(1000);
    await expect(page).toHaveScreenshot('workflows-page.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('115. Bots page', async ({ page }) => {
    await navigateToScreen(page, 'bots');
    await page.waitForTimeout(1000);
    await expect(page).toHaveScreenshot('bots-page.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('116. Scheduler page', async ({ page }) => {
    await navigateToScreen(page, 'scheduler');
    await page.waitForTimeout(1000);
    await expect(page).toHaveScreenshot('scheduler-page.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('117. Collapsed sidebar', async ({ page }) => {
    const collapseBtn = page.locator('[aria-label="Collapse sidebar"]');
    if (await collapseBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await collapseBtn.click();
      await page.waitForTimeout(500);
    }
    await expect(page).toHaveScreenshot('sidebar-collapsed.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });
  });

  test('118. Flight deck open', async ({ page }) => {
    await openFlightDeck(page);
    await page.waitForTimeout(500);
    const fdPanel = page.locator('[data-testid="flight-deck-overlay"], [data-testid="flight-deck-panel"]');
    if (await fdPanel.isVisible({ timeout: 3000 }).catch(() => false)) {
      await expect(page).toHaveScreenshot('flight-deck-open.png', {
        maxDiffPixelRatio: 0.05,
        timeout: 10_000,
      });
    }
  });
});

test.describe('16 — Viewport Variants', () => {
  test('119. Small viewport (800×600) — app renders without overflow', async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
    const page = await context.newPage();
    collectErrors(page);

    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await page.waitForTimeout(500);

    // Sidebar should still be visible or auto-collapsed
    const sidebar = page.locator('[data-sidebar="sidebar"], [data-testid="session-list"]');
    const sidebarVisible = await sidebar.first().isVisible({ timeout: 3000 }).catch(() => false);
    expect(sidebarVisible).toBe(true);

    // No horizontal scrollbar
    const hasHScroll = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
    expect(hasHScroll).toBe(false);

    await expect(page).toHaveScreenshot('viewport-800x600.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });

    await context.close();
  });

  test('120. Large viewport (1920×1080) — app uses full width', async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
    const page = await context.newPage();
    collectErrors(page);

    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await selectFirstSession(page);
    await page.waitForTimeout(500);

    // Main content area should be wider
    const mainWidth = await page.evaluate(() => {
      const main = document.querySelector('.main-area, main, .content-area');
      return main ? main.getBoundingClientRect().width : 0;
    });
    expect(mainWidth).toBeGreaterThan(800);

    await expect(page).toHaveScreenshot('viewport-1920x1080.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });

    await context.close();
  });

  test('121. Small viewport — settings modal fits', async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
    const page = await context.newPage();
    collectErrors(page);

    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await openSettings(page);
    await page.waitForTimeout(500);

    const modal = page.locator('[data-testid="settings-modal"]');
    if (await modal.isVisible({ timeout: 3000 }).catch(() => false)) {
      const box = await modal.boundingBox();
      if (box) {
        // Modal should fit within viewport
        expect(box.x).toBeGreaterThanOrEqual(0);
        expect(box.y).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width).toBeLessThanOrEqual(810); // small tolerance
        expect(box.y + box.height).toBeLessThanOrEqual(610);
      }
    }

    await context.close();
  });

  test('122. Large viewport — workflow designer has room for canvas', async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
    const page = await context.newPage();
    collectErrors(page);

    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await navigateToScreen(page, 'workflows');
    await page.waitForTimeout(500);

    // Click Edit in Designer if available
    const editBtn = page.locator('[title="Edit in Designer"], [data-testid="wf-edit-btn"]').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(1000);
    }

    await expect(page).toHaveScreenshot('viewport-1920-workflows.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });

    await context.close();
  });

  test('123. Medium viewport (1280×720) — balanced layout', async ({ browser }) => {
    const context = await browser.newContext({ viewport: { width: 1280, height: 720 } });
    const page = await context.newPage();
    collectErrors(page);

    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await selectFirstSession(page);
    await page.waitForTimeout(500);

    // Both sidebar and content should be visible
    const sidebar = page.locator('[data-sidebar="sidebar"]').first();
    const sidebarVisible = await sidebar.isVisible({ timeout: 3000 }).catch(() => false);
    expect(sidebarVisible).toBe(true);

    await expect(page).toHaveScreenshot('viewport-1280x720.png', {
      maxDiffPixelRatio: 0.05,
      timeout: 10_000,
    });

    await context.close();
  });

  test('124. Resize viewport dynamically', async ({ page }) => {
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Start at default size
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.waitForTimeout(300);

    // Shrink to small
    await page.setViewportSize({ width: 800, height: 600 });
    await page.waitForTimeout(500);

    // App should still render without errors
    const hasErrors = await page.evaluate(() => {
      const errorOverlay = document.querySelector('.error-boundary, .app-error');
      return !!errorOverlay;
    });
    expect(hasErrors).toBe(false);

    // Expand to large
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.waitForTimeout(500);

    expect(hasErrors).toBe(false);
  });
});
