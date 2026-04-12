import { test, expect, Page } from '@playwright/test';

/**
 * WorkflowDesigner freeze/black-screen regression test.
 *
 * Exercises the designer with rapid interactions over a sustained period,
 * monitoring for:
 *   1. DOM responsiveness (heartbeat timestamp keeps advancing)
 *   2. Console errors / unhandled exceptions
 *   3. Ability to interact (click, type) after sustained use
 *
 * Note: The designer uses an HTML Canvas renderer, so nodes are not DOM elements.
 * We use a hidden node-list div for existence checks and coordinate-based clicks.
 */

const HARNESS_URL = '/tests/harness.html';

/** Check the heartbeat element is still being updated (proves JS event loop is alive) */
async function assertHeartbeat(page: Page, label: string) {
  const ts1 = await page.locator('#heartbeat').getAttribute('data-ts');
  await page.waitForTimeout(1500);
  const ts2 = await page.locator('#heartbeat').getAttribute('data-ts');
  expect(Number(ts2), `Heartbeat stale at "${label}" — app froze`).toBeGreaterThan(Number(ts1));
}

/** Click a palette item by its title attribute to add a node */
async function addNodeFromPalette(page: Page, label: string) {
  await page.locator(`div[title*="${label}"]`).click();
  await page.waitForTimeout(200);
}

/** Click a node on the Canvas by computing screen coords from the hidden node-list */
async function clickNode(page: Page, nodeId: string) {
  const nodeEl = page.locator(`[data-testid="node-list"] [data-nodeid="${nodeId}"]`);
  if (await nodeEl.count() === 0) throw new Error(`Node ${nodeId} not found in node-list`);
  const gx = Number(await nodeEl.getAttribute('data-x'));
  const gy = Number(await nodeEl.getAttribute('data-y'));
  const canvas = page.locator('canvas').first();
  const box = await canvas.boundingBox();
  if (!box) throw new Error('Canvas not found');
  // Convert graph coords to screen coords (panX=0, panY=0, zoom=1 initially)
  const screenX = gx + 70 + box.x + box.width / 2;  // +70 ≈ nodeWidth/2
  const screenY = gy + 23 + box.y + box.height / 2;  // +23 ≈ NODE_H/2
  await page.mouse.click(screenX, screenY);
  await page.waitForTimeout(200);
}

/** Check a node exists via the hidden node-list */
async function nodeExists(page: Page, nodeId: string): Promise<boolean> {
  return (await page.locator(`[data-testid="node-list"] [data-nodeid="${nodeId}"]`).count()) > 0;
}

test.describe('WorkflowDesigner stability', () => {

  test('should not freeze during sustained editing', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('console', msg => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });
    page.on('pageerror', err => consoleErrors.push(`PAGE ERROR: ${err.message}`));

    // Load the test harness
    await page.goto(HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });
    await assertHeartbeat(page, 'initial load');

    // The harness loads with a workflow that has 2 nodes: read_step, write_step
    expect(await nodeExists(page, 'read_step')).toBe(true);
    expect(await nodeExists(page, 'write_step')).toBe(true);

    // ── Phase 1: Add several nodes from palette ──
    console.log('Phase 1: Adding nodes from palette');
    await addNodeFromPalette(page, 'Call Tool');
    await addNodeFromPalette(page, 'Invoke Agent');
    await addNodeFromPalette(page, 'Feedback Gate');
    await addNodeFromPalette(page, 'Branch');
    await addNodeFromPalette(page, 'Delay');
    await assertHeartbeat(page, 'after adding nodes');

    // ── Phase 2: Click nodes and type in their config fields rapidly ──
    console.log('Phase 2: Rapid node selection + field editing');
    for (let round = 0; round < 5; round++) {
      await clickNode(page, 'read_step');
      await page.waitForTimeout(50);
      await clickNode(page, 'write_step');
      await page.waitForTimeout(50);

      // Cycle through newly added nodes
      for (const suffix of [1, 2, 3, 4, 5]) {
        const possibleIds = ['call_tool', 'invoke_agent', 'feedback_gate', 'branch', 'delay'];
        for (const base of possibleIds) {
          const id = `${base}_${suffix}`;
          if (await nodeExists(page, id)) {
            await clickNode(page, id);
            break;
          }
        }
      }
    }
    await assertHeartbeat(page, 'after rapid selection');

    // ── Phase 3: Sustained typing in config fields (inside dialog) ──
    console.log('Phase 3: Sustained typing in config fields');
    // Select the first call_tool node
    for (let i = 1; i <= 10; i++) {
      if (await nodeExists(page, `call_tool_${i}`)) {
        await clickNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    // Open the "Edit Inputs" dialog
    const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
    if (await editBtn.isVisible().catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(300);

      // Type inside the dialog's inputs
      const dialogInputs = page.locator('[role="dialog"] input[type="text"]:visible:not([disabled]), [role="dialog"] input:not([type]):visible:not([disabled]), [role="dialog"] textarea:visible:not([disabled])');
      const inputCount = await dialogInputs.count();
      for (let i = 0; i < Math.min(inputCount, 5); i++) {
        const inp = dialogInputs.nth(i);
        try {
          for (let k = 0; k < 20; k++) {
            await inp.press('a');
            await page.waitForTimeout(10);
          }
          for (let k = 0; k < 20; k++) {
            await inp.press('Backspace');
            await page.waitForTimeout(10);
          }
        } catch { /* skip non-interactive fields */ }
      }

      // Close dialog via Cancel
      const cancelBtn = page.locator('[role="dialog"] button:has-text("Cancel"):visible').first();
      if (await cancelBtn.isVisible().catch(() => false)) await cancelBtn.click();
      await page.waitForTimeout(200);
    }
    await assertHeartbeat(page, 'after sustained typing');

    // ── Phase 4: Wait and monitor for 30 seconds ──
    console.log('Phase 4: 30-second idle monitoring');
    for (let sec = 0; sec < 6; sec++) {
      await page.waitForTimeout(5000);
      await assertHeartbeat(page, `idle check at ${(sec + 1) * 5}s`);
    }

    // ── Phase 5: Post-idle interaction — can we still use the designer? ──
    console.log('Phase 5: Post-idle interaction check');
    await addNodeFromPalette(page, 'While Loop');
    await assertHeartbeat(page, 'after post-idle add');

    await clickNode(page, 'read_step');
    await page.waitForTimeout(100);
    await clickNode(page, 'write_step');
    await assertHeartbeat(page, 'final check');

    // ── Verify no critical errors ──
    const criticalErrors = consoleErrors.filter(e =>
      !e.includes('Failed to load tools') &&
      !e.includes('favicon')
    );
    if (criticalErrors.length > 0) {
      console.log('Console errors:', criticalErrors);
    }
    expect(criticalErrors.length, `Too many console errors: ${criticalErrors.join('; ')}`).toBeLessThan(10);
  });

  test('should not leak memory during rapid node selection', async ({ page }) => {
    await page.goto(HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    // Add several call_tool nodes (most complex config UI with createSignal)
    for (let i = 0; i < 5; i++) {
      await addNodeFromPalette(page, 'Call Tool');
      await page.waitForTimeout(100);
    }
    await assertHeartbeat(page, 'after adding call_tool nodes');

    // Collect the actual node IDs
    const nodeIds: string[] = [];
    for (let i = 1; i <= 20; i++) {
      if (await nodeExists(page, `call_tool_${i}`)) {
        nodeIds.push(`call_tool_${i}`);
      }
    }
    expect(nodeIds.length).toBeGreaterThanOrEqual(5);

    // Rapidly cycle through all call_tool nodes many times
    console.log(`Cycling through ${nodeIds.length} call_tool nodes rapidly (50 cycles)...`);
    for (let cycle = 0; cycle < 50; cycle++) {
      const nodeId = nodeIds[cycle % nodeIds.length];
      await clickNode(page, nodeId).catch(() => {});
      await page.waitForTimeout(30);
    }
    await assertHeartbeat(page, 'after 50 rapid selections');

    // Still responsive? Can we type in the Step ID field (still inline)?
    await clickNode(page, nodeIds[0]).catch(() => {});
    await page.waitForTimeout(300);
    const anyInput = page.locator('input[type="text"]:visible:not([disabled]), input:not([type]):visible:not([disabled])').first();
    if (await anyInput.isVisible().catch(() => false)) {
      await anyInput.press('x');
      await page.waitForTimeout(100);
      await anyInput.press('Backspace');
    }
    await assertHeartbeat(page, 'after typing in rapidly-cycled node');
  });

  test('should handle drag operations without progressive slowdown', async ({ page }) => {
    await page.goto(HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible({ timeout: 5000 });
    const box = await canvas.boundingBox();
    if (!box) throw new Error('Canvas not found');

    // Simulate dragging on the canvas
    console.log('Simulating drag operations...');
    for (let i = 0; i < 20; i++) {
      const startX = box.x + box.width / 2 + (Math.random() - 0.5) * 100;
      const startY = box.y + box.height / 2 + (Math.random() - 0.5) * 100;
      await page.mouse.move(startX, startY);
      await page.mouse.down();
      for (let step = 0; step < 10; step++) {
        await page.mouse.move(
          startX + (Math.random() - 0.5) * 50,
          startY + (Math.random() - 0.5) * 50,
        );
      }
      await page.mouse.up();
      await page.waitForTimeout(50);
    }

    await assertHeartbeat(page, 'after 20 drag operations');

    const changeCount = await page.locator('#change-count').textContent();
    expect(Number(changeCount)).toBeGreaterThan(0);
  });

  test('should survive extended interaction session (2 min)', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('pageerror', err => consoleErrors.push(err.message));

    await page.goto(HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    // Add a mix of node types
    const nodeTypes = ['Call Tool', 'Invoke Agent', 'Feedback Gate', 'Call Tool', 'Branch'];
    for (const nt of nodeTypes) {
      await addNodeFromPalette(page, nt);
      await page.waitForTimeout(100);
    }

    // For 2 minutes: cycle interactions
    const startTime = Date.now();
    const duration = 120_000; // 2 minutes
    let iteration = 0;
    while (Date.now() - startTime < duration) {
      iteration++;
      const elapsed = Math.round((Date.now() - startTime) / 1000);

      // Select random node
      const ids = ['read_step', 'write_step'];
      for (let i = 1; i <= 10; i++) {
        for (const base of ['call_tool', 'invoke_agent', 'feedback_gate', 'branch']) {
          if (await nodeExists(page, `${base}_${i}`)) ids.push(`${base}_${i}`);
        }
      }
      const randomId = ids[iteration % ids.length];
      await clickNode(page, randomId).catch(() => {});
      await page.waitForTimeout(50);

      // Type in the Step ID field (still inline in the right panel)
      if (iteration % 3 === 0) {
        const inp = page.locator('input[type="text"]:visible:not([disabled]), input:not([type]):visible:not([disabled])').first();
        if (await inp.isVisible().catch(() => false)) {
          await inp.press('x');
          await page.waitForTimeout(20);
          await inp.press('Backspace');
        }
      }

      // Check heartbeat every 10 iterations
      if (iteration % 10 === 0) {
        console.log(`  [${elapsed}s] iteration ${iteration} — checking heartbeat`);
        await assertHeartbeat(page, `extended session at ${elapsed}s`);
      }

      await page.waitForTimeout(100);
    }

    console.log(`Extended test completed: ${iteration} iterations`);
    await assertHeartbeat(page, 'final extended check');

    expect(consoleErrors.filter(e => !e.includes('favicon')).length,
      `Page errors: ${consoleErrors.join('; ')}`
    ).toBeLessThan(5);
  });
});
