import { batch, createSignal } from 'solid-js';

export type ActivityItem = {
  id: string;
  kind: 'tool' | 'model' | 'skill' | 'inference' | 'feedback';
  label: string;
  detail?: string;
  startedAt: number;
  done: boolean;
  error?: boolean;
};

export type ToolCallRecord = {
  id: string;
  tool_id: string;
  callId: string;
  label: string;
  input?: string;
  output?: string;
  isError: boolean;
  startedAt: number;
  completedAt?: number;
};

export function createStreamingStore() {
  const [streamingContent, setStreamingContent] = createSignal<string>('');
  const [isStreaming, setIsStreaming] = createSignal(false);
  const [activities, setActivities] = createSignal<ActivityItem[]>([]);

  // Accumulates tool calls for the current streaming turn.
  const [pendingToolCalls, setPendingToolCalls] = createSignal<ToolCallRecord[]>([]);
  // Persistent history: maps assistant message ID → tool calls used to produce it.
  const [toolCallHistory, setToolCallHistory] = createSignal<Record<string, ToolCallRecord[]>>({});

  // Track active removal timeouts so we can cancel them on cleanup
  const activityTimeouts = new Set<ReturnType<typeof setTimeout>>();

  // Incrementing counter for unique activity IDs
  let inferenceCounter = 0;
  let toolCallCounter = 0;

  const pushActivity = (item: Omit<ActivityItem, 'startedAt' | 'done'>) => {
    setActivities(prev => [...prev.filter(a => a.id !== item.id), { ...item, startedAt: Date.now(), done: false }]);
  };

  const completeActivity = (id: string, error?: boolean) => {
    setActivities(prev => prev.map(a => a.id === id ? { ...a, done: true, error } : a));
    const timerId = setTimeout(() => {
      activityTimeouts.delete(timerId);
      setActivities(prev => prev.filter(a => a.id !== id));
    }, 2000);
    activityTimeouts.add(timerId);
  };

  const recordToolCallStart = (activityId: string, tool_id: string, label: string, input?: string) => {
    const callId = `tc-${++toolCallCounter}`;
    setPendingToolCalls(prev => [...prev, {
      id: activityId,
      tool_id,
      callId,
      label,
      input,
      isError: false,
      startedAt: Date.now(),
    }]);
    return callId;
  };

  const recordToolCallResult = (callId: string, output?: string, isError?: boolean) => {
    setPendingToolCalls(prev => {
      const idx = prev.findIndex(tc => tc.callId === callId && !tc.completedAt);
      if (idx < 0) return prev;
      const updated = [...prev];
      updated[idx] = { ...updated[idx], output, isError: isError ?? false, completedAt: Date.now() };
      return updated;
    });
  };

  /** Flush pending tool calls and associate them with the given message ID. */
  const commitToolCalls = (messageId: string) => {
    const calls = pendingToolCalls();
    if (calls.length === 0) return;
    setToolCallHistory(prev => ({ ...prev, [messageId]: calls }));
    setPendingToolCalls([]);
  };

  const tryParseJson = (s: string | undefined): Record<string, unknown> | undefined => {
    try { return s ? JSON.parse(s) : undefined; } catch { return undefined; }
  };

  const truncate = (s: string | undefined, n: number) =>
    s && s.length > n ? s.slice(0, n) + '…' : s;

  const clearStreamingState = () => {
    batch(() => {
      setIsStreaming(false);
      setStreamingContent('');
      setActivities([]);
      setPendingToolCalls([]);
      // Clear all pending activity removal timeouts
      activityTimeouts.forEach(clearTimeout);
      activityTimeouts.clear();
    });
  };

  const beginStreamingState = () => {
    const activityId = `inference-${++inferenceCounter}`;
    batch(() => {
      setIsStreaming(true);
      setStreamingContent('');
      setActivities([]);
      setPendingToolCalls([]);
      // Clear stale timeouts from any previous streaming session
      activityTimeouts.forEach(clearTimeout);
      activityTimeouts.clear();
      pushActivity({ id: activityId, kind: 'inference', label: 'Thinking...' });
    });
    return activityId;
  };

  const cleanup = () => {
    activityTimeouts.forEach(clearTimeout);
    activityTimeouts.clear();
  };

  return {
    streamingContent, setStreamingContent,
    isStreaming, setIsStreaming,
    activities,
    pushActivity, completeActivity,
    pendingToolCalls, toolCallHistory,
    recordToolCallStart, recordToolCallResult, commitToolCalls,
    tryParseJson, truncate,
    clearStreamingState, beginStreamingState, cleanup,
  };
}

export type StreamingStore = ReturnType<typeof createStreamingStore>;
