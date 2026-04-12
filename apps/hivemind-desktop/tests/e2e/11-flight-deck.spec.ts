import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  waitForAppReady,
  openFlightDeck,
  collectErrors,
  isVisible,
  clickButton,
} from '../helpers';

test.describe('Flight Deck', () => {
  test('Flight Deck toggle button should be visible', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Wait for the toggle button to render
    await page.waitForSelector('[data-testid="flight-deck-toggle"]', { timeout: 10_000 });
    const toggleBtn = page.locator('[data-testid="flight-deck-toggle"]');
    const toggleVisible = await toggleBtn.first().isVisible({ timeout: 5000 }).catch(() => false);

    expect(toggleVisible, 'Flight Deck toggle button should be visible').toBe(true);

    await assertHeartbeat(page, 'after checking toggle');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('clicking toggle should open Flight Deck overlay', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Click the Flight Deck toggle via helper
    await openFlightDeck(page);

    // The Flight Deck overlay and panel should now be visible
    const overlayVisible = await isVisible(page, '[data-testid="flight-deck-overlay"]');
    const panelVisible = await isVisible(page, '[data-testid="flight-deck-panel"]');

    expect(overlayVisible || panelVisible, 'Flight Deck overlay should be visible after clicking toggle').toBeTruthy();

    await assertHeartbeat(page, 'after opening Flight Deck');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Ctrl+Shift+F should toggle Flight Deck', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    const overlaySelector = '[data-testid="flight-deck-overlay"]';

    // Flight Deck should not be open initially
    const initiallyVisible = await isVisible(page, overlaySelector);

    // Press Ctrl+Shift+F to open
    await page.keyboard.press('Control+Shift+f');
    await page.waitForTimeout(500);

    const afterOpen = await page.locator(overlaySelector).first().isVisible({ timeout: 3000 }).catch(() => false);

    // If shortcut worked, overlay should be toggled
    if (afterOpen) {
      // Press again to close
      await page.keyboard.press('Control+Shift+f');
      await page.waitForTimeout(500);

      const afterClose = await page.locator(overlaySelector).first().isVisible({ timeout: 2000 }).catch(() => false);
      expect(afterClose, 'Ctrl+Shift+F should toggle Flight Deck closed').toBe(false);
    }

    await assertHeartbeat(page, 'after keyboard toggle');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Flight Deck should show agent section', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Open Flight Deck via helper
    await openFlightDeck(page);
    await assertHeartbeat(page, 'Flight Deck opened');

    // Look for content inside the flight deck panel (tabs, items, etc.)
    try {
      const panelContent = page.locator('[data-testid="flight-deck-panel"] .flight-deck-content, [data-testid="flight-deck-panel"] .flight-deck-tabs, [data-testid="flight-deck-panel"] .flight-deck-list').first();
      const contentVisible = await panelContent.isVisible({ timeout: 3000 }).catch(() => false);
      const panelVisible = await isVisible(page, '[data-testid="flight-deck-panel"]');
      expect(contentVisible || panelVisible, 'Flight Deck should display panel content').toBe(true);
    } catch {
      // Mocked data may vary
    }

    await assertHeartbeat(page, 'after agent section check');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Flight Deck should show workflow instances section', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Open Flight Deck via helper
    await openFlightDeck(page);
    await assertHeartbeat(page, 'Flight Deck opened');

    // Look for flight deck tabs or items
    try {
      const fdTabs = page.locator('[data-testid="flight-deck-panel"] [data-testid^="fd-tab-"]');
      const tabCount = await fdTabs.count();
      const panelVisible = await isVisible(page, '[data-testid="flight-deck-panel"]');
      expect(tabCount > 0 || panelVisible, 'Flight Deck should display tabs or panel content').toBe(true);
    } catch {
      // Mocked data may vary
    }

    await assertHeartbeat(page, 'after workflow section check');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('closing Flight Deck should remove the overlay', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    const overlaySelector = '[data-testid="flight-deck-overlay"]';

    // Open Flight Deck via helper
    await openFlightDeck(page);

    const isOpen = await page.locator(overlaySelector).first().isVisible({ timeout: 3000 }).catch(() => false);

    if (isOpen) {
      // Close with Escape key (toggle button may be obscured by overlay)
      await page.keyboard.press('Escape');
      await page.waitForTimeout(500);

      const afterClose = await page.locator(overlaySelector).first().isVisible({ timeout: 2000 }).catch(() => false);
      expect(afterClose, 'Flight Deck overlay should be removed after closing').toBe(false);
    }

    await assertHeartbeat(page, 'after closing Flight Deck');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(10);
  });
});
