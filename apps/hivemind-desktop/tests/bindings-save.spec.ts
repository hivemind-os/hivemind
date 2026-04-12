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

test('saving output bindings should not freeze', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', err => errors.push(err.message));
  page.on('console', msg => {
    if (msg.type() === 'error') errors.push(`CONSOLE: ${msg.text()}`);
  });

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  await assertHeartbeat(page, 'initial');

  // Select the read_step node
  await clickNode(page, 'read_step');
  await page.waitForTimeout(300);
  await assertHeartbeat(page, 'after selecting read_step');

  // Click "Bindings" button
  const bindingsBtn = page.locator('button:has-text("Bindings"):visible').first();
  await bindingsBtn.click();
  await page.waitForTimeout(500);
  await assertHeartbeat(page, 'after opening bindings dialog');

  // Click "+ Add Mapping"
  const addBtn = page.locator('[role="dialog"] button:has-text("Add Mapping"):visible').first();
  await addBtn.click();
  await page.waitForTimeout(300);
  console.log('Added a mapping');

  // Fill in the key and expression
  const keyInput = page.locator('[role="dialog"] input.wf-launch-input:visible').first();
  const exprInput = page.locator('[role="dialog"] input.wf-launch-input:visible').nth(1);
  
  await keyInput.fill('my_output');
  await keyInput.press('Tab');
  await page.waitForTimeout(100);
  
  await exprInput.fill('{{result.data}}');
  await page.waitForTimeout(100);
  
  console.log('Filled in key and expression');
  await assertHeartbeat(page, 'after filling bindings');

  // Click OK
  console.log('Clicking OK...');
  const okBtn = page.locator('[role="dialog"] button:has-text("OK"):visible').first();
  await okBtn.click();
  
  // Wait for the dialog to close and check heartbeat
  await page.waitForTimeout(500);
  console.log('Dialog should be closed, checking heartbeat...');
  await assertHeartbeat(page, 'AFTER clicking OK on bindings');

  // Wait 5 more seconds to see if delayed freeze
  for (let i = 0; i < 3; i++) {
    await page.waitForTimeout(2000);
    await assertHeartbeat(page, `post-save idle check ${i+1}`);
  }

  // Can we still interact?
  await clickNode(page, 'write_step');
  await assertHeartbeat(page, 'after post-save interaction');

  // Open bindings again to verify the saved data
  await clickNode(page, 'read_step');
  await page.waitForTimeout(300);
  const bindingsBtn2 = page.locator('button:has-text("Bindings"):visible').first();
  await bindingsBtn2.click();
  await page.waitForTimeout(500);
  await assertHeartbeat(page, 'after re-opening bindings');

  console.log('Page errors:', errors);
  expect(errors.filter(e => !e.includes('favicon')).length).toBeLessThan(3);
});

test('multiple binding saves should not freeze', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', err => errors.push(err.message));

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });

  // Do 5 rounds of: open bindings, add mapping, save
  for (let round = 0; round < 5; round++) {
    console.log(`Round ${round + 1}: opening bindings on read_step`);
    await clickNode(page, 'read_step');
    await page.waitForTimeout(200);

    const bindingsBtn = page.locator('button:has-text("Bindings"):visible').first();
    await bindingsBtn.click();
    await page.waitForTimeout(300);

    // Add a mapping
    const addBtn = page.locator('[role="dialog"] button:has-text("Add Mapping"):visible').first();
    await addBtn.click();
    await page.waitForTimeout(200);

    // Fill expression
    const inputs = page.locator('[role="dialog"] input.wf-launch-input:visible');
    const exprInput = inputs.nth(1);
    if (await exprInput.isVisible().catch(() => false)) {
      await exprInput.fill(`{{result.field_${round}}}`);
    }

    // Click OK
    const okBtn = page.locator('[role="dialog"] button:has-text("OK"):visible').first();
    await okBtn.click();
    await page.waitForTimeout(300);
    await assertHeartbeat(page, `after save round ${round + 1}`);
  }

  // Still alive?
  await clickNode(page, 'write_step');
  await assertHeartbeat(page, 'after 5 binding saves');
  console.log('Page errors:', errors);
});
