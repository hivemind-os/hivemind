import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  waitForAppReady,
  openSettings,
  collectErrors,
} from '../helpers';

/**
 * Focused test: verify each settings tab click shows distinct content
 * and the previously active tab's content disappears.
 */

const TABS = [
  { value: 'general-appearance', label: 'Appearance',    category: 'general',           uniqueText: 'Appearance' },
  { value: 'general-daemon',     label: 'Daemon',        category: 'general',           uniqueText: 'Daemon Status' },
  { value: 'general-recording',  label: 'Event Recording', category: 'general',         uniqueText: 'Event Recording' },
  { value: 'providers',          label: 'Providers',     category: 'ai-models',         uniqueText: 'Add Provider' },
  { value: 'local-models',       label: 'Local Models',  category: 'ai-models',         uniqueText: null },
  { value: 'downloads',          label: 'Downloads',     category: 'ai-models',         uniqueText: null },
  { value: 'compaction',         label: 'Compaction',    category: 'ai-models',         uniqueText: null },
  { value: 'mcp',                label: 'MCP Servers',   category: 'extensions',        uniqueText: 'Add Server' },
  { value: 'skills',             label: 'Skills',        category: 'extensions',        uniqueText: null },
  { value: 'tools',              label: 'Tools',         category: 'extensions',        uniqueText: null },
  { value: 'python',             label: 'Python',        category: 'extensions',        uniqueText: null },
  { value: 'security',           label: 'Policies',      category: 'security',          uniqueText: null },
  { value: 'comm-audit',         label: 'Audit Log',     category: 'security',          uniqueText: null },
  { value: 'personas',           label: 'Personas',      category: 'agents-automation', uniqueText: null },
  { value: 'channels',           label: 'Connectors',    category: 'agents-automation', uniqueText: null },
  { value: 'afk',                label: 'AFK / Status',  category: 'agents-automation', uniqueText: null },
  { value: 'scheduler',          label: 'Scheduler',     category: 'agents-automation', uniqueText: null },
];

test.describe('Settings Tab Switching', () => {
  test('clicking each tab should change the selected trigger and swap content', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    // Navigate to settings
    await openSettings(page);
    await page.locator('[data-testid="settings-modal"]').waitFor({ state: 'visible', timeout: 10_000 });

    // Appearance (first sub-tab of General) should be active by default
    const defaultTrigger = page.locator('[data-testid="settings-tab-general-appearance"]');
    await expect(defaultTrigger).toHaveAttribute('data-selected', '');

    // Content area
    const content = page.locator('.settings-content');
    await expect(content).toBeVisible();

    // Snapshot default content text
    const defaultContentText = await content.textContent();
    expect(defaultContentText).toContain('Appearance');

    // Click through every non-default tab and verify content changes
    for (const tab of TABS.filter(t => t.value !== 'general-appearance')) {
      // Expand the category if needed
      const categoryHeader = page.locator(`[data-testid="settings-category-${tab.category}"]`);
      await categoryHeader.click();
      await page.waitForTimeout(150);

      const trigger = page.locator(`[data-testid="settings-tab-${tab.value}"]`);
      await expect(trigger).toBeVisible({ timeout: 2_000 });
      await trigger.click();
      await page.waitForTimeout(300);

      // The clicked trigger should now have data-selected
      await expect(trigger).toHaveAttribute('data-selected', '', { timeout: 2_000 });

      // Default trigger should NOT have data-selected
      // (it may not be visible if its category is collapsed, so check if visible first)
      const defaultVisible = await defaultTrigger.isVisible().catch(() => false);
      if (defaultVisible) {
        const defSelected = await defaultTrigger.getAttribute('data-selected');
        expect(defSelected, `Appearance should be deselected when ${tab.label} is active`).toBeNull();
      }

      // Content should have changed (Theme select from Appearance tab should be gone)
      const themeSelect = page.locator('.settings-content select:has(option[value="dark"])');
      const themeVisible = await themeSelect.isVisible().catch(() => false);
      expect(themeVisible, `Theme select from Appearance tab should NOT be visible on ${tab.label} tab`).toBe(false);

      // If we know unique text, assert it
      if (tab.uniqueText) {
        const currentText = await content.textContent();
        expect(currentText, `${tab.label} tab should contain "${tab.uniqueText}"`).toContain(tab.uniqueText);
      }
    }

    // Click back to Appearance and verify it shows again
    const generalCategory = page.locator('[data-testid="settings-category-general"]');
    await generalCategory.click();
    await page.waitForTimeout(150);
    await defaultTrigger.click();
    await page.waitForTimeout(300);
    await expect(defaultTrigger).toHaveAttribute('data-selected', '');
    const restoredText = await content.textContent();
    expect(restoredText, 'Appearance tab content should reappear').toContain('Appearance');

    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  // Individual quick tests per tab to identify exactly which one fails
  for (const tab of TABS) {
    test(`${tab.label} tab should activate and show content`, async ({ page }) => {
      await page.goto(APP_HARNESS_URL);
      await waitForAppReady(page);
      await openSettings(page);
      await page.locator('[data-testid="settings-modal"]').waitFor({ state: 'visible', timeout: 10_000 });

      // Expand the category
      const categoryHeader = page.locator(`[data-testid="settings-category-${tab.category}"]`);
      await categoryHeader.click();
      await page.waitForTimeout(150);

      const trigger = page.locator(`[data-testid="settings-tab-${tab.value}"]`);
      await trigger.click();
      await page.waitForTimeout(300);

      // Trigger should have data-selected
      await expect(trigger).toHaveAttribute('data-selected', '', { timeout: 2_000 });

      // Content area should have content
      const content = page.locator('.settings-content');
      await expect(content).toBeVisible();
      const text = (await content.textContent()) ?? '';
      expect(text.length, `${tab.label} tab content should not be empty`).toBeGreaterThan(0);
    });
  }
});
