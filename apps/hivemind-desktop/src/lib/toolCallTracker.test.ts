/**
 * Tests for toolCallTracker — session-scoped tool call state machine.
 *
 * These tests verify:
 *  1. Cross-session contamination: events from session A must NOT leak into
 *     session B's pending tool calls or history.
 *  2. Session switching clears pending state and swaps caches.
 *  3. Commit assigns pending calls to the correct message.
 *  4. captureAndClear + commitCaptured lifecycle (used by syncChatStateAfterStream).
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { createRoot } from 'solid-js';
import { createToolCallTracker, type ToolCallTracker } from './toolCallTracker';

// Helper: run test body inside a SolidJS reactive root (required for signals)
function withRoot<T>(fn: (dispose: () => void) => T): T {
  let result!: T;
  createRoot((dispose) => {
    result = fn(dispose);
  });
  return result;
}

describe('toolCallTracker', () => {
  // -------------------------------------------------------------------------
  // BUG REPRODUCTION: Cross-session contamination
  // -------------------------------------------------------------------------

  describe('cross-session contamination', () => {
    it('ignores recordStart from a different session', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-a', null);

        // Event arrives for session-b while session-a is active
        tracker.recordStart('session-b', 'act-1', 'mcp.sysmon.get-system-info', 'Running tool');

        expect(tracker.pendingToolCalls()).toEqual([]);
        dispose();
      });
    });

    it('ignores recordResult from a different session', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'act-1', 'mcp.sysmon.get-info', 'Running tool');

        // Result arrives tagged with the wrong session
        tracker.recordResult('session-b', 'mcp.sysmon.get-info', '{"hostname":"x"}');

        const pending = tracker.pendingToolCalls();
        expect(pending).toHaveLength(1);
        expect(pending[0].completedAt).toBeUndefined(); // not completed
        dispose();
      });
    });

    it('ignores commit from a different session', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'act-1', 'tool-1', 'Running');
        tracker.recordResult('session-a', 'tool-1', 'output');

        // Commit arrives for the wrong session
        tracker.commit('session-b', 'msg-1');

        // Pending NOT cleared, history NOT populated
        expect(tracker.pendingToolCalls()).toHaveLength(1);
        expect(tracker.toolCallHistory()).toEqual({});
        dispose();
      });
    });

    it('REPRODUCES BUG: concurrent sessions corrupt pendingToolCalls without session scoping', () => {
      // This test demonstrates the original bug where pendingToolCalls was
      // a single global signal with no session guard.
      //
      // Scenario: User is on session-b. Session-a SSE stream is still alive
      // (sub-agents running). Events from session-a's sub-agents call
      // recordToolCallStart/Result, polluting session-b's pending state.
      // When session-b completes, the commit captures session-a's tool calls.
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-b', null);

        // Session-b starts its own tool call
        tracker.recordStart('session-b', 'b-act', 'core.read_file', 'Reading file');

        // Meanwhile, session-a sub-agent events arrive (stale SSE stream)
        tracker.recordStart('session-a', 'a-act', 'mcp.sysmon.get-system-info', 'Running sysmon');
        tracker.recordResult('session-a', 'mcp.sysmon.get-system-info', '{"hostname":"x"}');

        // Pending should only contain session-b's tool call
        const pending = tracker.pendingToolCalls();
        expect(pending).toHaveLength(1);
        expect(pending[0].tool_id).toBe('core.read_file');

        // Session-b stream ends, commit to message
        tracker.commit('session-b', 'msg-b');
        const history = tracker.toolCallHistory();
        expect(history['msg-b']).toHaveLength(1);
        expect(history['msg-b'][0].tool_id).toBe('core.read_file');
        // MUST NOT contain session-a's sysmon tool call
        expect(history['msg-b'].find(tc => tc.tool_id === 'mcp.sysmon.get-system-info')).toBeUndefined();

        dispose();
      });
    });

    it('REPRODUCES BUG: stale listener writes to shared state after session switch', () => {
      // Simulates the race: user switches from session-a to session-b.
      // A stale listener for session-a fires events AFTER the switch.
      withRoot((dispose) => {
        const tracker = createToolCallTracker();

        // Start on session-a
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'a-1', 'core.spawn_agent', 'Spawning');

        // Switch to session-b
        tracker.switchSession('session-b', 'session-a');

        // Stale events from session-a arrive after the switch
        tracker.recordResult('session-a', 'core.spawn_agent', 'agent-123');
        tracker.recordStart('session-a', 'a-2', 'mcp.sysmon.get-info', 'Running');
        tracker.recordResult('session-a', 'mcp.sysmon.get-info', '{"hostname":"x"}');

        // Session-b's pending state should be clean
        expect(tracker.pendingToolCalls()).toEqual([]);

        // Session-b records its own tool call
        tracker.recordStart('session-b', 'b-1', 'core.ask_user', 'Asking');
        expect(tracker.pendingToolCalls()).toHaveLength(1);
        expect(tracker.pendingToolCalls()[0].tool_id).toBe('core.ask_user');

        dispose();
      });
    });
  });

  // -------------------------------------------------------------------------
  // Session switching
  // -------------------------------------------------------------------------

  describe('session switching', () => {
    it('clears pending tool calls on session switch', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'act-1', 'tool-1', 'Running');

        tracker.switchSession('session-b', 'session-a');
        expect(tracker.pendingToolCalls()).toEqual([]);
        dispose();
      });
    });

    it('caches and restores tool call history across session switches', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();

        // Build history in session-a
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'a-1', 'tool-a', 'Running A');
        tracker.recordResult('session-a', 'tool-a', 'output-a');
        tracker.commit('session-a', 'msg-a');

        // Switch to session-b
        tracker.switchSession('session-b', 'session-a');
        expect(tracker.toolCallHistory()).toEqual({}); // fresh

        // Build history in session-b
        tracker.recordStart('session-b', 'b-1', 'tool-b', 'Running B');
        tracker.recordResult('session-b', 'tool-b', 'output-b');
        tracker.commit('session-b', 'msg-b');

        // Switch back to session-a — history restored
        tracker.switchSession('session-a', 'session-b');
        expect(tracker.toolCallHistory()['msg-a']).toHaveLength(1);
        expect(tracker.toolCallHistory()['msg-a'][0].tool_id).toBe('tool-a');

        // Switch back to session-b — its history also restored
        tracker.switchSession('session-b', 'session-a');
        expect(tracker.toolCallHistory()['msg-b']).toHaveLength(1);
        expect(tracker.toolCallHistory()['msg-b'][0].tool_id).toBe('tool-b');

        dispose();
      });
    });

    it('switching to null session clears everything', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-a', null);
        tracker.recordStart('session-a', 'a-1', 'tool', 'Running');

        tracker.switchSession(null, 'session-a');
        expect(tracker.pendingToolCalls()).toEqual([]);
        expect(tracker.toolCallHistory()).toEqual({});
        expect(tracker.activeSessionId).toBeNull();
        dispose();
      });
    });
  });

  // -------------------------------------------------------------------------
  // Core lifecycle
  // -------------------------------------------------------------------------

  describe('core lifecycle', () => {
    it('recordStart adds to pending', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'mcp.sysmon.get-info', 'Running sysmon');

        const pending = tracker.pendingToolCalls();
        expect(pending).toHaveLength(1);
        expect(pending[0].tool_id).toBe('mcp.sysmon.get-info');
        expect(pending[0].completedAt).toBeUndefined();
        dispose();
      });
    });

    it('recordResult completes matching pending call', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'mcp.sysmon.get-info', 'Running');
        tracker.recordResult('s1', 'mcp.sysmon.get-info', '{"hostname":"x"}', false, { raw: true });

        const pending = tracker.pendingToolCalls();
        expect(pending).toHaveLength(1);
        expect(pending[0].completedAt).toBeDefined();
        expect(pending[0].output).toBe('{"hostname":"x"}');
        expect(pending[0].mcpRaw).toEqual({ raw: true });
        dispose();
      });
    });

    it('commit moves pending to history under message ID', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'tool-1', 'Running');
        tracker.recordResult('s1', 'tool-1', 'output');
        tracker.commit('s1', 'msg-1');

        expect(tracker.pendingToolCalls()).toEqual([]);
        expect(tracker.toolCallHistory()['msg-1']).toHaveLength(1);
        expect(tracker.toolCallHistory()['msg-1'][0].tool_id).toBe('tool-1');
        dispose();
      });
    });

    it('commit with no pending calls is a no-op', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.commit('s1', 'msg-1');

        expect(tracker.toolCallHistory()).toEqual({});
        dispose();
      });
    });
  });

  // -------------------------------------------------------------------------
  // captureAndClear + commitCaptured (syncChatStateAfterStream pattern)
  // -------------------------------------------------------------------------

  describe('captureAndClear + commitCaptured', () => {
    it('captures pending calls and clears state', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'tool-1', 'Running');
        tracker.recordResult('s1', 'tool-1', 'output');

        const captured = tracker.captureAndClear('s1');
        expect(captured).toHaveLength(1);
        expect(captured[0].tool_id).toBe('tool-1');
        expect(tracker.pendingToolCalls()).toEqual([]); // cleared
        dispose();
      });
    });

    it('commitCaptured writes captured calls to history', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'tool-1', 'Running');
        tracker.recordResult('s1', 'tool-1', 'output');

        const captured = tracker.captureAndClear('s1');
        // Simulate: async sync resolves, now we know the message ID
        tracker.commitCaptured('s1', 'msg-1', captured);

        expect(tracker.toolCallHistory()['msg-1']).toHaveLength(1);
        dispose();
      });
    });

    it('captureAndClear returns empty for wrong session', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'tool-1', 'Running');

        const captured = tracker.captureAndClear('s2');
        expect(captured).toEqual([]);
        // Pending NOT cleared (it was for the wrong session)
        expect(tracker.pendingToolCalls()).toHaveLength(1);
        dispose();
      });
    });

    it('commitCaptured ignores stale session', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);
        tracker.recordStart('s1', 'act-1', 'tool-1', 'Running');
        const captured = tracker.captureAndClear('s1');

        // User switched to s2 before the async sync completed
        tracker.switchSession('s2', 's1');
        tracker.commitCaptured('s1', 'msg-1', captured);

        // Should NOT have written to s2's history
        expect(tracker.toolCallHistory()).toEqual({});
        dispose();
      });
    });

    it('REPRODUCES BUG: interleaved capture from two sessions without scoping', () => {
      // Scenario: session-a is streaming with sub-agents, user sends message
      // on session-b. Both have events arriving. Without session scoping,
      // captureAndClear for session-b would pick up session-a's pending calls.
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-b', null);

        // Session-b tool call
        tracker.recordStart('session-b', 'b-1', 'core.read_file', 'Reading');
        tracker.recordResult('session-b', 'core.read_file', 'file contents');

        // Session-a sub-agent events arrive (should be rejected)
        tracker.recordStart('session-a', 'a-1', 'mcp.sysmon.get-info', 'Sysmon');
        tracker.recordResult('session-a', 'mcp.sysmon.get-info', '{"hostname":"x"}');

        // Capture for session-b
        const captured = tracker.captureAndClear('session-b');
        expect(captured).toHaveLength(1);
        expect(captured[0].tool_id).toBe('core.read_file');
        // MUST NOT contain session-a tool call
        expect(captured.find(tc => tc.tool_id === 'mcp.sysmon.get-info')).toBeUndefined();

        dispose();
      });
    });
  });

  // -------------------------------------------------------------------------
  // Multiple tool calls in one turn
  // -------------------------------------------------------------------------

  describe('multiple tool calls', () => {
    it('handles multiple tool calls in one streaming turn', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);

        tracker.recordStart('s1', 'act-1', 'core.spawn_agent', 'Spawning');
        tracker.recordStart('s1', 'act-2', 'core.wait_for_agent', 'Waiting');
        tracker.recordResult('s1', 'core.spawn_agent', 'agent-1');
        tracker.recordResult('s1', 'core.wait_for_agent', 'done');

        tracker.commit('s1', 'msg-1');
        const history = tracker.toolCallHistory()['msg-1'];
        expect(history).toHaveLength(2);
        expect(history[0].tool_id).toBe('core.spawn_agent');
        expect(history[1].tool_id).toBe('core.wait_for_agent');
        dispose();
      });
    });

    it('recordResult matches first uncompleted call with matching tool_id', () => {
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);

        // Two calls to the same tool
        tracker.recordStart('s1', 'act-1', 'mcp.sysmon.get-info', 'Run 1');
        tracker.recordStart('s1', 'act-2', 'mcp.sysmon.get-info', 'Run 2');

        // First result completes the first call
        tracker.recordResult('s1', 'mcp.sysmon.get-info', 'output-1');
        const pending = tracker.pendingToolCalls();
        expect(pending[0].completedAt).toBeDefined();
        expect(pending[0].output).toBe('output-1');
        expect(pending[1].completedAt).toBeUndefined(); // second still pending

        // Second result completes the second call
        tracker.recordResult('s1', 'mcp.sysmon.get-info', 'output-2');
        const pending2 = tracker.pendingToolCalls();
        expect(pending2[1].completedAt).toBeDefined();
        expect(pending2[1].output).toBe('output-2');

        dispose();
      });
    });
  });

  // -------------------------------------------------------------------------
  // Reactive safety: switchSession must NOT create tracking dependencies
  // -------------------------------------------------------------------------

  describe('reactive safety', () => {
    it('switchSession does not track toolCallHistory reads (untrack regression)', () => {
      // Verify the untrack() wrapping by checking that switchSession reads
      // toolCallHistory without creating a dependency. In SSR mode (vitest node
      // environment), SolidJS effects don't run, so we verify the untrack
      // indirectly: call switchSession with a prevSessionId that has history,
      // then verify the cache was correctly populated without any signal errors.
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('session-1', null);

        // Build up some history
        tracker.recordStart('session-1', 'act-1', 'tool-1', 'Running');
        tracker.recordResult('session-1', 'tool-1', 'output');
        tracker.commitCaptured('session-1', 'msg-1', tracker.captureAndClear('session-1'));

        expect(tracker.toolCallHistory()['msg-1']).toHaveLength(1);

        // Now switch sessions — this should cache session-1's history via
        // untracked read, then restore session-2's (empty) history
        tracker.switchSession('session-2', 'session-1');

        // Session-2 has no history yet
        expect(tracker.toolCallHistory()).toEqual({});

        // Switch back — cached history should be restored
        tracker.switchSession('session-1', 'session-2');
        expect(tracker.toolCallHistory()['msg-1']).toHaveLength(1);
        expect(tracker.toolCallHistory()['msg-1'][0].tool_id).toBe('tool-1');

        // Verify the cache is correctly populated
        expect(tracker.toolCallHistoryCache.has('session-1')).toBe(true);

        dispose();
      });
    });

    it('commitCaptured correctly writes to toolCallHistory after sync', () => {
      // End-to-end simulation of the syncChatStateAfterStream flow
      withRoot((dispose) => {
        const tracker = createToolCallTracker();
        tracker.switchSession('s1', null);

        // Simulate streaming: tool call events arrive
        tracker.recordStart('s1', 'act-1', 'mcp.sysmon.get-system-info', 'Running sysmon');
        tracker.recordResult('s1', 'mcp.sysmon.get-system-info', '{"hostname":"x"}', false, { raw: true });

        // Simulate Done event: capture and clear
        const captured = tracker.captureAndClear('s1');
        expect(captured).toHaveLength(1);
        expect(tracker.pendingToolCalls()).toEqual([]);

        // Simulate syncChatState completing and finding the assistant message
        tracker.commitCaptured('s1', 'assistant-msg-1', captured);

        // toolCallHistory should now have the committed calls
        const history = tracker.toolCallHistory();
        expect(history['assistant-msg-1']).toHaveLength(1);
        expect(history['assistant-msg-1'][0].tool_id).toBe('mcp.sysmon.get-system-info');
        expect(history['assistant-msg-1'][0].mcpRaw).toEqual({ raw: true });

        dispose();
      });
    });
  });
});
