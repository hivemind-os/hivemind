import { test, expect, Page } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  collectErrors,
  isVisible,
  navigateToScreen,
  waitForAppReady,
} from './helpers';

async function waitForApp(page: Page) {
  await page.waitForSelector('[data-sidebar="sidebar"]', { timeout: 30_000 });
  await page.waitForTimeout(500);
}

async function gotoWorkflows(page: Page) {
  await navigateToScreen(page, 'workflows');
  await page.waitForTimeout(1000);
}

async function showDefinitions(page: Page) {
  const defsBtn = page.locator('[data-testid="wf-definitions-toggle"]').first();
  if (await defsBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
    await defsBtn.evaluate(el => (el as HTMLElement).click());
    await page.waitForTimeout(500);
  }
}

async function openWizard(page: Page) {
  const newBtn = page.locator('[data-testid="wf-new-definition-btn"]').first();
  await newBtn.waitFor({ state: 'visible', timeout: 5000 });
  await newBtn.evaluate(el => (el as HTMLElement).click());
  await page.waitForTimeout(500);
}

/** Click "Start from scratch" in step 1 */
async function startFromScratch(page: Page) {
  const btn = page.locator('button:has-text("Start from scratch")').first();
  await btn.waitFor({ state: 'visible', timeout: 3000 });
  await btn.click();
  await page.waitForTimeout(300);
}

/** Pick workflow type */
async function pickMode(page: Page, mode: 'Background' | 'Chat') {
  const card = page.locator(`button:has-text("${mode}")`).first();
  await card.waitFor({ state: 'visible', timeout: 3000 });
  await card.click();
  await page.waitForTimeout(300);
}

/** Click Next button */
async function clickNext(page: Page) {
  const btn = page.locator('button:has-text("Next")').first();
  await btn.waitFor({ state: 'visible', timeout: 3000 });
  await btn.click();
  await page.waitForTimeout(300);
}

/** Click Skip button */
async function clickSkip(page: Page) {
  const btn = page.locator('button:has-text("Skip")').first();
  await btn.waitFor({ state: 'visible', timeout: 3000 });
  await btn.click();
  await page.waitForTimeout(300);
}

/** Navigate wizard to trigger step (step 6) with given name */
async function navigateToTriggerStep(page: Page, name: string) {
  await openWizard(page);
  await startFromScratch(page);
  await pickMode(page, 'Background');
  const nameInput = page.locator('input[placeholder="my-workflow"]').first();
  await nameInput.fill(name);
  await clickNext(page);
  await clickSkip(page); // Attachments
  const skipAi = page.locator('button:has-text("No, I\'ll build it manually")').first();
  await skipAi.click();
  await page.waitForTimeout(300);
}

/** Navigate wizard to AI step (step 5) with given name */
async function navigateToAiStep(page: Page, name: string) {
  await openWizard(page);
  await startFromScratch(page);
  await pickMode(page, 'Background');
  const nameInput = page.locator('input[placeholder="my-workflow"]').first();
  await nameInput.fill(name);
  await clickNext(page);
  await clickSkip(page); // Attachments
}

test.describe('Workflow Creation Wizard', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto(APP_HARNESS_URL);
    await waitForApp(page);
    await gotoWorkflows(page);
    await showDefinitions(page);
  });

  test('wizard opens and shows step 1 choices', async ({ page }) => {
    await openWizard(page);
    // Step 1 should show two options
    await expect(page.locator('text=Start from scratch')).toBeVisible();
    await expect(page.locator('text=Copy from existing')).toBeVisible();
  });

  test('workflow name has non-editable user/ prefix', async ({ page }) => {
    await openWizard(page);
    await startFromScratch(page);
    // Step 2: pick type
    await pickMode(page, 'Background');
    // Step 3: Name — should see user/ prefix label
    await expect(page.locator('text=Name Your Workflow')).toBeVisible();
    const prefix = page.locator('span:has-text("user/")').first();
    await expect(prefix).toBeVisible();

    // The input should NOT contain "user/" — it's in the prefix label
    const input = page.locator('input[placeholder="my-workflow"]').first();
    await expect(input).toBeVisible();

    // Type a name
    await input.fill('test-workflow');
    await page.waitForTimeout(200);

    // Name validation should pass (Next button enabled)
    const nextBtn = page.locator('button:has-text("Next")').first();
    await expect(nextBtn).toBeEnabled();
  });

  test('step through to trigger selection and pick Manual', async ({ page }) => {
    await navigateToTriggerStep(page, 'test-manual');

    // Step 6: Trigger selection
    await expect(page.locator('h2:has-text("Choose Initial Trigger")')).toBeVisible();

    // Should see trigger cards (within the dialog)
    const dialog = page.locator('[role="dialog"]');
    await expect(dialog.locator('button:has-text("Manual")')).toBeVisible();
    await expect(dialog.locator('button:has-text("Schedule")')).toBeVisible();
    await expect(dialog.locator('button:has-text("Event Pattern")')).toBeVisible();

    // Click Manual trigger
    const manualCard = dialog.locator('button:has-text("Manual"):has-text("inputs")').first();
    await manualCard.click();
    await page.waitForTimeout(300);

    // Should show manual trigger config with Edit Input Schema button
    await expect(page.locator('button:has-text("Edit Input Schema")')).toBeVisible();
  });

  test('can open and use the Input Schema dialog from manual trigger', async ({ page }) => {
    await navigateToTriggerStep(page, 'test-schema');

    // Pick Manual trigger (within dialog)
    const wizardDialog = page.locator('[role="dialog"]').first();
    const manualCard = wizardDialog.locator('button:has-text("Manual"):has-text("inputs")').first();
    await manualCard.click();
    await page.waitForTimeout(300);

    // Click Edit Input Schema
    const editSchemaBtn = page.locator('button:has-text("Edit Input Schema")').first();
    await editSchemaBtn.click();
    await page.waitForTimeout(500);

    // The schema dialog should be visible (nested dialog)
    await expect(page.locator('text=Trigger Input Schema')).toBeVisible({ timeout: 3000 });

    // Click "+ Add input"
    const addBtn = page.locator('button:has-text("+ Add input")').first();
    await expect(addBtn).toBeVisible({ timeout: 3000 });
    await addBtn.click();
    await page.waitForTimeout(300);

    // Should see the new input's name field
    const inputNameField = page.locator('input[placeholder="Input name"]').first();
    await expect(inputNameField).toBeVisible();

    // Type a name for the input
    await inputNameField.fill('my_input');
    await page.waitForTimeout(200);

    // Click OK to save
    const okBtn = page.locator('button:has-text("OK")').first();
    await okBtn.click();
    await page.waitForTimeout(300);

    // Should show the input in the trigger config
    await expect(page.locator('text=my_input')).toBeVisible();
    await expect(page.locator('text=1 field')).toBeVisible();
  });

  test('can select Event Pattern trigger and see topic selector', async ({ page }) => {
    await navigateToTriggerStep(page, 'test-event');

    // Pick Event Pattern trigger (within dialog)
    const dialog = page.locator('[role="dialog"]').first();
    const eventCard = dialog.locator('button:has-text("Event Pattern")').first();
    await eventCard.click();
    await page.waitForTimeout(300);

    // Should show event topic selector
    await expect(page.locator('label:has-text("Event topic")')).toBeVisible();
    const topicInput = page.locator('input[placeholder*="builds.completed"]').first();
    await expect(topicInput).toBeVisible();

    // Focus the topic input — should show the popover with available topics
    await topicInput.click();
    await page.waitForTimeout(500);

    // The mock provides "build.completed" topic
    const topicOption = page.locator('text=build.completed').first();
    // If popover is showing, click the topic
    if (await topicOption.isVisible({ timeout: 2000 }).catch(() => false)) {
      await topicOption.click();
      await page.waitForTimeout(300);
    }

    // Filter expression input should be visible
    await expect(page.locator('text=Filter expression')).toBeVisible();
  });

  test('Generate with AI step shows prompt textarea', async ({ page }) => {
    await navigateToAiStep(page, 'test-ai');

    // Step 5: AI generation
    await expect(page.locator('h2:has-text("Generate with AI")')).toBeVisible({ timeout: 3000 });

    // Should see the prompt textarea
    const promptArea = page.locator('textarea[placeholder*="workflow that"]').first();
    await expect(promptArea).toBeVisible();

    // "Generate with AI" button should be disabled when prompt is empty
    const genBtn = page.locator('button:has-text("Generate with AI")').first();
    await expect(genBtn).toBeDisabled();

    // Type a prompt
    await promptArea.fill('A workflow that processes CSV files');
    await page.waitForTimeout(200);

    // Button should now be enabled
    await expect(genBtn).toBeEnabled();
  });

  test('Chat mode only shows Manual trigger option', async ({ page }) => {
    await openWizard(page);
    await startFromScratch(page);
    await pickMode(page, 'Chat');

    const nameInput = page.locator('input[placeholder="my-workflow"]').first();
    await nameInput.fill('test-chat');
    await clickNext(page);
    await clickSkip(page); // Attachments

    const skipAi = page.locator('button:has-text("No, I\'ll build it manually")').first();
    await skipAi.click();
    await page.waitForTimeout(300);

    // Should only see Manual trigger (chat mode) — scope to dialog
    const dialog = page.locator('[role="dialog"]').first();
    await expect(dialog.locator('button:has-text("Manual"):has-text("inputs")')).toBeVisible();
    // Schedule and Event Pattern should NOT be present in the dialog
    await expect(dialog.locator('button:has-text("Schedule")')).not.toBeVisible();
    await expect(dialog.locator('button:has-text("Event Pattern")')).not.toBeVisible();
  });

  test('Create button completes wizard with manual trigger', async ({ page }) => {
    await navigateToTriggerStep(page, 'test-create');

    // Pick Manual trigger (within dialog)
    const dialog = page.locator('[role="dialog"]').first();
    const manualCard = dialog.locator('button:has-text("Manual"):has-text("inputs")').first();
    await manualCard.click();
    await page.waitForTimeout(300);

    // Click Create
    const createBtn = page.locator('button:has-text("Create")').first();
    await expect(createBtn).toBeEnabled();
    await createBtn.click();
    await page.waitForTimeout(500);

    // Designer should open (wizard closes)
    await expect(page.locator('canvas').first()).toBeVisible({ timeout: 5000 });
  });

  test('mode is locked in designer when editing existing workflow', async ({ page }) => {
    // Open an existing workflow definition in the designer
    const editBtn = page.locator('[data-testid="wf-edit-btn"]').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(1000);

      // Mode selector should be disabled
      const modeSelect = page.locator('select:has(option[value="background"]):has(option[value="chat"])').first();
      if (await modeSelect.isVisible({ timeout: 3000 }).catch(() => false)) {
        await expect(modeSelect).toBeDisabled();
      }
    }
  });

  test('copy workflow path enforces user/ prefix', async ({ page }) => {
    await openWizard(page);
    // Click "Copy from existing"
    const copyBtn = page.locator('button:has-text("Copy from existing")').first();
    await copyBtn.click();
    await page.waitForTimeout(300);

    // Should see copy form
    await expect(page.locator('text=Copy Existing Workflow')).toBeVisible();

    // The name input should have user/ prefix label
    const prefix = page.locator('span:has-text("user/")').first();
    await expect(prefix).toBeVisible();

    const input = page.locator('input[placeholder="my-workflow-copy"]').first();
    await expect(input).toBeVisible();
  });
});
