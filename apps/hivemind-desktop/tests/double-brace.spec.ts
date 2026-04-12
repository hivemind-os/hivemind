import { test, expect, Page } from '@playwright/test';

const HARNESS_URL = '/tests/harness.html';

async function assertHeartbeat(page: Page, label: string) {
  const ts1 = await page.locator('#heartbeat').getAttribute('data-ts');
  await page.waitForTimeout(1500);
  const ts2 = await page.locator('#heartbeat').getAttribute('data-ts');
  expect(Number(ts2), `Heartbeat stale at "${label}" — app froze`).toBeGreaterThan(Number(ts1));
}

async function addNodeFromPalette(page: Page, label: string) {
  await page.locator(`div[title*="${label}"]`).click();
  await page.waitForTimeout(200);
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

test('typing {{ in feedback gate prompt should not freeze', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', err => errors.push(err.message));

  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  await assertHeartbeat(page, 'initial');

  // Add a feedback gate node
  await addNodeFromPalette(page, 'Feedback Gate');
  await page.waitForTimeout(300);

  // Open the "Edit Inputs" dialog for the newly added node
  const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
  await editBtn.click({ timeout: 5000 });
  await page.waitForTimeout(500);

  // Find the textarea inside the dialog (feedback gate has a Prompt textarea)
  const textareas = page.locator('[role="dialog"] textarea:visible:not([disabled])');
  const count = await textareas.count();
  console.log(`Found ${count} visible textareas in dialog`);
  
  let targetTextarea = textareas.first();
  
  // Type "hello " then {{ character by character
  console.log('Typing "hello " ...');
  await targetTextarea.click();
  await targetTextarea.fill('hello ');
  await assertHeartbeat(page, 'after "hello "');
  
  console.log('Typing first { ...');
  await targetTextarea.press('{');
  await assertHeartbeat(page, 'after "hello {"');
  
  console.log('Typing second { ...');
  await targetTextarea.press('{');
  await assertHeartbeat(page, 'after "hello {{"');
  
  // Keep typing to see if it's still alive
  console.log('Typing more text after {{ ...');
  await targetTextarea.pressSequentially('var_name}}', { delay: 50 });
  await assertHeartbeat(page, 'after "hello {{var_name}}"');
  
  // Click OK to apply
  const okBtn = page.locator('[role="dialog"] button:has-text("OK"):visible').first();
  await okBtn.click();
  await page.waitForTimeout(300);
  
  // Wait 10 seconds to see if delayed freeze kicks in
  console.log('Waiting 10 seconds for delayed freeze...');
  for (let i = 0; i < 5; i++) {
    await page.waitForTimeout(2000);
    await assertHeartbeat(page, `post-{{ idle check ${i+1}`);
  }
  
  // Can we still interact?
  await clickNode(page, 'read_step');
  await assertHeartbeat(page, 'post-{{ interaction');
  
  console.log('Page errors:', errors);
  expect(errors.length).toBeLessThan(3);
});

test('typing various {{ patterns should not crash', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', err => errors.push(err.message));
  
  await page.goto(HARNESS_URL);
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
  
  // Add feedback gate
  await addNodeFromPalette(page, 'Feedback Gate');
  await page.waitForTimeout(500);

  // Open the "Edit Inputs" dialog
  const editBtn = page.locator('button:has-text("Edit Inputs"):visible').first();
  await editBtn.click({ timeout: 5000 });
  await page.waitForTimeout(500);
  
  const textarea = page.locator('[role="dialog"] textarea:visible:not([disabled])').first();
  
  // Test various problematic patterns
  const patterns = [
    'hello {{',           // unclosed braces
    '{{}}',               // empty expression
    '{{ {{',              // nested opens
    '{{{',                // triple brace
    'a{b{c{d',           // scattered braces
    '}}hello{{',          // reversed
    'test {{var}} end',   // valid expression
    '{{a.b.c.d.e}}',     // deep path
  ];
  
  for (const pattern of patterns) {
    console.log(`Testing pattern: "${pattern}"`);
    await textarea.fill(pattern);
    await page.waitForTimeout(200);
    await assertHeartbeat(page, `pattern: ${pattern}`);
  }
  
  console.log('All patterns survived. Page errors:', errors);
});
