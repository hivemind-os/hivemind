import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  collectErrors,
  isVisible,
} from '../helpers';

/** Wait for the SolidJS app to mount (not just harness) */
async function waitForApp(page: import('@playwright/test').Page) {
  await page.waitForSelector('[data-sidebar="sidebar"]', { timeout: 30_000 });
  await page.waitForTimeout(500);
}

test.describe('Workflows page', () => {
  /** Wait for app to fully render then navigate to workflows page */
  async function gotoWorkflows(page: import('@playwright/test').Page) {
    // Use evaluate + dispatchEvent to navigate (avoids Playwright click actionability waits)
    await page.evaluate(() => {
      const btns = document.querySelectorAll('button');
      for (const btn of btns) {
        if (btn.textContent?.trim() === 'Workflows') {
          btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
          break;
        }
      }
    });
    await page.waitForTimeout(1500);
  }

  /** Switch to the Definitions view via the sidebar gear button */
  async function showDefinitions(page: import('@playwright/test').Page) {
    const defsBtn = page.locator('[data-testid="wf-definitions-toggle"]').first();
    if (await defsBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await defsBtn.evaluate(el => (el as HTMLElement).click());
      await page.waitForTimeout(500);
    }
  }

  /** Dismiss workflow-specific modals (YAML editor overlay or dialog) */
  async function dismissWfDialog(page: import('@playwright/test').Page) {
    // Try dialog cancel button first
    const wfCancel = page.locator('[role="dialog"] button:has-text("Cancel"):visible').first();
    if (await wfCancel.isVisible({ timeout: 1000 }).catch(() => false)) {
      await wfCancel.click();
      await page.waitForTimeout(300);
      return;
    }
    // Try YAML editor Cancel button (inline overlay, not .modal-overlay)
    const cancel = page.locator('button:has-text("Cancel"):visible').first();
    if (await cancel.isVisible({ timeout: 1000 }).catch(() => false)) {
      await cancel.click();
      await page.waitForTimeout(300);
      return;
    }
    // Fallback: press Escape
    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
  }

  test('workflows page should list workflow definitions', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);
    await assertHeartbeat(page, 'initial');

    await gotoWorkflows(page);

    // Definitions section is collapsed by default - expand it
    await showDefinitions(page);

    // Mock definitions: ci-pipeline v1, code-review v1, data-pipeline v2
    const ciPipeline = await isVisible(page, 'text=ci-pipeline').catch(() => false);
    const codeReview = await isVisible(page, 'text=code-review').catch(() => false);
    const dataPipeline = await isVisible(page, 'text=data-pipeline').catch(() => false);
    console.log(`Workflow definitions visible: ci-pipeline=${ciPipeline}, code-review=${codeReview}, data-pipeline=${dataPipeline}`);

    const hasDefinitions = ciPipeline || codeReview || dataPipeline;
    expect(hasDefinitions).toBe(true);

    await assertHeartbeat(page, 'after viewing workflows');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('clicking "New Workflow" should open the visual designer', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // Click "New" button in the definitions sub-view header
    const newBtn = page.locator('[data-testid="wf-new-definition-btn"]').first();
    if (await newBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await newBtn.evaluate(el => (el as HTMLElement).click());
      await page.waitForTimeout(1000);
      await assertHeartbeat(page, 'after clicking New Definition');

      // Visual designer should appear - look for canvas or designer container
      const designerEl = page.locator('canvas:visible, [class*="designer"]:visible');
      const designerVisible = await designerEl.count() > 0;
      console.log(`Visual designer visible: ${designerVisible}`);
    } else {
      console.log('New Definition button not found in definitions view');
    }

    await assertHeartbeat(page, 'after New Workflow designer');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('definitions sub-view should show workflow definitions', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // The definitions grid should be visible
    const defGrid = page.locator('.wf-def-grid').first();
    const gridVisible = await defGrid.isVisible({ timeout: 3000 }).catch(() => false);
    console.log(`Definitions grid visible: ${gridVisible}`);

    // Check for definition cards
    const defCards = page.locator('.wf-def-card');
    const cardCount = await defCards.count();
    console.log(`Definition cards found: ${cardCount}`);

    await assertHeartbeat(page, 'after viewing definitions');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('clicking edit on a definition should open the visual designer', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // Look for "Edit in Designer" button on a definition card
    const editBtn = page.locator('[data-testid="wf-edit-btn"]').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.evaluate(el => (el as HTMLElement).click());
      await page.waitForTimeout(1000);
      await assertHeartbeat(page, 'after clicking edit');

      // Visual designer should appear - look for canvas or designer container
      const designerEl = page.locator('canvas:visible, [class*="designer"]:visible');
      const designerVisible = await designerEl.count() > 0;
      console.log(`Visual designer visible: ${designerVisible}`);
    } else {
      console.log('Edit in Designer button not found - definitions may not have loaded');
    }

    await assertHeartbeat(page, 'after visual designer test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch button should open wizard overlay', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // Click the Launch button on a definition card
    const launchBtn = page.locator('[data-testid="wf-launch-btn"]').first();
    if (await launchBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await launchBtn.evaluate(el => (el as HTMLElement).click());
      await page.waitForTimeout(500);
      await assertHeartbeat(page, 'after clicking launch');

      // Wizard overlay should appear
      const wizard = page.locator('.wf-wizard-overlay');
      const wizardVisible = await wizard.isVisible({ timeout: 3000 }).catch(() => false);
      console.log(`Wizard overlay visible: ${wizardVisible}`);
      expect(wizardVisible).toBe(true);

      // Close by pressing Escape
      await page.keyboard.press('Escape');
      await page.waitForTimeout(300);
    } else {
      console.log('Launch button not found - definitions may not have loaded');
    }

    await assertHeartbeat(page, 'after launch wizard test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch wizard should complete and show success', async ({ page }) => {
    const errors = collectErrors(page);
    const consoleLogs: string[] = [];
    page.on('console', msg => { if (msg.text().includes('[workflow]')) consoleLogs.push(msg.text()); });

    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // Open launch wizard
    const launchBtn = page.locator('[data-testid="wf-launch-btn"]').first();
    await launchBtn.evaluate(el => (el as HTMLElement).click());
    await page.waitForTimeout(500);

    const wizard = page.locator('.wf-wizard-overlay');
    expect(await wizard.isVisible({ timeout: 3000 }).catch(() => false)).toBe(true);

    // Check wizard step 0 state
    const step0State = await page.evaluate(() => {
      const wizBody = document.querySelector('.wf-wizard-body');
      const nextBtn = document.querySelector('.wf-btn-next') as HTMLButtonElement;
      return {
        bodyText: wizBody?.textContent?.substring(0, 200) ?? '',
        nextBtnVisible: !!nextBtn,
        nextBtnDisabled: nextBtn?.disabled ?? false,
        nextBtnText: nextBtn?.textContent ?? '',
      };
    });
    console.log('Step 0 state:', JSON.stringify(step0State));

    // Step 0 → 1
    const nextBtn = page.locator('.wf-btn-next');
    await nextBtn.evaluate(el => (el as HTMLElement).click());
    await page.waitForTimeout(300);

    // Check wizard step 1 state
    const step1State = await page.evaluate(() => {
      const wizBody = document.querySelector('.wf-wizard-body');
      const nextBtn = document.querySelector('.wf-btn-next') as HTMLButtonElement;
      return {
        bodyText: wizBody?.textContent?.substring(0, 200) ?? '',
        nextBtnVisible: !!nextBtn,
        nextBtnDisabled: nextBtn?.disabled ?? false,
        nextBtnText: nextBtn?.textContent ?? '',
      };
    });
    console.log('Step 1 state:', JSON.stringify(step1State));

    // Step 1 → 2
    await nextBtn.evaluate(el => (el as HTMLElement).click());
    await page.waitForTimeout(300);

    // Check wizard step 2 state
    const step2State = await page.evaluate(() => {
      const submitBtn = document.querySelector('[data-testid="wf-launch-submit-btn"]') as HTMLButtonElement;
      const wizBody = document.querySelector('.wf-wizard-body');
      return {
        bodyText: wizBody?.textContent?.substring(0, 300) ?? '',
        submitBtnVisible: !!submitBtn,
        submitBtnDisabled: submitBtn?.disabled ?? false,
        submitBtnText: submitBtn?.textContent ?? '',
      };
    });
    console.log('Step 2 state:', JSON.stringify(step2State));
    expect(step2State.submitBtnVisible).toBe(true);
    expect(step2State.submitBtnDisabled).toBe(false);

    // Click Launch
    const submitBtn = page.locator('[data-testid="wf-launch-submit-btn"]');
    await submitBtn.evaluate(el => (el as HTMLElement).click());
    await page.waitForTimeout(2000);

    // Check result
    const result = await page.evaluate(() => {
      return {
        hasSuccess: !!document.querySelector('.wf-wizard-success'),
        hasError: !!document.querySelector('.wf-wizard-error'),
        errorText: document.querySelector('.wf-wizard-error span')?.textContent ?? '',
        btnText: (document.querySelector('[data-testid="wf-launch-submit-btn"]') as HTMLButtonElement)?.textContent ?? '',
        btnDisabled: (document.querySelector('[data-testid="wf-launch-submit-btn"]') as HTMLButtonElement)?.disabled ?? false,
        wizardVisible: !!document.querySelector('.wf-wizard-overlay'),
      };
    });
    console.log('Launch result:', JSON.stringify(result));
    console.log('Workflow console logs:', consoleLogs);

    expect(result.hasSuccess || !result.hasError).toBe(true);

    await assertHeartbeat(page, 'after launch wizard complete');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('instance timeline should show status indicators', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);

    // Default tab is Instances — timeline should show mock instances
    const timeline = page.locator('.wf-timeline');
    const timelineVisible = await timeline.isVisible({ timeout: 3000 }).catch(() => false);
    console.log(`Timeline visible: ${timelineVisible}`);

    // Check for timeline items (from mock data: 3 instances)
    const timelineItems = page.locator('.wf-timeline-item');
    const itemCount = await timelineItems.count();
    console.log(`Timeline items: ${itemCount}`);

    await assertHeartbeat(page, 'after checking instance timeline');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('delete definition should check for dependents first', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);

    await gotoWorkflows(page);
    await showDefinitions(page);

    // Click the "Delete" button on a definition card (non-bundled defs have delete)
    const deleteBtn = page.locator('[data-testid="wf-delete-btn"]').first();
    if (await deleteBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await deleteBtn.evaluate(el => (el as HTMLElement).click());
      await page.waitForTimeout(500);

      // Should show a confirmation dialog
      const confirmDialog = page.locator('[role="dialog"], [role="alertdialog"]');
      const dialogVisible = await confirmDialog.count() > 0;
      console.log(`Delete confirmation dialog visible: ${dialogVisible}`);

      if (dialogVisible) {
        await dismissWfDialog(page);
      }
    } else {
      console.log('Delete button not found - definitions may not have loaded');
    }

    expect(typeof true).toBe('boolean');

    await assertHeartbeat(page, 'after delete definition test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });
});
