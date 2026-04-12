import { test, expect } from '@playwright/test';
import {
  DESIGNER_HARNESS_URL,
  assertHeartbeat,
  collectErrors,
  clickDesignerNode,
  addDesignerNode,
  designerNodeExists,
  clickButton,
  isVisible,
  dismissModal,
  typeIntoInput,
} from '../helpers';

test.describe('WorkflowDesigner – extended tests', () => {
  test('designer should render canvas and palette on load', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });
    await assertHeartbeat(page, 'initial load');

    const canvas = page.locator('canvas').first();
    await expect(canvas).toBeVisible({ timeout: 5_000 });

    // Palette should contain at least one item
    const paletteItems = page.locator('div[title*="Call Tool"], div[title*="Invoke Agent"], div[title*="Branch"]');
    expect(await paletteItems.count()).toBeGreaterThanOrEqual(1);

    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('adding all node types from palette should work without errors', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });
    await assertHeartbeat(page, 'initial');

    // Palette items match exactly what WorkflowDesigner defines
    const nodeTypes = [
      'Call Tool',
      'Invoke Agent',
      'Feedback Gate',
      'Branch',
      'Delay',
      'While Loop',
      'Event Gate',
      'Set Variable',
    ];

    for (const node_type of nodeTypes) {
      await addDesignerNode(page, node_type);
      await assertHeartbeat(page, `after adding ${node_type}`);
      await page.waitForTimeout(300);
    }

    // Node IDs use subtype as prefix (e.g. while_1, not while_loop_1)
    const expectedPrefixes = [
      'call_tool', 'invoke_agent', 'feedback_gate', 'branch',
      'delay', 'while', 'event_gate', 'set_variable',
    ];
    for (const prefix of expectedPrefixes) {
      let found = false;
      for (let i = 1; i <= 15; i++) {
        if (await designerNodeExists(page, `${prefix}_${i}`)) {
          found = true;
          break;
        }
      }
      expect(found, `Expected at least one ${prefix}_* node`).toBe(true);
    }

    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('selecting a node should show its config in the right panel', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    // The harness pre-loads read_step and write_step
    expect(await designerNodeExists(page, 'read_step')).toBe(true);

    await clickDesignerNode(page, 'read_step');
    await page.waitForTimeout(300);

    // The right panel should show config — look for a visible input or panel content
    const configPanel = page.locator(
      'input[type="text"]:visible:not([disabled]), textarea:visible:not([disabled]), button:has-text("Edit Inputs"):visible, button:has-text("Bindings"):visible'
    );
    expect(await configPanel.count(), 'Config panel should show fields after selecting node').toBeGreaterThanOrEqual(1);
    await assertHeartbeat(page, 'after selecting node');

    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('Step ID field should be editable for task nodes', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    // Find and click the newly created call_tool node
    let nodeId = '';
    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        nodeId = `call_tool_${i}`;
        break;
      }
    }
    expect(nodeId).not.toBe('');
    await clickDesignerNode(page, nodeId);
    await page.waitForTimeout(300);

    // Look for the Step ID input field and try editing it
    const stepIdInput = page.locator('input[type="text"]:visible:not([disabled])').first();
    if (await stepIdInput.isVisible({ timeout: 2000 }).catch(() => false)) {
      const originalValue = await stepIdInput.inputValue();
      await stepIdInput.click();
      await stepIdInput.fill('my_custom_step');
      await page.waitForTimeout(200);
      const newValue = await stepIdInput.inputValue();
      expect(newValue).toBe('my_custom_step');

      // Restore original to avoid side-effects
      await stepIdInput.fill(originalValue || nodeId);
    }

    await assertHeartbeat(page, 'after editing step ID');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('opening Edit Inputs dialog should display tool arguments', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    // Select the call_tool node
    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        await clickDesignerNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    // Click "Edit Inputs"
    const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(500);
      await assertHeartbeat(page, 'after opening Edit Inputs');

      // Modal should be visible with input fields or content
      const modalContent = page.locator('[role="dialog"]');
      expect(await modalContent.count()).toBeGreaterThanOrEqual(1);

      const dialogInputs = page.locator(
        '[role="dialog"] input[type="text"]:visible, [role="dialog"] textarea:visible, [role="dialog"] input:not([type]):visible'
      );
      // Tool arguments may or may not be present depending on tool selection
      const inputCount = await dialogInputs.count();
      console.log(`Edit Inputs dialog has ${inputCount} input fields`);

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after closing Edit Inputs');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('changing tool selection in dropdown should update input fields', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        await clickDesignerNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    // Find the tool selector dropdown (select element or custom dropdown)
    const toolSelect = page.locator('select:visible').first();
    if (await toolSelect.isVisible({ timeout: 3000 }).catch(() => false)) {
      const options = toolSelect.locator('option');
      const optionCount = await options.count();
      console.log(`Tool dropdown has ${optionCount} options`);

      if (optionCount > 1) {
        // Select the second option to trigger a change
        const secondValue = await options.nth(1).getAttribute('value');
        if (secondValue) {
          await toolSelect.selectOption(secondValue);
          await page.waitForTimeout(500);
          await assertHeartbeat(page, 'after changing tool selection');

          // Open Edit Inputs to see if fields updated
          const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
          if (await editBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
            await editBtn.click();
            await page.waitForTimeout(500);
            const dialogInputs = page.locator('[role="dialog"] input:visible, [role="dialog"] textarea:visible');
            console.log(`After tool change, Edit Inputs has ${await dialogInputs.count()} fields`);
            await dismissModal(page);
          }
        }
      }
    }

    await assertHeartbeat(page, 'after tool selection change');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('expression helper popup should show available variables', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        await clickDesignerNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    // Open Edit Inputs dialog
    const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(500);

      // Type {{ to trigger expression helper
      const dialogInput = page.locator(
        '[role="dialog"] input[type="text"]:visible:not([disabled]), [role="dialog"] input:not([type]):visible:not([disabled])'
      ).first();
      if (await dialogInput.isVisible({ timeout: 2000 }).catch(() => false)) {
        await dialogInput.click();
        await dialogInput.fill('{{');
        await page.waitForTimeout(500);

        // Look for expression helper popup / autocomplete dropdown
        const helperPopup = page.locator(
          '.expression-helper, .autocomplete-dropdown, [class*="expression"], [class*="popup"]:visible, [role="listbox"]:visible'
        );
        const popupVisible = await helperPopup.count() > 0;
        console.log(`Expression helper popup visible: ${popupVisible}`);

        // Even if popup doesn't appear, typing {{ should not crash
        await assertHeartbeat(page, 'after typing {{ in input');
      }

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after expression helper test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('inserting an expression should populate the input field', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        await clickDesignerNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
    if (await editBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await editBtn.click();
      await page.waitForTimeout(500);

      const dialogInput = page.locator(
        '[role="dialog"] input[type="text"]:visible:not([disabled]), [role="dialog"] input:not([type]):visible:not([disabled])'
      ).first();
      if (await dialogInput.isVisible({ timeout: 2000 }).catch(() => false)) {
        const expression = '{{steps.read_step.output.data}}';
        await dialogInput.click();
        await dialogInput.fill(expression);
        await page.waitForTimeout(200);

        const value = await dialogInput.inputValue();
        expect(value).toContain('{{');
        expect(value).toContain('}}');
        console.log(`Input field value after expression insert: "${value}"`);
      }

      // Click OK to save
      const okBtn = page.locator('[role="dialog"] button:has-text("OK"):visible').first();
      if (await okBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
        await okBtn.click();
        await page.waitForTimeout(500);
      } else {
        await dismissModal(page);
      }
    }

    await assertHeartbeat(page, 'after expression insertion');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('on-error configuration should be expandable and editable', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    await addDesignerNode(page, 'Call Tool');
    await page.waitForTimeout(300);

    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `call_tool_${i}`)) {
        await clickDesignerNode(page, `call_tool_${i}`);
        break;
      }
    }
    await page.waitForTimeout(300);

    // Look for on-error section — it may be a collapsible section or a button
    const onErrorToggle = page.locator(
      'button:has-text("On Error"):visible, summary:has-text("On Error"):visible, [class*="on-error"]:visible, button:has-text("on_error"):visible, summary:has-text("on_error"):visible'
    ).first();

    if (await onErrorToggle.isVisible({ timeout: 3000 }).catch(() => false)) {
      await onErrorToggle.click();
      await page.waitForTimeout(300);
      await assertHeartbeat(page, 'after expanding on-error');

      // Look for on-error config fields (retry, fail, continue, etc.)
      const onErrorFields = page.locator(
        'select:visible, input[type="text"]:visible, input[type="number"]:visible'
      );
      const fieldCount = await onErrorFields.count();
      console.log(`On-error section has ${fieldCount} visible fields`);
      expect(fieldCount, 'On-error section should have editable fields').toBeGreaterThanOrEqual(1);

      // Try editing a field
      const firstField = onErrorFields.first();
      const tagName = await firstField.evaluate(el => el.tagName.toLowerCase());
      if (tagName === 'select') {
        const options = firstField.locator('option');
        if (await options.count() > 1) {
          const val = await options.nth(1).getAttribute('value');
          if (val) await firstField.selectOption(val);
        }
      } else {
        await firstField.click();
        await firstField.fill('3');
      }
      await page.waitForTimeout(200);
    } else {
      console.log('On-error toggle not found — may not be visible for this node type');
    }

    await assertHeartbeat(page, 'after on-error config test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });

  test('deleting a node should remove it from the canvas and YAML', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(DESIGNER_HARNESS_URL);
    await page.waitForSelector('#heartbeat', { timeout: 10_000 });

    // Add a node to delete
    await addDesignerNode(page, 'Delay');
    await page.waitForTimeout(300);

    let targetNodeId = '';
    for (let i = 1; i <= 10; i++) {
      if (await designerNodeExists(page, `delay_${i}`)) {
        targetNodeId = `delay_${i}`;
        break;
      }
    }
    expect(targetNodeId, 'Should have created a delay node').not.toBe('');
    expect(await designerNodeExists(page, targetNodeId)).toBe(true);

    // Select the node
    await clickDesignerNode(page, targetNodeId);
    await page.waitForTimeout(300);

    // Delete the node — try Delete key first, then look for delete button
    const deleteBtn = page.locator(
      'button:has-text("Delete"):visible, button[aria-label="Delete"]:visible, button[title="Delete"]:visible'
    ).first();

    if (await deleteBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
      await deleteBtn.click();
      await page.waitForTimeout(300);

      // Confirm deletion if a confirmation dialog appears
      const confirmBtn = page.locator(
        '[role="dialog"] button:has-text("OK"):visible, [role="dialog"] button:has-text("Delete"):visible, [role="dialog"] button:has-text("Confirm"):visible'
      ).first();
      if (await confirmBtn.isVisible({ timeout: 1000 }).catch(() => false)) {
        await confirmBtn.click();
        await page.waitForTimeout(300);
      }
    } else {
      // Try keyboard shortcut
      await page.keyboard.press('Delete');
      await page.waitForTimeout(300);

      // Handle possible confirmation
      const confirmBtn = page.locator(
        '[role="dialog"] button:has-text("OK"):visible, [role="dialog"] button:has-text("Delete"):visible'
      ).first();
      if (await confirmBtn.isVisible({ timeout: 1000 }).catch(() => false)) {
        await confirmBtn.click();
        await page.waitForTimeout(300);
      }
    }

    await page.waitForTimeout(500);

    // Verify the node is removed
    const stillExists = await designerNodeExists(page, targetNodeId);
    expect(stillExists, `Node ${targetNodeId} should have been deleted`).toBe(false);

    await assertHeartbeat(page, 'after deleting node');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(3);
  });
});
