import { createSignal, createEffect, createMemo, type Accessor, type Setter } from 'solid-js';
import type { ChatSessionSummary } from '../types';

export interface SessionOrderingDeps {
  sessions: Accessor<ChatSessionSummary[]>;
  setSessions: Setter<ChatSessionSummary[]>;
}

export interface SessionOrderingReturn {
  sessionOrder: Accessor<string[]>;
  setSessionOrder: Setter<string[]>;
  updateSessions: (list: ChatSessionSummary[] | null | undefined) => void;
  orderedSessions: Accessor<ChatSessionSummary[]>;
  displayedSessions: Accessor<ChatSessionSummary[]>;
  reorderSessions: (fromIndex: number, toIndex: number) => void;
}

const SESSION_ORDER_KEY = 'hivemind-session-order';

function safeParseJsonArray(key: string): string[] {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

export function createSessionOrdering(deps: SessionOrderingDeps): SessionOrderingReturn {
  const { sessions, setSessions } = deps;

  const [sessionOrder, setSessionOrder] = createSignal<string[]>(
    safeParseJsonArray(SESSION_ORDER_KEY)
  );
  createEffect(() => {
    localStorage.setItem(SESSION_ORDER_KEY, JSON.stringify(sessionOrder()));
  });

  const mergeOrder = (newList: ChatSessionSummary[] | null | undefined, currentOrder: string[]): string[] => {
    const safeList = Array.isArray(newList) ? newList : [];
    const existingSet = new Set(safeList.map((s) => s.id));
    const kept = currentOrder.filter((id) => existingSet.has(id));
    const keptSet = new Set(kept);
    const prepend = safeList.filter((s) => !keptSet.has(s.id)).map((s) => s.id);
    return [...prepend, ...kept];
  };

  const updateSessions = (list: ChatSessionSummary[] | null | undefined) => {
    const safeList = Array.isArray(list) ? list : [];
    setSessions(safeList);
    setSessionOrder((prev) => mergeOrder(safeList, prev));
  };

  const orderedSessions = createMemo(() => {
    const order = sessionOrder();
    const map = new Map(sessions().map((s) => [s.id, s]));
    return order.map((id) => map.get(id)).filter((s): s is ChatSessionSummary => s !== undefined);
  });

  // Sessions to display in the sidebar — excludes bot-backed sessions.
  const displayedSessions = createMemo(() =>
    orderedSessions().filter((s) => !s.bot_id)
  );

  const reorderSessions = (fromIndex: number, toIndex: number) => {
    setSessionOrder((prev) => {
      if (fromIndex < 0 || fromIndex >= prev.length || toIndex < 0 || toIndex > prev.length) return prev;
      const next = [...prev];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      return next;
    });
  };

  return {
    sessionOrder,
    setSessionOrder,
    updateSessions,
    orderedSessions,
    displayedSessions,
    reorderSessions,
  };
}
