/**
 * Shared test helpers for E2E Playwright tests.
 */
import { Page, expect } from '@playwright/test';

export const APP_HARNESS_URL = '/tests/app-harness.html';
export const DESIGNER_HARNESS_URL = '/tests/harness.html';

/** Assert the heartbeat element timestamp is advancing (app is not frozen) */
export async function assertHeartbeat(page: Page, label: string, timeoutMs = 3000) {
  const hb = page.locator('#heartbeat');
  const ts1 = await hb.getAttribute('data-ts');
  await page.waitForTimeout(1500);
  const ts2 = await hb.getAttribute('data-ts');
  expect(Number(ts2), `Heartbeat stale at "${label}" — app froze`).toBeGreaterThan(Number(ts1));
}

/** Wait for the app to finish initializing */
export async function waitForAppReady(page: Page, timeoutMs = 15_000) {
  // Wait for the initializing overlay to disappear
  await page.waitForFunction(
    () => !document.querySelector('.initializing-overlay'),
    { timeout: timeoutMs }
  );
  // Wait for heartbeat to appear
  await page.waitForSelector('#heartbeat', { timeout: 5_000 });
}

/** Navigate to a screen via sidebar clicks */
export async function navigateToScreen(page: Page, screen: 'session' | 'bots' | 'scheduler' | 'workflows' | 'settings') {
  if (screen === 'settings') {
    return openSettings(page);
  }
  const textMap: Record<string, string> = {
    session: 'Sessions',
    bots: 'Bots',
    scheduler: 'Scheduler',
    workflows: 'Workflows',
  };
  const btn = page.locator(`button:has-text("${textMap[screen]}")`).first();
  await btn.waitFor({ state: 'visible', timeout: 10_000 });
  await btn.click();
  await page.waitForTimeout(500);
}

/** Click the settings button in the sidebar */
export async function openSettings(page: Page) {
  const btn = page.locator('[data-testid="sidebar-settings-btn"], [aria-label="Settings"]').first();
  await btn.waitFor({ state: 'visible', timeout: 30_000 });
  await btn.click();
  await page.waitForTimeout(500);
}

/** Click a tab in the chat panel */
export async function switchChatTab(page: Page, tab: 'chat' | 'workspace' | 'stage' | 'knowledge') {
  const tabBtn = page.locator(`[data-testid="tab-${tab}"]`).first();
  if (await tabBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await tabBtn.click();
    await page.waitForTimeout(300);
  }
}

/** Select the first session in the sidebar */
export async function selectFirstSession(page: Page) {
  const session = page.locator('[data-testid^="session-item-"]').first();
  if (await session.isVisible({ timeout: 3000 }).catch(() => false)) {
    await session.click();
    await page.waitForTimeout(500);
  }
}

/** Open the Flight Deck */
export async function openFlightDeck(page: Page) {
  const btn = page.locator('[data-testid="flight-deck-toggle"]').first();
  if (await btn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await btn.click();
    await page.waitForTimeout(500);
  }
}

/** Collect page errors during a test */
export function collectErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (err) => errors.push(`PAGE: ${err.message}`));
  page.on('console', (msg) => {
    if (msg.type() === 'error' && !msg.text().includes('favicon') && !msg.text().includes('tauri-mock')) {
      errors.push(`CONSOLE: ${msg.text()}`);
    }
  });
  return errors;
}

/** Type text into a visible input/textarea */
export async function typeIntoInput(page: Page, selector: string, text: string) {
  const input = page.locator(selector).first();
  await input.click();
  await input.fill(text);
  await page.waitForTimeout(100);
}

/** Click a button by its text content */
export async function clickButton(page: Page, text: string) {
  const btn = page.locator(`button:has-text("${text}"):visible`).first();
  if (await btn.isVisible({ timeout: 3000 }).catch(() => false)) {
    await btn.click();
    await page.waitForTimeout(300);
  }
}

/** Check if an element is visible */
export async function isVisible(page: Page, selector: string): Promise<boolean> {
  return page.locator(selector).first().isVisible({ timeout: 2000 }).catch(() => false);
}

/** Dismiss a modal by clicking its close/cancel button */
export async function dismissModal(page: Page) {
  const closeBtn = page.locator('[role="dialog"] button:has-text("Cancel"):visible, [role="dialog"] button:has-text("Close"):visible').first();
  if (await closeBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await closeBtn.click();
    await page.waitForTimeout(300);
  }
}

/** Click a node on the workflow designer canvas */
export async function clickDesignerNode(page: Page, nodeId: string) {
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

/** Add a node from the designer palette */
export async function addDesignerNode(page: Page, subtypeLabel: string) {
  const btn = page.locator(`div[title*="${subtypeLabel}"]`).first();
  if (await btn.isVisible({ timeout: 5000 }).catch(() => false)) {
    await btn.click();
    await page.waitForTimeout(800);
  }
}

/** Check if a designer node exists */
export async function designerNodeExists(page: Page, nodeId: string): Promise<boolean> {
  return (await page.locator(`[data-testid="node-list"] [data-nodeid="${nodeId}"]`).count()) > 0;
}
