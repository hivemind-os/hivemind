import { test, expect } from '@playwright/test';
import { APP_HARNESS_URL, waitForAppReady } from '../helpers';

async function navigateToSkillsDiscover(page: import('@playwright/test').Page) {
  await page.goto(APP_HARNESS_URL);
  await waitForAppReady(page);

  const settingsBtn = page.locator('[data-testid="sidebar-settings-btn"], [aria-label="Settings"]').first();
  await settingsBtn.waitFor({ state: 'visible', timeout: 30_000 });
  await settingsBtn.click();
  await page.waitForTimeout(500);

  // Expand Agents & Automation section and select Personas tab
  await page.locator('text=AGENTS & AUTOMATION').first().click();
  await page.waitForTimeout(300);
  await page.getByText('Personas', { exact: true }).click();
  await page.waitForTimeout(1000);

  // Click edit icon on persona card (pencil icon is to the right of the name)
  const coderTitle = page.getByText('Coder', { exact: true }).first();
  const box = await coderTitle.boundingBox();
  if (box) {
    await page.mouse.click(box.x + box.width + 180, box.y + box.height / 2);
    await page.waitForTimeout(1000);
  }

  // Scroll to bottom to see Skills section
  await page.evaluate(() => {
    document.querySelectorAll('*').forEach(el => {
      if (el.scrollHeight > el.clientHeight && el.scrollHeight > 500) {
        el.scrollTop = el.scrollHeight;
      }
    });
  });
  await page.waitForTimeout(500);

  await page.locator('button:has-text("Discover")').click();
  await page.waitForTimeout(2000);
}

async function openAuditDialog(page: import('@playwright/test').Page) {
  const installBtn = page.getByRole('button', { name: 'Install', exact: true });
  await expect(installBtn).toBeVisible({ timeout: 5_000 });
  await installBtn.click();
  const dialog = page.getByRole('dialog', { name: /Security Audit/ });
  await expect(dialog).toBeVisible({ timeout: 5_000 });
  return dialog;
}

test.describe('Skills security audit dialog', () => {
  test('stays open after Install click with all controls visible', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);
    await expect(dialog.getByRole('heading', { name: /Security Audit/ })).toBeVisible();
    await expect(dialog.locator('select')).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Start Audit' })).toBeVisible();
  });

  test('model select is interactable and dialog stays open', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);

    const select = dialog.locator('select').first();
    await select.selectOption({ index: 1 });
    await page.waitForTimeout(300);

    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole('heading', { name: /Security Audit/ })).toBeVisible();
    await expect(dialog.getByRole('button', { name: 'Start Audit' })).toBeEnabled();
  });

  test('Cancel button closes the dialog', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await page.waitForTimeout(300);
    await expect(dialog).not.toBeVisible({ timeout: 3_000 });
  });

  test('Escape key closes the dialog', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);

    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
    await expect(dialog).not.toBeVisible({ timeout: 3_000 });
  });

  test('Install Anyway button works after audit with risks', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);

    // Select a model and run audit
    const select = dialog.locator('select').first();
    await select.selectOption({ index: 1 });
    await page.waitForTimeout(200);

    const startAuditBtn = dialog.getByRole('button', { name: 'Start Audit' });
    await startAuditBtn.click();
    await page.waitForTimeout(1000);

    // Audit results should show risks
    await expect(dialog.getByText('Audit Results')).toBeVisible({ timeout: 5_000 });
    await expect(dialog.getByText('RISK-001')).toBeVisible({ timeout: 3_000 });

    // "Install Anyway" button should be visible
    const installAnywayBtn = dialog.getByRole('button', { name: /Install Anyway/ });
    await expect(installAnywayBtn).toBeVisible({ timeout: 3_000 });
    await expect(installAnywayBtn).toBeEnabled();

    // Click it
    await installAnywayBtn.click();
    await page.waitForTimeout(500);

    // Dialog should close after successful install
    await expect(dialog).not.toBeVisible({ timeout: 5_000 });
  });

  test('Install passes model to backend', async ({ page }) => {
    await navigateToSkillsDiscover(page);
    const dialog = await openAuditDialog(page);

    // Select a model
    const select = dialog.locator('select').first();
    await select.selectOption({ index: 1 });
    const selectedModel = await select.inputValue();
    await page.waitForTimeout(200);

    // Run audit
    await dialog.getByRole('button', { name: 'Start Audit' }).click();
    await page.waitForTimeout(1000);
    await expect(dialog.getByText('Audit Results')).toBeVisible({ timeout: 5_000 });

    // Clear call log, then click Install Anyway
    await page.evaluate(() => {
      (window as any).__TAURI_TEST_INVOKE_CALLS__.length = 0;
    });
    const installAnywayBtn = dialog.getByRole('button', { name: /Install Anyway/ });
    await installAnywayBtn.click();
    await page.waitForTimeout(1000);

    // Verify the install invoke was called with model and audit
    const installCall = await page.evaluate(() => {
      return (window as any).__TAURI_TEST_INVOKE_CALLS__?.find(
        (c: any) => c.command === 'skills_install' || c.command === 'skills_install_for_persona'
      );
    });
    expect(installCall).toBeTruthy();
    expect((installCall as any).args.model).toBe(selectedModel);
    expect((installCall as any).args.model).toBeTruthy();
    // Audit result must be passed through so backend skips re-auditing
    expect((installCall as any).args.audit).toBeTruthy();
    expect((installCall as any).args.audit.risks).toBeTruthy();
  });

  test('Audit error message is visible after failure', async ({ page }) => {
    await navigateToSkillsDiscover(page);

    // Override the audit mock to simulate a failure
    await page.evaluate(() => {
      const internals = (window as any).__TAURI_INTERNALS__;
      const originalInvoke = internals.invoke;
      internals.invoke = async (cmd: string, args: any) => {
        if (cmd === 'skills_audit' || cmd === 'skills_audit_for_persona') {
          throw new Error('model_not_supported: The requested model is not supported.');
        }
        return originalInvoke(cmd, args);
      };
    });

    const dialog = await openAuditDialog(page);

    // Select a model and run audit
    const select = dialog.locator('select').first();
    await select.selectOption({ index: 1 });
    await page.waitForTimeout(200);
    await dialog.getByRole('button', { name: 'Start Audit' }).click();
    await page.waitForTimeout(2000);

    // Error message should be visible (not hidden behind spinner)
    const errorText = dialog.locator('.text-destructive');
    await expect(errorText).toBeVisible({ timeout: 5_000 });
    const text = await errorText.textContent();
    // The frontend translates raw errors to user-friendly messages
    expect(text).toContain('not available');
  });
});
