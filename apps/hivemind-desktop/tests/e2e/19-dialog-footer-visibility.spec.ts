import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  collectErrors,
  navigateToScreen,
  selectFirstSession,
  waitForAppReady,
} from '../helpers';

test.describe('Dialog footer visibility', () => {
  test('session config footer stays visible while the dialog body scrolls', async ({ page }) => {
    const errors = collectErrors(page);

    await page.setViewportSize({ width: 1100, height: 620 });
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await selectFirstSession(page);

    const configBtn = page
      .locator(
        '[data-testid="chat-settings-btn"], [data-testid="session-config-btn"], [aria-label="Chat settings"], [aria-label="Session config"], button[title*="Configure session"]',
      )
      .first();
    await configBtn.waitFor({ state: 'visible', timeout: 30_000 });
    await configBtn.click();

    const dialog = page.locator('[data-testid="session-config-dialog"]');
    await dialog.waitFor({ state: 'visible', timeout: 10_000 });

    const body = dialog.locator('[data-slot="dialog-body"]').first();
    const footerButton = dialog.locator('[data-slot="dialog-footer"] button:has-text("Close")').first();

    await body.evaluate((el) => {
      el.scrollTop = Math.max(0, (el.scrollHeight - el.clientHeight) / 2);
    });

    await expect(footerButton).toBeVisible();
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('wizard dialogs keep their action footer visible while the body scrolls', async ({ page }) => {
    const errors = collectErrors(page);

    await page.setViewportSize({ width: 1100, height: 620 });
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await navigateToScreen(page, 'bots');

    const launchBtn = page.locator('button:has-text("Launch"), button:has-text("New Bot")').first();
    await launchBtn.waitFor({ state: 'visible', timeout: 10_000 });
    await launchBtn.click();

    const wizardBody = page.locator('.channel-wizard-body').first();
    const footer = page.locator('.channel-wizard-footer').first();
    await wizardBody.waitFor({ state: 'visible', timeout: 10_000 });

    await wizardBody.evaluate((el) => {
      el.scrollTop = Math.max(0, (el.scrollHeight - el.clientHeight) / 2);
    });

    await expect(footer).toBeVisible();
    await expect(footer.locator('button').first()).toBeVisible();
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('session permissions dialog keeps save actions visible', async ({ page }) => {
    const errors = collectErrors(page);

    await page.setViewportSize({ width: 1100, height: 620 });
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await selectFirstSession(page);

    const permsBtn = page.locator('[aria-label="Session permissions"]').first();
    await permsBtn.waitFor({ state: 'visible', timeout: 30_000 });
    await permsBtn.click();

    const dialog = page.locator('[data-testid="session-perms-dialog"]');
    await dialog.waitFor({ state: 'visible', timeout: 10_000 });

    const body = dialog.locator('[data-slot="dialog-body"]').first();
    const saveBtn = dialog.locator('[data-slot="dialog-footer"] button:has-text("Save")').first();

    await body.evaluate((el) => {
      el.scrollTop = Math.max(0, el.scrollHeight - el.clientHeight);
    });

    await expect(saveBtn).toBeVisible();
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });
});
