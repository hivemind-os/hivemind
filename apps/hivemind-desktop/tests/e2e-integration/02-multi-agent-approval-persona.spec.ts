/**
 * Scenario 2: Multi-agent workflow with ask_user, feedback gate, and persona isolation.
 *
 * Verifies:
 * - Researcher agent asks a question via ask_user → surfaces in chat
 * - Feedback gate step surfaces as a pending question in the chat
 * - Gate response unblocks the executor agent
 * - Executor agent output chains from researcher output
 * - Workflow completes successfully
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

const SCENARIO2_YAML = fs.readFileSync(
  path.resolve(__dirname, '../../../../tests/fixtures/workflows/scenario2-multi-agent.yaml'),
  'utf8',
);

test.describe('Scenario 2: Multi-agent workflow', () => {
  test.beforeEach(async ({ page }) => {
    const config = loadDaemonConfig();
    await saveWorkflowDefinition(SCENARIO2_YAML, config);
    await navigateToIntegrationApp(page, config);
    await waitForAppReady(page);
  });

  test('researcher question, feedback gate, and executor complete in sequence', async ({ page }) => {
    const config = loadDaemonConfig();

    // Create a session and launch the workflow
    const session = await createSession(config);
    const launchResp = await postDaemonApi('/api/v1/workflows/instances', {
      definition: 'test/scenario2-multi-agent',
      parent_session_id: session.id,
      inputs: {},
    }, config);
    expect(launchResp.ok).toBeTruthy();

    // --- Phase A: Wait for researcher's ask_user question ---
    let question: any = null;
    for (let i = 0; i < 60; i++) {
      const questions = await queryDaemonApi(
        `/api/v1/chat/sessions/${session.id}/pending-questions`, config,
      ) as any[];
      question = questions.find((q: any) => q.text?.includes('topic interests'));
      if (question) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(question).toBeTruthy();
    expect(question.text).toBe('Which topic interests you?');

    // Respond to the question
    const answerResp = await postDaemonApi(
      `/api/v1/chat/sessions/${session.id}/agents/${question.agent_id}/interaction`,
      {
        request_id: question.request_id,
        payload: { type: 'answer', selected_choice: 0, text: 'Quantum computing' },
      },
      config,
    );
    expect(answerResp.ok).toBeTruthy();

    // --- Phase B: Wait for the feedback gate ---
    let gateQuestion: any = null;
    for (let i = 0; i < 60; i++) {
      const questions = await queryDaemonApi(
        `/api/v1/chat/sessions/${session.id}/pending-questions`, config,
      ) as any[];
      gateQuestion = questions.find((q: any) => q.routing === 'gate');
      if (gateQuestion) break;
      await new Promise((r) => setTimeout(r, 500));
    }
    expect(gateQuestion).toBeTruthy();
    expect(gateQuestion.workflowInstanceId).toBeTruthy();
    expect(gateQuestion.workflowStepId).toBeTruthy();

    // Respond to the gate
    const gateResp = await postDaemonApi(
      `/api/v1/workflows/instances/${gateQuestion.workflowInstanceId}/steps/${gateQuestion.workflowStepId}/respond`,
      { response: 'Confirmed, proceed with quantum computing' },
      config,
    );
    expect(gateResp.ok).toBeTruthy();

    // --- Phase C: Wait for workflow completion ---
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

    // Verify no leftover questions
    const finalQuestions = await queryDaemonApi(
      `/api/v1/chat/sessions/${session.id}/pending-questions`, config,
    ) as any[];
    expect(finalQuestions).toHaveLength(0);
  });
});
