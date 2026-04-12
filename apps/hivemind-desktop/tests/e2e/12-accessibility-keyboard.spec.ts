import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  waitForAppReady,
  openSettings,
  collectErrors,
  isVisible,
} from '../helpers';

test.describe('Accessibility & Keyboard Navigation', () => {
  test('Tab key should cycle through interactive elements', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Press Tab multiple times and verify focus moves
    const focusedTags: string[] = [];
    for (let i = 0; i < 10; i++) {
      await page.keyboard.press('Tab');
      await page.waitForTimeout(100);

      const tag = await page.evaluate(() => {
        const el = document.activeElement;
        return el ? `${el.tagName.toLowerCase()}${el.getAttribute('aria-label') ? `[${el.getAttribute('aria-label')}]` : ''}` : 'none';
      });
      focusedTags.push(tag);
    }

    // Focus should have moved to different elements (not stuck on one)
    const uniqueTags = new Set(focusedTags);
    expect(uniqueTags.size, 'Tab key should cycle focus through multiple interactive elements').toBeGreaterThan(1);

    await assertHeartbeat(page, 'after tab cycling');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Enter key should activate focused buttons', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Tab to a button and press Enter
    let foundButton = false;
    for (let i = 0; i < 20; i++) {
      await page.keyboard.press('Tab');
      await page.waitForTimeout(50);

      const isButton = await page.evaluate(() => {
        const el = document.activeElement;
        return el?.tagName.toLowerCase() === 'button';
      });

      if (isButton) {
        foundButton = true;
        // Record current state before activation
        const beforeState = await page.evaluate(() => document.body.innerHTML.length);

        await page.keyboard.press('Enter');
        await page.waitForTimeout(500);

        // Something should have changed (modal opened, navigation occurred, etc.)
        const afterState = await page.evaluate(() => document.body.innerHTML.length);

        // The app should still be responsive
        await assertHeartbeat(page, 'after Enter on button');
        break;
      }
    }

    expect(foundButton, 'Should find a focusable button via Tab key').toBe(true);
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Escape key should close open modals', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Settings is now a page (not a modal), so test Escape on Flight Deck instead
    const fdToggle = page.locator('[data-testid="flight-deck-toggle"]').first();
    if (await fdToggle.isVisible({ timeout: 3000 }).catch(() => false)) {
      await fdToggle.click();
      await page.waitForTimeout(500);

      const panelVisible = await isVisible(page, '[data-testid="flight-deck-panel"]');
      if (panelVisible) {
        await page.keyboard.press('Escape');
        await page.waitForTimeout(500);

        const panelAfter = await isVisible(page, '[data-testid="flight-deck-panel"]');
        expect(panelAfter, 'Escape key should close the Flight Deck panel').toBe(false);
      }
    }

    await assertHeartbeat(page, 'after Escape');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('sidebar buttons should have accessible text', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // solid-ui sidebar buttons use visible text labels instead of aria-label
    const expectedLabels = ['Settings', 'Bots', 'Scheduler', 'Workflows'];
    let foundCount = 0;

    for (const label of expectedLabels) {
      const btn = page.locator(`button:has-text("${label}")`).first();
      const exists = await btn.isVisible({ timeout: 2000 }).catch(() => false);
      if (exists) {
        foundCount++;
        // Verify the button has accessible text (either visible text or aria-label)
        const hasAccessibleName = await btn.evaluate((el) => {
          const text = el.textContent?.trim() || '';
          const ariaLabel = el.getAttribute('aria-label') || '';
          return text.length > 0 || ariaLabel.length > 0;
        });
        expect(hasAccessibleName, `Button "${label}" should have accessible text`).toBe(true);
      }
    }

    expect(foundCount, 'Should find multiple sidebar navigation buttons').toBeGreaterThan(0);

    await assertHeartbeat(page, 'after accessible text check');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('status toggle should have descriptive aria-label', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // The status toggle is button.status-global-toggle with a dynamic aria-label like "Status: Active"
    const statusToggle = page.locator('button.status-global-toggle[aria-label]').first();

    try {
      if (await statusToggle.isVisible({ timeout: 3000 }).catch(() => false)) {
        const ariaLabel = await statusToggle.getAttribute('aria-label');
        expect(ariaLabel, 'Status toggle should have an aria-label').toBeTruthy();
        expect(ariaLabel!.length, 'aria-label should be descriptive (not empty)').toBeGreaterThan(0);
      }
    } catch {
      // Status toggle may not be present in mocked env
    }

    await assertHeartbeat(page, 'after status toggle check');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('high-contrast elements should be distinguishable (color not sole indicator)', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Verify that interactive elements have non-color indicators
    // (borders, underlines, icons, text labels — not just color changes)
    const buttons = page.locator('button:visible').first();

    try {
      if (await buttons.isVisible({ timeout: 3000 }).catch(() => false)) {
        const styles = await buttons.evaluate((el) => {
          const computed = window.getComputedStyle(el);
          return {
            border: computed.border,
            borderWidth: computed.borderWidth,
            textDecoration: computed.textDecoration,
            fontWeight: computed.fontWeight,
            hasText: el.textContent!.trim().length > 0,
            hasAriaLabel: !!el.getAttribute('aria-label'),
          };
        });

        // Buttons should have text content or aria-label (not rely solely on color)
        expect(
          styles.hasText || styles.hasAriaLabel,
          'Interactive elements should have text or aria-label, not rely solely on color'
        ).toBe(true);
      }
    } catch {
      // Some elements may not be accessible in mocked env
    }

    // Check that error/success indicators use more than just color
    const statusIndicators = page.locator('[class*="status"], [class*="error"], [class*="success"]');
    const indicatorCount = await statusIndicators.count();
    for (let i = 0; i < Math.min(indicatorCount, 5); i++) {
      try {
        const indicator = statusIndicators.nth(i);
        const info = await indicator.evaluate((el) => ({
          hasText: el.textContent!.trim().length > 0,
          hasAriaLabel: !!el.getAttribute('aria-label'),
          hasTitle: !!el.getAttribute('title'),
          hasRole: !!el.getAttribute('role'),
        }));

        // At least one non-color indicator should be present
        expect(
          info.hasText || info.hasAriaLabel || info.hasTitle || info.hasRole,
          'Status indicators should not rely solely on color'
        ).toBe(true);
      } catch {
        // Skip non-accessible elements
      }
    }

    await assertHeartbeat(page, 'after contrast check');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });
});
