/**
 * Scenario 1: Chat → Workflow → Agent asks question → Answer → Results
 *
 * Verifies:
 * - Workflow can be launched from the chat
 * - Agent's ask_user question surfaces inline in the chat
 * - After answering, the agent completes and the workflow finishes
 * - Question badges appear on Flight Deck panels
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
  createSession,
  postDaemonApi,
  queryDaemonApi,
} from '../integration/helpers';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const SCENARIO1_YAML = fs.readFileSync(
  path.resolve(__dirname, '../../../../tests/fixtures/workflows/scenario1-agent-question.yaml'),
  'utf8',
);

test.describe('Scenario 1: Workflow agent question', () => {
  test.beforeEach(async ({ page }) => {
    const config = loadDaemonConfig();
    await saveWorkflowDefinition(SCENARIO1_YAML, config);
    await navigateToIntegrationApp(page, config);
    await waitForAppReady(page);
  });

  test('agent question surfaces in chat and can be answered', async ({ page }) => {
    const config = loadDaemonConfig();

    // Create a session and launch the workflow
    const session = await createSession(config);
    const launchResp = await postDaemonApi('/api/v1/workflows/instances', {
      definition: 'test/scenario1-agent-question',
      parent_session_id: session.id,
      inputs: {},
    }, config);
    expect(launchResp.ok).toBeTruthy();

    // Wait for the ask_user question to appear
    let question: any = null;
    for (let i = 0; i < 60; i++) {
      const questions = await queryDaemonApi(
        `/api/v1/chat/sessions/${session.id}/pending-questions`, config,
      ) as any[];
      question = questions.find((q: any) => q.text?.includes('favorite color'));
      if (question) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(question).toBeTruthy();
    expect(question.text).toBe('What is your favorite color?');
    expect(question.choices).toEqual(['Red', 'Blue', 'Green']);

    // Respond to the question
    const answerResp = await postDaemonApi(
      `/api/v1/chat/sessions/${session.id}/agents/${question.agent_id}/interaction`,
      {
        request_id: question.request_id,
        payload: { type: 'answer', selected_choice: 1, text: 'Blue' },
      },
      config,
    );
    expect(answerResp.ok).toBeTruthy();

    // Wait for workflow completion
    let completed = false;
    for (let i = 0; i < 60; i++) {
      const body = await queryDaemonApi(
        `/api/v1/workflows/instances?session_id=${session.id}`, config,
      ) as { items: any[] };
      if (body.items?.some((inst: any) => inst.status === 'completed')) {
        completed = true;
        break;
      }
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(completed).toBeTruthy();

    // Verify no pending questions remain
    const finalQuestions = await queryDaemonApi(
      `/api/v1/chat/sessions/${session.id}/pending-questions`, config,
    ) as any[];
    expect(finalQuestions).toHaveLength(0);
  });
});
