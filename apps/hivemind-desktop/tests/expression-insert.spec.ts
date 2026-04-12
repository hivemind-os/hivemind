import { test, expect, Page } from '@playwright/test';

const HARNESS_URL = '/tests/harness.html';

async function assertHeartbeat(page: Page, label: string) {
  const ts1 = await page.locator('#heartbeat').getAttribute('data-ts');
  await page.waitForTimeout(1500);
  const ts2 = await page.locator('#heartbeat').getAttribute('data-ts');
  expect(Number(ts2), `Heartbeat stale at "${label}" — app froze`).toBeGreaterThan(Number(ts1));
}

async function clickNode(page: Page, nodeId: string) {
  const nodeEl = page.locator(`[data-testid="node-list"] [data-nodeid="${nodeId}"]`);
  if (await nodeEl.count() === 0) throw new Error(`Node ${nodeId} not found`);
  const gx = Number(await nodeEl.getAttribute('data-x'));
  const gy = Number(await nodeEl.getAttribute('data-y'));
  const canvas = page.locator('canvas').first();
  const box = await canvas.boundingBox();
  if (!box) throw new Error('Canvas not found');
  const screenX = gx + 70 + box.x + box.width / 2;
  const screenY = gy + 23 + box.y + box.height / 2;
  await page.mouse.click(screenX, screenY);
  await page.waitForTimeout(300);
}

test('clicking an expression in the step config dialog should not freeze', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (err) => errors.push(err.message));
  page.on('console', msg => {
    if (msg.type() === 'error') console.log('BROWSER ERROR:', msg.text());
  });

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  await assertHeartbeat(page, 'initial');

  // Select read_step
  await clickNode(page, 'read_step');
  await assertHeartbeat(page, 'after selecting read_step');

  // Open Edit Inputs dialog
  const editInputsBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
  await expect(editInputsBtn).toBeVisible({ timeout: 5000 });
  await editInputsBtn.click();
  await page.waitForTimeout(500);

  const dialog = page.locator('[role="dialog"]');
  await expect(dialog).toBeVisible({ timeout: 5000 });
  console.log('Step config dialog opened');

  // Find expression helper buttons
  const exprButtons = dialog.locator('button:has-text("{{}}"):visible');
  const exprCount = await exprButtons.count();
  console.log(`Found ${exprCount} expression helper buttons`);
  expect(exprCount).toBeGreaterThan(0);

  // Open the expression popup
  await exprButtons.first().click();
  await page.waitForTimeout(500);

  // The popup now uses position:fixed with z-index:10000, so it should be above everything
  // Look for the popup anywhere on the page (not just inside dialog)
  const popupItems = page.locator('div[style*="z-index: 10000"] button[style*="text-align"], div[style*="z-index:10000"] button[style*="text-align"]');
  const itemCount = await popupItems.count();
  console.log(`Found ${itemCount} expression items in popup`);
  expect(itemCount).toBeGreaterThan(0);

  // Click the first expression item — should work without force:true now
  const firstLabel = await popupItems.first().textContent();
  console.log(`Clicking expression: "${firstLabel}"`);
  await popupItems.first().click({ timeout: 5000 });
  await page.waitForTimeout(1000);
  console.log('Expression clicked, checking heartbeat...');

  await assertHeartbeat(page, 'after expression click');
  console.log('Heartbeat OK after expression click');

  // Dialog should still be visible (popup closes, not dialog)
  const dialogStillVisible = await dialog.isVisible();
  console.log(`Dialog still visible: ${dialogStillVisible}`);
  expect(dialogStillVisible).toBe(true);

  // Click OK
  const okBtn = dialog.locator('button.primary:has-text("OK")');
  if (await okBtn.isVisible()) {
    console.log('Clicking OK...');
    await okBtn.click();
    await page.waitForTimeout(500);
  }

  await assertHeartbeat(page, 'after OK click');
  console.log('Heartbeat OK after OK click');
  console.log(`Page errors: ${JSON.stringify(errors)}`);
});

test('clicking multiple expressions in sequence should not freeze', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (err) => errors.push(err.message));

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  await assertHeartbeat(page, 'initial');

  await clickNode(page, 'read_step');
  await page.waitForTimeout(300);

  await page.locator('button:has-text("Edit Inputs"):visible').first().click();
  await page.waitForTimeout(500);

  const dialog = page.locator('[role="dialog"]');
  await expect(dialog).toBeVisible({ timeout: 5000 });

  const exprButtons = dialog.locator('button:has-text("{{}}"):visible');
  const exprCount = await exprButtons.count();
  console.log(`Found ${exprCount} expression helper buttons`);

  for (let round = 0; round < 5; round++) {
    console.log(`Round ${round + 1}: opening expression popup...`);

    // Re-locate buttons since DOM may have changed after insert
    const btn = dialog.locator('button:has-text("{{}}"):visible').first();
    if (!(await btn.isVisible())) {
      console.log('Expression button not visible, skipping round');
      continue;
    }
    await btn.click();
    await page.waitForTimeout(400);

    const popupItems = page.locator('div[style*="z-index: 10000"] button[style*="text-align"], div[style*="z-index:10000"] button[style*="text-align"]');
    const count = await popupItems.count();
    console.log(`  Found ${count} popup items`);

    if (count > 0) {
      const idx = round % count;
      await popupItems.nth(idx).click({ timeout: 5000 });
      console.log(`  Clicked item ${idx}`);
    }

    await page.waitForTimeout(500);
    await assertHeartbeat(page, `after round ${round + 1}`);
    console.log(`  Heartbeat OK after round ${round + 1}`);
  }

  // Verify dialog still open
  expect(await dialog.isVisible()).toBe(true);

  const okBtn = dialog.locator('button.primary:has-text("OK")');
  if (await okBtn.isVisible()) {
    await okBtn.click();
    await page.waitForTimeout(500);
  }

  await assertHeartbeat(page, 'after final OK');
  console.log(`Page errors: ${JSON.stringify(errors)}`);
});

test('expression popup should not close the parent dialog', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (err) => errors.push(err.message));

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });

  await clickNode(page, 'read_step');
  await page.waitForTimeout(300);

  await page.locator('button:has-text("Edit Inputs"):visible').first().click();
  await page.waitForTimeout(500);

  const dialog = page.locator('[role="dialog"]');
  await expect(dialog).toBeVisible({ timeout: 5000 });

  // Open expression popup
  const exprBtn = dialog.locator('button:has-text("{{}}"):visible').first();
  await exprBtn.click();
  await page.waitForTimeout(400);

  // Click an expression
  const popupItems = page.locator('div[style*="z-index: 10000"] button[style*="text-align"], div[style*="z-index:10000"] button[style*="text-align"]');
  const count = await popupItems.count();
  console.log(`Found ${count} items`);
  expect(count).toBeGreaterThan(0);

  await popupItems.nth(0).click({ timeout: 5000 });
  await page.waitForTimeout(500);

  // The dialog should STILL be open — only the popup should close
  expect(await dialog.isVisible(), 'Dialog should remain open after inserting expression').toBe(true);

  // The popup should be closed
  const popupAfter = page.locator('div[style*="z-index: 10000"] button[style*="text-align"], div[style*="z-index:10000"] button[style*="text-align"]');
  expect(await popupAfter.count()).toBe(0);

  console.log('Dialog remains open, popup closed — correct behavior');
  console.log(`Page errors: ${JSON.stringify(errors)}`);
});
