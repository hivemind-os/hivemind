/**
 * LCARS Theme Visual Audit
 *
 * Systematically visits every page, dialog, tab, and overlay in the app
 * with the LCARS theme active. Each test takes a screenshot so we can
 * review colour / contrast issues across the full UI surface.
 */
import { test, expect, Page } from '@playwright/test';
import {
  APP_HARNESS_URL,
  waitForAppReady,
  selectFirstSession,
  navigateToScreen,
  openSettings,
  openFlightDeck,
  switchChatTab,
  collectErrors,
} from '../helpers';

// ── Helpers ────────────────────────────────────────────────────────

/** Apply the LCARS theme before the app reads localStorage */
async function applyLcarsTheme(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem('hivemind-theme', 'lcars');
  });
}

/** Take a named screenshot with generous diff tolerance for first-run baseline */
async function snap(page: Page, name: string) {
  await page.waitForTimeout(400);
  await expect(page).toHaveScreenshot(`lcars-${name}.png`, {
    maxDiffPixelRatio: 0.05,
    timeout: 10_000,
  });
}

/** Expand a settings category and click a sub-tab by its data-testid id */
async function clickSettingsTab(page: Page, tabId: string, categoryId?: string) {
  if (categoryId) {
    const cat = page.locator(`[data-testid="settings-category-${categoryId}"]`);
    if (await cat.isVisible({ timeout: 3000 }).catch(() => false)) {
      await cat.click();
      await page.waitForTimeout(200);
    }
  }
  const tab = page.locator(`[data-testid="settings-tab-${tabId}"]`);
  if (await tab.isVisible({ timeout: 3000 }).catch(() => false)) {
    await tab.click();
    await page.waitForTimeout(400);
  }
}

/** Click a FlightDeck tab by its data-testid id */
async function clickFDTab(page: Page, tabId: string) {
  const tab = page.locator(`[data-testid="fd-tab-${tabId}"]`);
  if (await tab.isVisible({ timeout: 3000 }).catch(() => false)) {
    await tab.click();
    await page.waitForTimeout(400);
  }
}

// ── Tests ──────────────────────────────────────────────────────────

test.describe('LCARS Theme — Full UI Audit', () => {
  test.beforeEach(async ({ page }) => {
    collectErrors(page);
    await applyLcarsTheme(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
  });

  // ── Top-level screens ──────────────────────────────────────────

  test('Empty state / session list', async ({ page }) => {
    await snap(page, 'empty-state');
  });

  test('Session loaded — chat tab', async ({ page }) => {
    await selectFirstSession(page);
    await page.waitForTimeout(600);
    await snap(page, 'session-chat');
  });

  test('Session — workspace tab', async ({ page }) => {
    await selectFirstSession(page);
    await switchChatTab(page, 'workspace');
    await snap(page, 'session-workspace');
  });

  test('Session — stage tab', async ({ page }) => {
    await selectFirstSession(page);
    await switchChatTab(page, 'stage');
    await snap(page, 'session-stage');
  });

  test('Session — knowledge tab', async ({ page }) => {
    await selectFirstSession(page);
    await switchChatTab(page, 'knowledge');
    await snap(page, 'session-knowledge');
  });

  test('Bots page', async ({ page }) => {
    await navigateToScreen(page, 'bots');
    await snap(page, 'bots-page');
  });

  test('Scheduler page', async ({ page }) => {
    await navigateToScreen(page, 'scheduler');
    await snap(page, 'scheduler-page');
  });

  test('Workflows page', async ({ page }) => {
    await navigateToScreen(page, 'workflows');
    await snap(page, 'workflows-page');
  });

  test('Collapsed sidebar', async ({ page }) => {
    const collapseBtn = page.locator('[aria-label="Collapse sidebar"]');
    if (await collapseBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await collapseBtn.click();
      await page.waitForTimeout(400);
    }
    await snap(page, 'sidebar-collapsed');
  });

  // ── Settings modal — every tab ─────────────────────────────────

  const settingsTabs = [
    { id: 'general-appearance', category: 'general' },
    { id: 'general-daemon', category: 'general' },
    { id: 'general-recording', category: 'general' },
    { id: 'providers', category: 'ai-models' },
    { id: 'local-models', category: 'ai-models' },
    { id: 'downloads', category: 'ai-models' },
    { id: 'compaction', category: 'ai-models' },
    { id: 'mcp', category: 'extensions' },
    { id: 'skills', category: 'extensions' },
    { id: 'tools', category: 'extensions' },
    { id: 'python', category: 'extensions' },
    { id: 'security', category: 'security' },
    { id: 'comm-audit', category: 'security' },
    { id: 'personas', category: 'agents-automation' },
    { id: 'channels', category: 'agents-automation' },
    { id: 'afk', category: 'agents-automation' },
    { id: 'scheduler', category: 'agents-automation' },
  ];

  for (const tab of settingsTabs) {
    test(`Settings — ${tab.id}`, async ({ page }) => {
      await openSettings(page);
      await clickSettingsTab(page, tab.id, tab.category);
      await snap(page, `settings-${tab.id}`);
    });
  }

  // ── FlightDeck — every tab ─────────────────────────────────────

  const fdTabs = [
    'agents',
    'workflows',
    'triggers',
    'mcp',
    'sessions',
    'models',
    'events',
    'services',
    'health',
  ];

  for (const tab of fdTabs) {
    test(`FlightDeck — ${tab}`, async ({ page }) => {
      await selectFirstSession(page);
      await openFlightDeck(page);
      await clickFDTab(page, tab);
      await snap(page, `fd-${tab}`);
    });
  }

  // ── Dialogs & overlays ─────────────────────────────────────────

  test('Launch Bot dialog', async ({ page }) => {
    await navigateToScreen(page, 'bots');
    const launchBtn = page.locator('button:has-text("Launch"), button:has-text("New Bot")').first();
    if (await launchBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await launchBtn.click();
      await page.waitForTimeout(500);
      await snap(page, 'launch-bot-dialog');
    }
  });

  test('Inspector modal', async ({ page }) => {
    const inspectorBtn = page.locator('[data-testid="sidebar-inspector-btn"], [aria-label="Inspector"]').first();
    if (await inspectorBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await inspectorBtn.click();
      await page.waitForTimeout(500);
      await snap(page, 'inspector-modal');
    }
  });

  test('Session config dialog', async ({ page }) => {
    await selectFirstSession(page);
    const configBtn = page.locator('[data-testid="session-config-btn"], [aria-label="Session config"], button:has-text("Configure")').first();
    if (await configBtn.isVisible({ timeout: 3000 }).catch(() => false)) {
      await configBtn.click();
      await page.waitForTimeout(500);
      await snap(page, 'session-config-dialog');
    }
  });
});
