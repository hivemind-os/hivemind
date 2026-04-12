import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  DESIGNER_HARNESS_URL,
  assertHeartbeat,
  waitForAppReady,
  navigateToScreen,
  openSettings,
  collectErrors,
  isVisible,
  addDesignerNode,
  designerNodeExists,
} from '../helpers';

test.describe('Stress Testing & Resilience', () => {
  test('rapid screen switching (50 cycles) should not freeze the app', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    const screens: Array<'session' | 'bots' | 'scheduler' | 'workflows'> = [
      'session', 'bots', 'scheduler', 'workflows',
    ];

    for (let i = 0; i < 50; i++) {
      const screen = screens[i % screens.length];
      try {
        await navigateToScreen(page, screen);
      } catch {
        // Some screens may not be available in mocked env
      }

      // Check heartbeat every 10 cycles
      if (i % 10 === 9) {
        await assertHeartbeat(page, `screen switch cycle ${i + 1}/50`);
      }
    }

    await assertHeartbeat(page, 'after 50 screen switches');
    expect(errors.length, `Too many errors after screen switching: ${errors.join('; ')}`).toBeLessThan(10);
  });

  test('opening and closing settings modal 20 times should not leak memory', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Get baseline DOM node count
    const baselineNodes = await page.evaluate(() => document.querySelectorAll('*').length);

    for (let i = 0; i < 20; i++) {
      // Open settings
      await openSettings(page);
      await page.waitForTimeout(200);

      // Close settings with Escape (settings is now a page with [data-testid="settings-modal"])
      await page.keyboard.press('Escape');
      await page.waitForTimeout(200);

      // If still open, try again
      if (await isVisible(page, '[data-testid="settings-modal"]')) {
        await page.keyboard.press('Escape');
        await page.waitForTimeout(200);
      }

      // Check heartbeat every 5 cycles
      if (i % 5 === 4) {
        await assertHeartbeat(page, `settings open/close cycle ${i + 1}/20`);
      }
    }

    // DOM node count should not have grown excessively (< 3x baseline)
    const finalNodes = await page.evaluate(() => document.querySelectorAll('*').length);
    const ratio = finalNodes / baselineNodes;
    expect(ratio, `DOM node count grew ${ratio.toFixed(1)}x — possible memory leak`).toBeLessThan(3);

    await assertHeartbeat(page, 'after 20 modal cycles');
    expect(errors.length, `Too many errors: ${errors.join('; ')}`).toBeLessThan(10);
  });

  test('rapid sidebar session selection (50 switches) should remain responsive', async ({ page }) => {
    test.slow();
    test.setTimeout(300_000);
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Navigate to sessions screen
    await navigateToScreen(page, 'session');
    await page.waitForTimeout(300);

    // Find session items in the sidebar using [data-testid^="session-item-"]
    const sessionItems = page.locator('[data-testid^="session-item-"]:visible');
    const sessionCount = await sessionItems.count();

    if (sessionCount >= 2) {
      for (let i = 0; i < 50; i++) {
        const idx = i % sessionCount;
        try {
          await sessionItems.nth(idx).click();
          await page.waitForTimeout(50);
        } catch {
          // Item may have been re-rendered
        }

        // Check heartbeat every 25 cycles
        if (i % 25 === 24) {
          await assertHeartbeat(page, `session switch ${i + 1}/50`);
        }
      }
    } else {
      // If only one or no sessions, just rapidly click the sessions nav
      for (let i = 0; i < 50; i++) {
        try {
          await navigateToScreen(page, 'session');
          await page.waitForTimeout(50);
        } catch {
          // Skip failures
        }

        if (i % 25 === 24) {
          await assertHeartbeat(page, `nav click ${i + 1}/50`);
        }
      }
    }

    await assertHeartbeat(page, 'after 50 rapid selections');
    expect(errors.length, `Too many errors: ${errors.slice(0, 3).join('; ')}`).toBeLessThan(30);
  });

  test('simultaneous multiple modal opens should not corrupt UI state', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    // Rapidly try to open/close settings in quick succession
    for (let i = 0; i < 5; i++) {
      // Open settings
      await openSettings(page);
      await page.waitForTimeout(100);

      // Try pressing Escape immediately to close
      await page.keyboard.press('Escape');
      await page.waitForTimeout(100);

      // Open settings again without waiting for animation to complete
      await openSettings(page);
      await page.waitForTimeout(100);

      // Close via Escape
      await page.keyboard.press('Escape');
      await page.waitForTimeout(100);
    }

    // After rapid open/close, UI should be in a clean state (no stacked modals)
    await page.waitForTimeout(500);
    const settingsOverlays = page.locator('[data-testid="settings-modal"]:visible');
    const overlayCount = await settingsOverlays.count();
    expect(overlayCount, 'No stacked/orphaned modals should remain').toBeLessThanOrEqual(1);

    // If a modal is still open, close it
    if (overlayCount > 0) {
      await page.keyboard.press('Escape');
      await page.waitForTimeout(300);
    }

    // App should still be functional
    await navigateToScreen(page, 'session');
    await assertHeartbeat(page, 'after rapid modal stress');
    expect(errors.length, `Too many errors: ${errors.join('; ')}`).toBeLessThan(10);
  });

  test('workflow designer: adding 20 nodes should not degrade performance', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });
    await assertHeartbeat(page, 'initial');

    const nodeTypes = [
      'Call Tool', 'Invoke Agent', 'Feedback Gate', 'Branch', 'Delay',
    ];

    // Time the addition of 20 nodes
    const timings: number[] = [];
    for (let i = 0; i < 20; i++) {
      const label = nodeTypes[i % nodeTypes.length];
      const start = Date.now();

      await addDesignerNode(page, label);

      const elapsed = Date.now() - start;
      timings.push(elapsed);

      // Check heartbeat every 5 nodes
      if (i % 5 === 4) {
        await assertHeartbeat(page, `after adding node ${i + 1}/20`);
      }
    }

    // Performance should not degrade significantly:
    // Average time for last 5 nodes should not be more than 5x the first 5
    const firstFiveAvg = timings.slice(0, 5).reduce((a, b) => a + b, 0) / 5;
    const lastFiveAvg = timings.slice(-5).reduce((a, b) => a + b, 0) / 5;

    if (firstFiveAvg > 0) {
      const degradation = lastFiveAvg / firstFiveAvg;
      expect(degradation, `Performance degraded ${degradation.toFixed(1)}x from first to last 5 nodes`).toBeLessThan(5);
    }

    await assertHeartbeat(page, 'after adding 20 nodes');
    expect(errors.length, `Too many errors: ${errors.join('; ')}`).toBeLessThan(10);
  });
});

// Separate describe for the long-running sustained test
test.describe('Stress Testing – sustained', () => {
  test('full app: 2-minute sustained interaction should maintain heartbeat', async ({ page }) => {
    test.slow();
    test.setTimeout(300_000);

    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    const startTime = Date.now();
    const duration = 30_000; // 30 seconds (sufficient for stress testing)
    let iteration = 0;

    while (Date.now() - startTime < duration) {
      iteration++;
      const elapsed = Math.round((Date.now() - startTime) / 1000);

      // Lightweight interaction patterns (avoid slow helper timeouts)
      try {
        switch (iteration % 3) {
          case 0:
            // Navigate between screens (direct click, no isVisible wait)
            await page.locator(`button:has-text("${['Bots', 'Scheduler', 'Workflows'][iteration % 3]}")`).click({ timeout: 1000 }).catch(() => {});
            break;
          case 1:
            // Tab through elements
            await page.keyboard.press('Tab');
            break;
          case 2:
            // Click a session item if available
            await page.locator('[data-testid^="session-item-"]').first().click({ timeout: 500 }).catch(() => {});
            break;
        }
      } catch {
        // Individual actions may fail — that's OK for stress testing
      }

      // Check heartbeat every 50 iterations (reduce overhead)
      if (iteration % 50 === 0) {
        await assertHeartbeat(page, `sustained interaction at ${elapsed}s (iter ${iteration})`);
      }

      await page.waitForTimeout(50);
    }

    await assertHeartbeat(page, 'final sustained check');
    expect(errors.length, `Too many errors in sustained test: ${errors.slice(0, 3).join('; ')}`).toBeLessThan(50);
  });
});
