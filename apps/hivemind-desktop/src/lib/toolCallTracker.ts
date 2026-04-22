/**
 * Tool call tracking state machine — extracted from App.tsx for testability.
 *
 * Manages the lifecycle of tool calls during LLM streaming:
 *   recordStart → recordResult → commit (to a message ID)
 *
 * Session-scoped: each tracker is bound to a session ID.  Events from other
 * sessions are silently ignored, preventing cross-session contamination.
 */
import { createSignal, batch, untrack } from 'solid-js';

export type ToolCallRecord = {
  id: string;
  tool_id: string;
  label: string;
  input?: string;
  output?: string;
  isError: boolean;
  startedAt: number;
  completedAt?: number;
  /** Raw MCP CallToolResult (for MCP Apps structuredContent) */
  mcpRaw?: unknown;
};

export function createToolCallTracker() {
  const [pendingToolCalls, setPendingToolCalls] = createSignal<ToolCallRecord[]>([]);
  const [toolCallHistory, setToolCallHistory] = createSignal<Record<string, ToolCallRecord[]>>({});
  // Per-session cache so tool call history survives session switches
  const toolCallHistoryCache = new Map<string, Record<string, ToolCallRecord[]>>();

  // The session this tracker is currently scoped to.
  let activeSessionId: string | null = null;

  /** Bind the tracker to a session.  Clears pending state & swaps cache. */
  function switchSession(newSessionId: string | null, prevSessionId: string | null) {
    // Save current tool call history before switching away.
    // Use untrack() to avoid creating a reactive dependency on toolCallHistory
    // inside the caller's createEffect — otherwise every commit would re-trigger
    // the session-switch effect in an infinite loop.
    if (prevSessionId) {
      const current = untrack(() => toolCallHistory());
      if (Object.keys(current).length > 0) {
        toolCallHistoryCache.set(prevSessionId, current);
      }
    }
    activeSessionId = newSessionId;
    batch(() => {
      setPendingToolCalls([]);
      setToolCallHistory(newSessionId ? (toolCallHistoryCache.get(newSessionId) ?? {}) : {});
    });
  }

  /** Guard: is this event for the currently active session? */
  function isActiveSession(sessionId: string): boolean {
    return sessionId === activeSessionId;
  }

  function recordStart(sessionId: string, activityId: string, tool_id: string, label: string, input?: string) {
    if (sessionId !== activeSessionId) return;
    setPendingToolCalls(prev => [...prev, { id: activityId, tool_id, label, input, isError: false, startedAt: Date.now() }]);
  }

  function recordResult(sessionId: string, tool_id: string, output?: string, isError?: boolean, mcpRaw?: unknown) {
    if (sessionId !== activeSessionId) return;
    setPendingToolCalls(prev => {
      const idx = prev.findIndex(tc => tc.tool_id === tool_id && !tc.completedAt);
      if (idx < 0) return prev;
      const updated = [...prev];
      updated[idx] = { ...updated[idx], output, isError: isError ?? false, completedAt: Date.now(), mcpRaw };
      return updated;
    });
  }

  function commit(sessionId: string, messageId: string) {
    if (sessionId !== activeSessionId) return;
    const calls = pendingToolCalls();
    if (calls.length === 0) return;
    setToolCallHistory(prev => ({ ...prev, [messageId]: calls }));
    setPendingToolCalls([]);
  }

  /** Capture pending calls, clear state, return the captured calls. */
  function captureAndClear(sessionId: string): ToolCallRecord[] {
    if (sessionId !== activeSessionId) return [];
    const captured = pendingToolCalls().slice();
    clearPending();
    return captured;
  }

  function clearPending() {
    setPendingToolCalls([]);
  }

  /** Commit previously-captured calls to a message after async sync. */
  function commitCaptured(sessionId: string, messageId: string, captured: ToolCallRecord[]) {
    if (sessionId !== activeSessionId) return;
    if (captured.length === 0) return;
    setToolCallHistory(prev => ({ ...prev, [messageId]: captured }));
  }

  return {
    pendingToolCalls,
    toolCallHistory,
    switchSession,
    isActiveSession,
    recordStart,
    recordResult,
    commit,
    captureAndClear,
    clearPending,
    commitCaptured,
    // Expose for tests
    get activeSessionId() { return activeSessionId; },
    toolCallHistoryCache,
  };
}

export type ToolCallTracker = ReturnType<typeof createToolCallTracker>;
