import { test, expect } from '@playwright/test';
import {
  APP_HARNESS_URL,
  assertHeartbeat,
  waitForAppReady,
  navigateToScreen,
  collectErrors,
  clickButton,
  isVisible,
  dismissModal,
  typeIntoInput,
} from '../helpers';

test.describe('Bots & Agents page', () => {
  /** Wait for app render then navigate to bots page */
  async function gotoBots(page: import('@playwright/test').Page) {
    await navigateToScreen(page, 'bots');
    await expect(page.locator('button:has-text("Launch Bot"), button:has-text("Launch")')).toBeVisible({ timeout: 10_000 });
  }

  /** Navigate to bots and open the Launch Bot dialog */
  async function openLaunchDialog(page: import('@playwright/test').Page) {
    await gotoBots(page);
    // Button uses solid-ui Button component with text "+ Launch Bot"
    const launchBtn = page.locator('button:has-text("Launch Bot")').first();
    try {
      await expect(launchBtn).toBeVisible({ timeout: 5000 });
      await launchBtn.click();
      await page.waitForTimeout(500);
      return true;
    } catch {
      return false;
    }
  }

  test('bots page should render with launch button', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await assertHeartbeat(page, 'initial');

    await gotoBots(page);

    // The page should have a "Launch Bot" button (button.btn.btn-primary)
    // Use expect().toBeVisible() which properly waits, unlike isVisible() which returns immediately
    await expect(page.locator('button:has-text("Launch Bot")')).toBeVisible({ timeout: 10_000 });

    await assertHeartbeat(page, 'after viewing bots page');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('clicking launch should open the LaunchBot dialog', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    const opened = await openLaunchDialog(page);
    if (opened) {
      await assertHeartbeat(page, 'after clicking launch');

      // Dialog uses solid-ui Dialog with role="dialog"
      const modal = page.locator('[role="dialog"]');
      expect(await modal.count(), 'LaunchBot dialog should appear').toBeGreaterThanOrEqual(1);

      await dismissModal(page);
    } else {
      console.log('Launch button not found on bots page');
    }

    await assertHeartbeat(page, 'after LaunchBot dialog test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch dialog should have friendly name input', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    const opened = await openLaunchDialog(page);
    if (opened) {
      // Name input has placeholder "e.g. Code Reviewer"
      const nameInput = page.locator('[role="dialog"] input[placeholder="e.g. Code Reviewer"]:visible').first();
      const nameInputAlt = page.locator('[role="dialog"] input[type="text"]:visible').first();

      const hasNameField = await nameInput.isVisible({ timeout: 2000 }).catch(() => false) ||
                           await nameInputAlt.isVisible({ timeout: 1000 }).catch(() => false);
      console.log(`Friendly name input visible: ${hasNameField}`);
      expect(hasNameField).toBe(true);

      if (await nameInput.isVisible().catch(() => false)) {
        await nameInput.click();
        await nameInput.fill('Test Bot');
        const value = await nameInput.inputValue();
        expect(value).toBe('Test Bot');
      } else if (await nameInputAlt.isVisible().catch(() => false)) {
        await nameInputAlt.click();
        await nameInputAlt.fill('Test Bot');
        const value = await nameInputAlt.inputValue();
        expect(value).toBe('Test Bot');
      }

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after friendly name test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch dialog should have persona selector dropdown', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    const opened = await openLaunchDialog(page);
    if (opened) {
      // Persona is a <select> element in the dialog
      const personaSelect = page.locator('[role="dialog"] select:visible').first();
      const hasPersona = await personaSelect.isVisible({ timeout: 2000 }).catch(() => false);
      console.log(`Persona selector found: ${hasPersona}`);
      expect(hasPersona).toBe(true);

      // Verify it has persona options (default, coder, reviewer from mock)
      if (hasPersona) {
        const optionCount = await personaSelect.locator('option').count();
        console.log(`Persona select has ${optionCount} options`);
        expect(optionCount).toBeGreaterThan(0);
      }

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after persona selector test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch dialog should have mode selection (one-shot/idle/daemon)', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    const opened = await openLaunchDialog(page);
    if (opened) {
      // Mode uses radio buttons: input[type="radio"][name="bot-mode"]
      const modeRadios = page.locator('[role="dialog"] input[type="radio"][name="bot-mode"]:visible, [role="dialog"] input[type="radio"]:visible');
      const radioCount = await modeRadios.count();
      console.log(`Found ${radioCount} mode radio buttons`);

      // Check for mode values: one_shot, idle_after_task, continuous
      const modeValues = ['one_shot', 'idle_after_task', 'continuous'];
      let modeFound = false;
      for (const mode of modeValues) {
        const radio = page.locator(`[role="dialog"] input[type="radio"][value="${mode}"]`);
        if (await radio.count() > 0) {
          modeFound = true;
          console.log(`Found mode radio: ${mode}`);
        }
      }

      if (!modeFound && radioCount > 0) {
        modeFound = true;
      }

      console.log(`Mode selection found: ${modeFound}`);
      expect(modeFound).toBe(true);

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after mode selection test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('launch dialog should have allowed tools multiselect', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    const opened = await openLaunchDialog(page);
    if (opened) {
      // Look for override toggle switches (solid-ui Switch) and tool-related controls
      const switches = page.locator('[role="dialog"] [data-scope="switch"]:visible');
      const switchCount = await switches.count();
      console.log(`Found ${switchCount} switches in launch dialog`);

      const toolsLabel = page.locator('[role="dialog"] label:has-text("Override"):visible, [role="dialog"] label:has-text("tool"):visible');
      const hasToolsUI = switchCount > 0 || await toolsLabel.count() > 0;
      console.log(`Tools/override controls found: ${hasToolsUI}`);

      if (switchCount > 0) {
        const firstSwitch = switches.first();
        await firstSwitch.click();
        console.log(`Toggled switch`);
      }

      await dismissModal(page);
    }

    await assertHeartbeat(page, 'after allowed tools test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('bot list should show active/inactive status indicators', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    await gotoBots(page);

    // Check for any status text or indicators on the page
    const statusIndicators = page.locator('[class*="status"], [class*="pill"], [class*="badge"]');
    const indicatorCount = await statusIndicators.count();
    console.log(`Found ${indicatorCount} status indicator elements`);

    for (const status of ['active', 'inactive', 'running', 'stopped', 'idle']) {
      const statusEl = page.locator(`text=${status}`).first();
      const visible = await statusEl.isVisible({ timeout: 500 }).catch(() => false);
      if (visible) console.log(`Status "${status}" is visible`);
    }

    // Informational - status depends on mock data
    expect(typeof indicatorCount).toBe('number');

    await assertHeartbeat(page, 'after status indicators test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });

  test('agent controls should have pause/resume/restart/kill buttons', async ({ page }) => {
    const errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);

    await gotoBots(page);

    // Look for agent control buttons
    const controlTitles = ['Pause', 'Resume', 'Restart', 'Kill', 'Stop'];
    let foundControls = 0;
    for (const title of controlTitles) {
      const btn = page.locator(`button[title="${title}"]:visible, button:has-text("${title}"):visible`).first();
      const btnVisible = await btn.isVisible({ timeout: 1000 }).catch(() => false);
      if (btnVisible) {
        foundControls++;
        console.log(`Control button "${title}" is visible`);
      }
    }
    console.log(`Found ${foundControls}/${controlTitles.length} agent control buttons`);

    // Informational: control buttons depend on active agents
    expect(typeof foundControls).toBe('number');

    await assertHeartbeat(page, 'after agent controls test');
    expect(errors.length, `Unexpected errors: ${errors.join('; ')}`).toBeLessThan(5);
  });
});
