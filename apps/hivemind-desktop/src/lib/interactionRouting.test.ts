/**
 * Unit tests for interactionRouting.ts — verifies that question answers,
 * tool approvals, and gate responses are dispatched to the correct Tauri
 * command (chat_respond_interaction vs agent_respond_interaction vs
 * bot_interaction) based on the interaction's agent_id, session_id, and
 * routing fields.
 *
 * These tests were added after fixing a bug where session-level questions
 * (agent_id === session_id) were routed to the agent endpoint, causing 404.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import {
  answerQuestion,
  respondToApproval,
  respondToGate,
  type PendingInteraction,
} from '../lib/interactionRouting';
import { _lastInvoke, _resetInvokes } from '../__mocks__/tauri';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeInteraction(overrides: Partial<PendingInteraction>): PendingInteraction {
  return {
    request_id: 'req-1',
    entity_id: 'session/sess-123',
    source_name: 'Test',
    type: 'question',
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// answerQuestion routing
// ---------------------------------------------------------------------------

describe('answerQuestion routing', () => {
  beforeEach(_resetInvokes);

  it('routes session-level question (agent_id === session_id) to chat_respond_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'sess-123', session_id: 'sess-123', routing: undefined }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('chat_respond_interaction');
    expect(_lastInvoke!.args).not.toHaveProperty('agent_id');
  });

  it('routes session-level question with routing="session" to chat_respond_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'sess-123', session_id: 'sess-123', routing: 'session' }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('chat_respond_interaction');
  });

  it('routes real supervisor sub-agent question to agent_respond_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'agent-456', session_id: 'sess-123', routing: 'session' }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('agent_respond_interaction');
    expect(_lastInvoke!.args.agent_id).toBe('agent-456');
  });

  it('routes sub-agent question without routing field to agent_respond_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'agent-456', session_id: 'sess-123', routing: undefined }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('agent_respond_interaction');
    expect(_lastInvoke!.args.agent_id).toBe('agent-456');
  });

  it('routes bot question with routing="bot" to bot_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'bot-789', session_id: undefined, routing: 'bot' }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('bot_interaction');
    expect(_lastInvoke!.args.agent_id).toBe('bot-789');
  });

  it('routes bot question without routing to bot_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: 'bot-789', session_id: undefined, routing: undefined }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('bot_interaction');
  });

  it('routes session-only question (no agent_id) to chat_respond_interaction', async () => {
    await answerQuestion(
      makeInteraction({ agent_id: undefined, session_id: 'sess-123', routing: undefined }),
      { text: 'hello' },
    );
    expect(_lastInvoke!.cmd).toBe('chat_respond_interaction');
  });

  it('throws when no routing info is available', async () => {
    await expect(
      answerQuestion(
        makeInteraction({ agent_id: undefined, session_id: undefined, routing: undefined }),
        { text: 'hello' },
      ),
    ).rejects.toThrow('Cannot route interaction: no routing info');
  });

  it('includes correct payload shape for choice answers', async () => {
    await answerQuestion(
      makeInteraction({ session_id: 'sess-123' }),
      { selected_choice: 0 },
    );
    const payload = _lastInvoke!.args.response as Record<string, unknown>;
    expect(payload).toHaveProperty('request_id', 'req-1');
    expect((payload as any).payload.selected_choice).toBe(0);
    expect((payload as any).payload.type).toBe('answer');
  });

  it('includes correct payload shape for multi-select answers', async () => {
    await answerQuestion(
      makeInteraction({ session_id: 'sess-123' }),
      { selected_choices: [0, 2] },
    );
    const payload = _lastInvoke!.args.response as Record<string, unknown>;
    expect((payload as any).payload.selected_choices).toEqual([0, 2]);
  });
});

// ---------------------------------------------------------------------------
// respondToApproval routing
// ---------------------------------------------------------------------------

describe('respondToApproval routing', () => {
  beforeEach(_resetInvokes);

  it('routes session-level approval (agent_id === session_id) to chat_respond_interaction', async () => {
    await respondToApproval(
      makeInteraction({
        type: 'tool_approval',
        agent_id: 'sess-123',
        session_id: 'sess-123',
        routing: 'session',
      }),
      { approved: true },
    );
    expect(_lastInvoke!.cmd).toBe('chat_respond_interaction');
  });

  it('routes sub-agent approval to agent_respond_interaction', async () => {
    await respondToApproval(
      makeInteraction({
        type: 'tool_approval',
        agent_id: 'agent-456',
        session_id: 'sess-123',
        routing: 'session',
      }),
      { approved: false },
    );
    expect(_lastInvoke!.cmd).toBe('agent_respond_interaction');
    expect(_lastInvoke!.args.agent_id).toBe('agent-456');
  });

  it('includes approval payload fields', async () => {
    await respondToApproval(
      makeInteraction({ type: 'tool_approval', session_id: 'sess-123' }),
      { approved: true, allow_session: true },
    );
    const payload = _lastInvoke!.args.response as any;
    expect(payload.payload.type).toBe('tool_approval');
    expect(payload.payload.approved).toBe(true);
    expect(payload.payload.allow_session).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// respondToGate routing
// ---------------------------------------------------------------------------

describe('respondToGate', () => {
  beforeEach(_resetInvokes);

  it('invokes workflow_respond_gate with instance_id and step_id', async () => {
    await respondToGate(
      makeInteraction({
        type: 'workflow_gate',
        instance_id: 42,
        step_id: 'step-1',
      }),
      { selected: 'option-a', text: 'notes' },
    );
    expect(_lastInvoke!.cmd).toBe('workflow_respond_gate');
    expect(_lastInvoke!.args.instance_id).toBe(42);
    expect(_lastInvoke!.args.step_id).toBe('step-1');
  });

  it('throws when missing instance_id/step_id', async () => {
    await expect(
      respondToGate(
        makeInteraction({ type: 'workflow_gate', instance_id: undefined, step_id: undefined }),
        { selected: 'x' },
      ),
    ).rejects.toThrow('missing instance_id/step_id');
  });
});
