/**
 * Scenario 3: Email trigger → Workflow → Agent responds.
 *
 * This scenario is primarily backend-driven (Tier 1) since email triggers don't
 * originate from the UI. The Playwright test verifies that:
 * - A workflow triggered by an email event appears in the workflow instances API
 * - The workflow completes successfully
 */
import { test, expect } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import {
  loadDaemonConfig,
  navigateToIntegrationApp,
  waitForAppReady,
  saveWorkflowDefinition,
  postDaemonApi,
  queryDaemonApi,
} from '../integration/helpers';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const SCENARIO3_YAML = fs.readFileSync(
  path.resolve(__dirname, '../../../../tests/fixtures/workflows/scenario3-email-responder.yaml'),
  'utf8',
);

test.describe('Scenario 3: Email trigger workflow', () => {
  test.beforeEach(async ({ page }) => {
    const config = loadDaemonConfig();
    await saveWorkflowDefinition(SCENARIO3_YAML, config);
    await navigateToIntegrationApp(page, config);
    await waitForAppReady(page);
  });

  test('email-triggered workflow appears and completes', async ({ page }) => {
    const config = loadDaemonConfig();

    // Trigger the email event via daemon API
    const eventResp = await postDaemonApi('/api/v1/events/publish', {
      topic: 'comm.message.received.test-email',
      payload: {
        from: 'client@example.com',
        subject: 'Pricing inquiry',
        body: 'What are your current prices?',
        channel_id: 'test-email',
        provider: 'test-email',
        timestamp: new Date().toISOString(),
      },
    }, config);
    // The events/publish endpoint might not exist — fall back to polling
    if (!eventResp.ok) {
      console.warn('Events publish endpoint not available; scenario depends on trigger manager');
    }

    // Wait for a workflow instance to appear
    let instance: any = null;
    for (let i = 0; i < 60; i++) {
      const body = await queryDaemonApi('/api/v1/workflows/instances', config) as { items: any[] };
      instance = body.items?.find((inst: any) =>
        inst.definition_name?.includes('scenario3') || inst.definition?.includes('scenario3'),
      );
      if (instance) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(instance).toBeTruthy();

    // Wait for workflow to complete
    let completed = false;
    for (let i = 0; i < 60; i++) {
      const inst = await queryDaemonApi(
        `/api/v1/workflows/instances/${instance.id}`, config,
      ) as { status: string };
      if (inst.status === 'completed') {
        completed = true;
        break;
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(completed).toBeTruthy();
  });
});
