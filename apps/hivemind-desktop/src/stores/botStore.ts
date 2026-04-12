import { createSignal, createMemo } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { BotSummary, AgentStatus } from '../types';

interface PendingApproval {
  agent_id: string;
  request_id: string;
  [key: string]: unknown;
}

interface PendingBotQuestion {
  agent_id: string;
  request_id: string;
  [key: string]: unknown;
}

interface BotEventPayload {
  error?: string;
}

export function createBotStore() {
  const [bots, setBots] = createSignal<BotSummary[]>([]);
  const [selectedBotId, setSelectedBotId] = createSignal<string | null>(null);
  const [statusFilter, setStatusFilter] = createSignal<AgentStatus | 'all'>('all');
  const [searchQuery, setSearchQuery] = createSignal('');
  const [pendingApprovals, setPendingApprovals] = createSignal<PendingApproval[]>([]);
  const [pendingQuestions, setPendingQuestions] = createSignal<PendingBotQuestion[]>([]);

  const filteredBots = createMemo(() => {
    let result = bots();
    const status = statusFilter();
    if (status !== 'all') {
      result = result.filter(b => b.status === status);
    }
    const query = searchQuery().toLowerCase();
    if (query) {
      result = result.filter(b => b.config.friendly_name.toLowerCase().includes(query));
    }
    return result;
  });

  function approvalsForBot(bot_id: string): number {
    return pendingApprovals().filter(a => a.agent_id === bot_id).length;
  }

  function questionsForBot(bot_id: string): number {
    return pendingQuestions().filter(q => q.agent_id === bot_id).length;
  }

  async function loadBots() {
    try {
      const result = await invoke<BotSummary[]>('list_bots');
      setBots(result ?? []);
    } catch (e) {
      console.warn('[botStore] Failed to load bots:', e);
    }
  }

  async function loadPendingInteractions() {
    try {
      const [approvals, questions] = await Promise.all([
        invoke<PendingApproval[]>('list_pending_approvals'),
        invoke<PendingBotQuestion[]>('list_all_pending_questions'),
      ]);
      setPendingApprovals(approvals ?? []);
      setPendingQuestions(questions ?? []);
    } catch (e) {
      console.warn('[botStore] Failed to load pending interactions:', e);
    }
  }

  function selectBot(bot_id: string | null) {
    setSelectedBotId(bot_id);
  }

  async function refresh() {
    await Promise.all([loadBots(), loadPendingInteractions()]);
  }

  // ── SSE subscription for real-time bot events ──

  let eventUnlisten: UnlistenFn | null = null;
  let errorUnlisten: UnlistenFn | null = null;
  let sseConnected = false;
  let disposed = false;

  // Debounce helper: coalesce rapid-fire events into a single refresh
  let refreshDebounce: ReturnType<typeof setTimeout> | undefined;
  function debouncedRefresh() {
    if (refreshDebounce) clearTimeout(refreshDebounce);
    refreshDebounce = setTimeout(() => void refresh(), 200);
  }

  async function subscribeBotEvents() {
    if (sseConnected) return;
    disposed = false;

    try {
      await invoke('bot_subscribe');
    } catch (e) {
      console.warn('[botStore] Failed to start bot event stream:', e);
      return;
    }

    if (disposed) return;

    try {
      const [eventUl, errorUl] = await Promise.all([
        listen<{ session_id: string; event: any }>('stage:event', (e) => {
          if (e.payload.session_id !== '__service__') return;
          if (!disposed) debouncedRefresh();
        }),
        listen<BotEventPayload>('bot:error', (e) => {
          console.warn('[botStore] Bot event stream error:', e.payload?.error);
        }),
      ]);

      if (disposed) {
        eventUl();
        errorUl();
        return;
      }

      eventUnlisten = eventUl;
      errorUnlisten = errorUl;
      sseConnected = true;
    } catch (e) {
      console.warn('[botStore] Failed to listen for bot events:', e);
    }
  }

  function unsubscribeBotEvents() {
    disposed = true;
    if (eventUnlisten) { eventUnlisten(); eventUnlisten = null; }
    if (errorUnlisten) { errorUnlisten(); errorUnlisten = null; }
    if (refreshDebounce) clearTimeout(refreshDebounce);
    sseConnected = false;
  }

  return {
    bots, filteredBots, selectedBotId, statusFilter, searchQuery,
    pendingApprovals, pendingQuestions,
    setStatusFilter, setSearchQuery, setSelectedBotId,
    approvalsForBot, questionsForBot,
    loadBots, loadPendingInteractions, selectBot, refresh,
    subscribeBotEvents, unsubscribeBotEvents,
  };
}

export type BotStore = ReturnType<typeof createBotStore>;
