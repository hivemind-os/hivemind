/**
 * Shared helpers for Playwright integration tests.
 */
import { type Page, expect } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

const DAEMON_CONFIG_PATH = path.join(os.tmpdir(), 'hive-test-daemon.json');
const APP_HARNESS_URL = '/tests/integration-harness.html';

export interface DaemonConfig {
  baseUrl: string;
  authToken: string;
  pid: number;
}

/** Read daemon connection info written by global-setup. */
export function loadDaemonConfig(): DaemonConfig {
  return JSON.parse(fs.readFileSync(DAEMON_CONFIG_PATH, 'utf8'));
}

/** Navigate to the integration harness with daemon URL injected. */
export async function navigateToIntegrationApp(page: Page, config?: DaemonConfig) {
  const { baseUrl, authToken } = config || loadDaemonConfig();
  const url = `${APP_HARNESS_URL}?daemon_url=${encodeURIComponent(baseUrl)}&authToken=${encodeURIComponent(authToken)}`;
  await page.goto(url);
}

/** Wait for the app to finish initializing. */
export async function waitForAppReady(page: Page, timeout = 30_000) {
  // Wait for the initializing overlay to disappear (or never appear)
  try {
    await page.waitForSelector('.initializing-overlay', { state: 'detached', timeout });
  } catch {
    // Overlay may never have been visible (e.g., if init was fast); that's fine
  }
  // Wait for heartbeat element (added by the harness itself)
  await page.waitForSelector('#heartbeat', { timeout: 10_000 });
}

/** Wait for a chat message matching the given text to appear. */
export async function waitForChatMessage(page: Page, textMatcher: string | RegExp, timeout = 30_000) {
  const selector = '.message-list .message-card';
  return page.waitForFunction(
    ({ sel, matcher }) => {
      const cards = document.querySelectorAll(sel);
      for (const card of cards) {
        const text = card.textContent || '';
        if (typeof matcher === 'string' ? text.includes(matcher) : new RegExp(matcher).test(text)) {
          return true;
        }
      }
      return false;
    },
    { sel: selector, matcher: textMatcher instanceof RegExp ? textMatcher.source : textMatcher },
    { timeout },
  );
}

/** Wait for an inline question to appear in the chat. */
export async function waitForInlineQuestion(page: Page, textMatcher: string, timeout = 30_000) {
  // Questions typically appear as special cards in the message list
  return page.waitForFunction(
    (matcher) => {
      const el = document.querySelector('[data-testid="inline-question"], .inline-question');
      if (el && el.textContent?.includes(matcher)) return true;
      // Also check for question text anywhere in the page
      return document.body.textContent?.includes(matcher) ?? false;
    },
    textMatcher,
    { timeout },
  );
}

/** Submit an answer to a freeform question. */
export async function answerFreeformQuestion(page: Page, answer: string) {
  const input = page.locator('input[placeholder*="answer"], textarea[placeholder*="answer"]').first();
  await input.fill(answer);
  await input.press('Enter');
}

/** Click a choice button in an inline question. */
export async function answerQuestionChoice(page: Page, choice: string) {
  await page.getByRole('button', { name: choice }).click();
}

/** Open the Flight Deck panel. */
export async function openFlightDeck(page: Page) {
  await page.click('[data-testid="flight-deck-toggle"]');
  await page.waitForSelector('[data-testid="flight-deck-overlay"]', { state: 'visible' });
}

/** Close the Flight Deck panel. */
export async function closeFlightDeck(page: Page) {
  await page.click('[data-testid="flight-deck-close"]');
  await page.waitForSelector('[data-testid="flight-deck-overlay"]', { state: 'detached' });
}

/** Check the badge count on a Flight Deck tab. */
export async function assertFlightDeckBadge(page: Page, tabName: string, expectedCount: number) {
  const tab = page.locator(`[data-testid="fd-tab-${tabName}"]`);
  if (expectedCount === 0) {
    await expect(tab.locator('.badge, [data-testid="badge"]')).not.toBeVisible();
  } else {
    await expect(tab.locator('.badge, [data-testid="badge"]')).toHaveText(String(expectedCount));
  }
}

/** Wait for a workflow card to appear in the chat timeline. */
export async function waitForWorkflowCard(page: Page, timeout = 30_000) {
  return page.waitForSelector('.wf-timeline, [data-testid="workflow-card"]', { timeout });
}

/** Wait for a feedback gate prompt in the chat. */
export async function waitForFeedbackGate(page: Page, textMatcher: string, timeout = 30_000) {
  return page.waitForFunction(
    (matcher) => document.body.textContent?.includes(matcher) ?? false,
    textMatcher,
    { timeout },
  );
}

/** Wait for a tool approval dialog. */
export async function waitForToolApproval(page: Page, toolName?: string, timeout = 30_000) {
  const selector = '[data-testid="tool-approval-dialog"]';
  await page.waitForSelector(selector, { timeout });
  if (toolName) {
    await expect(page.locator(selector)).toContainText(toolName);
  }
}

/** Approve a pending tool call. */
export async function approveToolCall(page: Page) {
  await page.click('[data-testid="tool-approval-dialog"] button:has-text("Approve")');
}

/** Make a direct HTTP call to the daemon API. */
export async function queryDaemonApi(endpoint: string, config?: DaemonConfig): Promise<unknown> {
  const { baseUrl, authToken } = config || loadDaemonConfig();
  const resp = await fetch(`${baseUrl}${endpoint}`, {
    headers: { Authorization: `Bearer ${authToken}` },
  });
  return resp.json();
}

/** POST to the daemon API with a JSON body. */
export async function postDaemonApi(endpoint: string, body: unknown = {}, config?: DaemonConfig): Promise<Response> {
  const { baseUrl, authToken } = config || loadDaemonConfig();
  return fetch(`${baseUrl}${endpoint}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${authToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
}

/** Create a chat session and return the session snapshot. */
export async function createSession(config?: DaemonConfig): Promise<{ id: string }> {
  const resp = await postDaemonApi('/api/v1/chat/sessions', {}, config);
  if (!resp.ok) throw new Error(`Failed to create session: ${resp.status} ${await resp.text()}`);
  return resp.json() as Promise<{ id: string }>;
}

/** Save a workflow definition via the daemon API. */
export async function saveWorkflowDefinition(yaml: string, config?: DaemonConfig) {
  const { baseUrl, authToken } = config || loadDaemonConfig();
  const resp = await fetch(`${baseUrl}/api/v1/workflows/definitions`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${authToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ yaml }),
  });
  if (!resp.ok) throw new Error(`Failed to save workflow: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

/** Launch a workflow via the daemon API. */
export async function launchWorkflow(
  definition: string,
  parentSessionId: string,
  config?: DaemonConfig,
) {
  const { baseUrl, authToken } = config || loadDaemonConfig();
  const resp = await fetch(`${baseUrl}/api/v1/workflows/instances`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${authToken}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      definition,
      parent_session_id: parentSessionId,
      inputs: {},
    }),
  });
  if (!resp.ok) throw new Error(`Failed to launch workflow: ${resp.status} ${await resp.text()}`);
  return resp.json();
}
