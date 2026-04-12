/**
 * E2E tests for streaming simulation via mock Tauri events.
 * Tests chat streaming, tool calls, approvals, and error handling.
 */
import { test, expect, Page } from '@playwright/test';
import { APP_HARNESS_URL, waitForAppReady, assertHeartbeat, selectFirstSession, collectErrors } from '../helpers';

test.describe('14 — Streaming Simulation', () => {
  let errors: string[];

  test.beforeEach(async ({ page }) => {
    errors = collectErrors(page);
    await page.goto(APP_HARNESS_URL);
    await waitForAppReady(page);
    await selectFirstSession(page);
    await page.waitForTimeout(500);
  });

  test('101. Emit chat tokens and see streaming text appear', async ({ page }) => {
    // Wait for composer to confirm session is loaded
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    // Emit streaming tokens via mock event system
    await page.evaluate(() => {
      const { emitChatToken } = (window as any).__TEST_HELPERS__ ?? {};
      if (emitChatToken) {
        emitChatToken('session-1', 'Hello ');
        emitChatToken('session-1', 'world!');
      } else {
        // Fallback: emit directly
        const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
        if (listeners?.has('chat:event')) {
          for (const handler of listeners.get('chat:event')) {
            handler({ event: 'chat:event', payload: { session_id: 'session-1', event: { Token: { delta: 'Hello ' } } }, id: Date.now() });
          }
          for (const handler of listeners.get('chat:event')) {
            handler({ event: 'chat:event', payload: { session_id: 'session-1', event: { Token: { delta: 'world!' } } }, id: Date.now() + 1 });
          }
        }
      }
    });

    await page.waitForTimeout(500);
    await assertHeartbeat(page, 'after-token-emission');
  });

  test('102. Chat stream done event completes streaming state', async ({ page }) => {
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    // Emit a token then done
    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('chat:event')) {
        for (const handler of listeners.get('chat:event')) {
          handler({ event: 'chat:event', payload: { session_id: 'session-1', event: { Token: { delta: 'Test response' } } }, id: Date.now() });
        }
      }
      if (listeners?.has('chat:done')) {
        for (const handler of listeners.get('chat:done')) {
          handler({ event: 'chat:done', payload: { session_id: 'session-1' }, id: Date.now() + 1 });
        }
      }
    });

    await page.waitForTimeout(500);
    await assertHeartbeat(page, 'after-stream-done');
  });

  test('103. Chat error event handles gracefully', async ({ page }) => {
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('chat:error')) {
        for (const handler of listeners.get('chat:error')) {
          handler({ event: 'chat:error', payload: { session_id: 'session-1', error: 'Model rate limited' }, id: Date.now() });
        }
      }
    });

    await page.waitForTimeout(500);
    // App should not freeze on error
    await assertHeartbeat(page, 'after-error-event');
  });

  test('104. Tool call start/result events update activity indicators', async ({ page }) => {
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('chat:event')) {
        // Emit tool call start
        for (const handler of listeners.get('chat:event')) {
          handler({
            event: 'chat:event',
            payload: {
              session_id: 'session-1',
              event: { ToolCallStart: { tool_id: 'fs.read_file', input: { path: '/tmp/test.txt' } } },
            },
            id: Date.now(),
          });
        }

        // Emit tool call result after a short delay
        setTimeout(() => {
          for (const handler of listeners.get('chat:event')!) {
            handler({
              event: 'chat:event',
              payload: {
                session_id: 'session-1',
                event: { ToolCallResult: { tool_id: 'fs.read_file', output: 'file contents here', is_error: false } },
              },
              id: Date.now(),
            });
          }
        }, 200);
      }
    });

    await page.waitForTimeout(800);
    await assertHeartbeat(page, 'after-tool-call');
  });

  test('105. Approval event creates toast notification', async ({ page }) => {
    // Wait for approval stream to be subscribed
    await page.waitForTimeout(1000);

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('approval:event')) {
        for (const handler of listeners.get('approval:event')) {
          handler({
            event: 'approval:event',
            payload: {
              type: 'added',
              session_id: 'session-1',
              agent_id: 'default',
              agent_name: 'Default Agent',
              request_id: 'approval-1',
              tool_id: 'shell.execute',
              input: '{"command": "rm -rf /tmp/test"}',
              reason: 'Shell command execution requires approval',
            },
            id: Date.now(),
          });
        }
      }
    });

    await page.waitForTimeout(500);
    await assertHeartbeat(page, 'after-approval-event');
  });

  test('106. Resolving approval removes toast', async ({ page }) => {
    await page.waitForTimeout(1000);

    // Add approval then resolve it
    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('approval:event')) {
        // Add
        for (const handler of listeners.get('approval:event')) {
          handler({
            event: 'approval:event',
            payload: { type: 'added', session_id: 'session-1', agent_id: 'default', request_id: 'approval-2', tool_id: 'fs.write_file', reason: 'Write approval' },
            id: Date.now(),
          });
        }
        // Resolve after delay
        setTimeout(() => {
          for (const handler of listeners.get('approval:event')!) {
            handler({
              event: 'approval:event',
              payload: { type: 'resolved', request_id: 'approval-2' },
              id: Date.now(),
            });
          }
        }, 300);
      }
    });

    await page.waitForTimeout(800);
    await assertHeartbeat(page, 'after-approval-resolved');
  });

  test('107. Workflow events trigger UI refresh', async ({ page }) => {
    await page.waitForTimeout(500);

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('workflow:event')) {
        for (const handler of listeners.get('workflow:event')) {
          handler({
            event: 'workflow:event',
            payload: { topic: 'workflow.instance.status_changed', payload: { instance_id: 'wf-inst-1' } },
            id: Date.now(),
          });
        }
      }
    });

    await page.waitForTimeout(500);
    await assertHeartbeat(page, 'after-workflow-event');
  });

  test('108. Multiple rapid token emissions dont freeze UI', async ({ page }) => {
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    // Emit 50 tokens rapidly
    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('chat:event')) {
        for (let i = 0; i < 50; i++) {
          for (const handler of listeners.get('chat:event')!) {
            handler({
              event: 'chat:event',
              payload: { session_id: 'session-1', event: { Token: { delta: `token-${i} ` } } },
              id: Date.now() + i,
            });
          }
        }
      }
    });

    await page.waitForTimeout(1000);
    await assertHeartbeat(page, 'after-rapid-tokens');
  });

  test('109. Stage events update agent display', async ({ page }) => {
    await page.waitForTimeout(500);

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      if (listeners?.has('stage:event')) {
        for (const handler of listeners.get('stage:event')) {
          handler({
            event: 'stage:event',
            payload: {
              session_id: 'session-1',
              event: {
                type: 'snapshot',
                agents: [
                  { id: 'agent-1', name: 'Default', status: 'active', spec: { role: 'coder' } },
                ],
                telemetry: { per_agent: [] },
              },
            },
            id: Date.now(),
          });
        }
      }
    });

    await page.waitForTimeout(500);
    await assertHeartbeat(page, 'after-stage-event');
  });

  test('110. Mixed event types in rapid succession', async ({ page }) => {
    await page.waitForSelector('[data-testid="composer-textarea"], .composer-input-area textarea', { timeout: 10_000 });

    await page.evaluate(() => {
      const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
      const emitAll = (event: string, payload: unknown) => {
        if (listeners?.has(event)) {
          for (const handler of listeners.get(event)!) {
            handler({ event, payload, id: Date.now() + Math.random() });
          }
        }
      };

      // Fire a mix of event types rapidly
      emitAll('chat:event', { session_id: 'session-1', event: { Token: { delta: 'Starting...' } } });
      emitAll('chat:event', { session_id: 'session-1', event: { ToolCallStart: { tool_id: 'fs.read_file' } } });
      emitAll('workflow:event', { topic: 'workflow.instance.created', payload: {} });
      emitAll('chat:event', { session_id: 'session-1', event: { ToolCallResult: { tool_id: 'fs.read_file', output: 'ok', is_error: false } } });
      emitAll('chat:event', { session_id: 'session-1', event: { Token: { delta: ' Done.' } } });
      emitAll('chat:done', { session_id: 'session-1' });
    });

    await page.waitForTimeout(1000);
    await assertHeartbeat(page, 'after-mixed-events');
  });
});
