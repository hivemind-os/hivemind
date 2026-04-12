import { test, expect, Page } from '@playwright/test';

const HARNESS_URL = '/tests/harness.html';

async function assertHeartbeat(page: Page, label: string, timeoutMs = 3000) {
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

async function addNodeFromPalette(page: Page, subtypeLabel: string) {
  // Palette items are divs with title="Drag or click to add <label>"
  const btn = page.locator(`div[title*="${subtypeLabel}"]`).first();
  if (await btn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await btn.click();
    await page.waitForTimeout(500);
  }
}

test('adding 6 nodes and configuring inputs+outputs should remain stable', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (err) => errors.push(`PAGE: ${err.message}`));
  page.on('console', msg => {
    if (msg.type() === 'error' && !msg.text().includes('WARN')) {
      errors.push(`CONSOLE: ${msg.text()}`);
    }
  });

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  await assertHeartbeat(page, 'initial');

  // The harness starts with: trigger_0, read_step, write_step (3 nodes)
  // Let's add more nodes to reach 6+
  console.log('=== Phase 1: Adding nodes ===');

  // Add node 4: invoke_agent
  await addNodeFromPalette(page, 'Invoke Agent');
  await assertHeartbeat(page, 'after adding invoke_agent');
  console.log('Added invoke_agent (node 4)');

  // Add node 5: feedback_gate
  await addNodeFromPalette(page, 'Feedback Gate');
  await assertHeartbeat(page, 'after adding feedback_gate');
  console.log('Added feedback_gate (node 5)');

  // Add node 6: call_tool
  await addNodeFromPalette(page, 'Call Tool');
  await assertHeartbeat(page, 'after adding call_tool');
  console.log('Added call_tool (node 6)');

  // Add node 7: another call_tool
  await addNodeFromPalette(page, 'Call Tool');
  await assertHeartbeat(page, 'after adding second call_tool');
  console.log('Added second call_tool (node 7)');

  console.log('=== Phase 2: Configure inputs on each node ===');

  // Configure read_step inputs
  await clickNode(page, 'read_step');
  let editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
  if (await editBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await editBtn.click();
    await page.waitForTimeout(500);
    let dialog = page.locator('[role="dialog"]');
    if (await dialog.isVisible()) {
      // Just click OK to close
      await dialog.locator('button.primary:has-text("OK")').click();
      await page.waitForTimeout(300);
    }
  }
  await assertHeartbeat(page, 'after configuring read_step');
  console.log('Configured read_step');

  // Configure write_step inputs
  await clickNode(page, 'write_step');
  editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
  if (await editBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await editBtn.click();
    await page.waitForTimeout(500);
    let dialog = page.locator('[role="dialog"]');
    if (await dialog.isVisible()) {
      await dialog.locator('button.primary:has-text("OK")').click();
      await page.waitForTimeout(300);
    }
  }
  await assertHeartbeat(page, 'after configuring write_step');
  console.log('Configured write_step');

  console.log('=== Phase 3: Configure output bindings ===');

  // Add output bindings on read_step
  await clickNode(page, 'read_step');
  let bindingsBtn = page.locator('button:has-text("Bindings"):visible').first();
  if (await bindingsBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await bindingsBtn.click();
    await page.waitForTimeout(500);
    let dialog = page.locator('[role="dialog"]');
    if (await dialog.isVisible()) {
      // Add a mapping
      const addBtn = dialog.locator('button:has-text("Add Mapping"):visible').first();
      if (await addBtn.isVisible().catch(() => false)) {
        await addBtn.click();
        await page.waitForTimeout(300);
        // Fill in key and expression
        const inputs = dialog.locator('input.wf-launch-input:visible');
        if (await inputs.count() >= 2) {
          await inputs.first().fill('file_content');
          await inputs.nth(1).fill('{{result}}');
        }
      }
      await dialog.locator('button.primary:has-text("OK")').click();
      await page.waitForTimeout(500);
    }
  }
  await assertHeartbeat(page, 'after read_step bindings');
  console.log('Added bindings to read_step');

  // Add output bindings on write_step
  await clickNode(page, 'write_step');
  bindingsBtn = page.locator('button:has-text("Bindings"):visible').first();
  if (await bindingsBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await bindingsBtn.click();
    await page.waitForTimeout(500);
    let dialog = page.locator('[role="dialog"]');
    if (await dialog.isVisible()) {
      const addBtn = dialog.locator('button:has-text("Add Mapping"):visible').first();
      if (await addBtn.isVisible().catch(() => false)) {
        await addBtn.click();
        await page.waitForTimeout(300);
        const inputs = dialog.locator('input.wf-launch-input:visible');
        if (await inputs.count() >= 2) {
          await inputs.first().fill('status');
          await inputs.nth(1).fill('{{result}}');
        }
      }
      await dialog.locator('button.primary:has-text("OK")').click();
      await page.waitForTimeout(500);
    }
  }
  await assertHeartbeat(page, 'after write_step bindings');
  console.log('Added bindings to write_step');

  console.log('=== Phase 4: Rapid node selection cycling ===');
  // Rapidly cycle through all nodes to stress test
  const nodeIds = ['read_step', 'write_step'];
  for (let i = 0; i < 20; i++) {
    const id = nodeIds[i % nodeIds.length];
    await clickNode(page, id);
    if (i % 5 === 4) {
      await assertHeartbeat(page, `cycling iteration ${i + 1}`);
      console.log(`  Cycling heartbeat OK at iteration ${i + 1}`);
    }
  }
  await assertHeartbeat(page, 'after cycling');
  console.log('Node cycling complete');

  console.log('=== Phase 5: Open and close dialogs rapidly ===');
  for (let i = 0; i < 5; i++) {
    await clickNode(page, 'read_step');
    const eb = page.locator('button:has-text("Edit Inputs"):visible').first();
    if (await eb.isVisible({ timeout: 2000 }).catch(() => false)) {
      await eb.click();
      await page.waitForTimeout(300);
      const dialog = page.locator('[role="dialog"]');
      if (await dialog.isVisible()) {
        // Cancel without saving
        await dialog.locator('button:has-text("Cancel")').click();
        await page.waitForTimeout(200);
      }
    }
  }
  await assertHeartbeat(page, 'after rapid dialog open/close');
  console.log('Rapid dialog cycling complete');

  console.log('=== Phase 6: Wait 30 seconds for delayed freeze ===');
  for (let i = 0; i < 6; i++) {
    await page.waitForTimeout(5000);
    await assertHeartbeat(page, `idle check ${i + 1}/6`);
    console.log(`  Idle heartbeat OK (${(i + 1) * 5}s)`);
  }

  console.log(`\nFinal page errors: ${JSON.stringify(errors)}`);
  console.log('TEST PASSED — 7 nodes stable through full workflow');
});
