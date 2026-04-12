/**
 * Centralized interaction store — single source of truth for pending
 * interactions (questions, approvals, workflow gates) and badge counts.
 *
 * Subscribes to a push-based SSE stream via the Tauri bridge and exposes
 * scoped accessors that components can use without reimplementing routing.
 */
import { createSignal, createMemo } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  PendingInteraction,
  InteractionCounts,
} from "../lib/interactionRouting";

// ── Store factory ────────────────────────────────────────────────────────

export function createInteractionStore() {
  const [interactions, setInteractions] = createSignal<PendingInteraction[]>([]);
  const [counts, setCounts] = createSignal<Record<string, InteractionCounts>>({});

  // ── Push-based subscription ────────────────────────────────────────

  let unlisten: UnlistenFn | null = null;
  let subscribed = false;
  let disposed = false;
  let refreshTimer: ReturnType<typeof setInterval> | null = null;

  /** Fetch a snapshot via IPC commands (works regardless of SSE state). */
  async function poll() {
    try {
      const [interactionsResult, countsResult] = await Promise.all([
        invoke<PendingInteraction[]>("list_pending_interactions").catch(() => null),
        invoke<Record<string, InteractionCounts>>("get_pending_interaction_counts").catch(() => null),
      ]);
      if (disposed) return;
      if (interactionsResult) setInteractions(interactionsResult);
      if (countsResult) setCounts(countsResult);
    } catch (e) {
      console.warn("[interactionStore] poll failed:", e);
    }
  }

  async function startPolling() {
    disposed = false;
    if (subscribed) return;

    try {
      // Attach the listener BEFORE starting the SSE subscription so the
      // initial snapshot (emitted immediately on connect) is never missed.
      const ul = await listen<string>("interaction:event", (ev) => {
        if (disposed) return;
        try {
          const snapshot = JSON.parse(ev.payload) as {
            interactions: PendingInteraction[];
            counts: Record<string, InteractionCounts>;
          };
          setInteractions(snapshot.interactions);
          setCounts(snapshot.counts);
        } catch (e) {
          console.warn("[interactionStore] failed to parse SSE event:", e);
        }
      });

      if (disposed) {
        ul();
        return;
      }

      unlisten = ul;
      subscribed = true;
    } catch (e) {
      console.warn("[interactionStore] Failed to listen for interaction events:", e);
    }

    // Start the SSE subscription on the Tauri side (after listener is attached).
    invoke("interactions_subscribe").catch((e: unknown) => {
      console.warn("[interactionStore] interactions_subscribe failed:", e);
    });

    // Fetch a snapshot to cover any interactions that existed before the
    // SSE connected (the SSE initial snapshot races with the HTTP connect).
    void poll();

    // Safety-net: poll periodically in case the SSE stream dies silently
    // or never connects.  This guarantees pending questions surface within
    // a few seconds regardless of SSE health.
    if (!refreshTimer) {
      refreshTimer = setInterval(() => {
        if (!disposed) void poll();
      }, 5_000);
    }
  }

  /** Re-subscribe the SSE stream (e.g. after daemon reconnect). */
  function resubscribe() {
    invoke("interactions_subscribe").catch((e: unknown) => {
      console.warn("[interactionStore] interactions_subscribe (resub) failed:", e);
    });
    void poll();
  }

  function stopPolling() {
    disposed = true;
    subscribed = false;
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
    if (refreshTimer) {
      clearInterval(refreshTimer);
      refreshTimer = null;
    }
  }

  // ── Scoped accessors ─────────────────────────────────────────────────

  /** All interactions owned by a specific entity. */
  function interactionsForEntity(entity_id: string): PendingInteraction[] {
    return interactions().filter((i) => i.entity_id === entity_id);
  }

  /** Badge count for an entity (includes propagated child counts). */
  function badgeCountForEntity(entity_id: string): InteractionCounts {
    return counts()[entity_id] ?? { questions: 0, approvals: 0, gates: 0 };
  }

  /** Total interaction count across all entities. */
  const totalCount = createMemo(() => {
    const all = interactions();
    return {
      questions: all.filter((i) => i.type === "question").length,
      approvals: all.filter((i) => i.type === "tool_approval").length,
      gates: all.filter((i) => i.type === "workflow_gate").length,
      total: all.length,
    };
  });

  /** Get all interactions of a specific type. */
  function interactionsOfType(type: PendingInteraction["type"]): PendingInteraction[] {
    return interactions().filter((i) => i.type === type);
  }

  /** Get all interactions whose entity is a descendant of the given entity. */
  function interactionsUnderEntity(entity_id: string): PendingInteraction[] {
    const c = counts();
    // If the entity has counts, there are interactions somewhere under it
    const entityCounts = c[entity_id];
    if (
      !entityCounts ||
      entityCounts.questions + entityCounts.approvals + entityCounts.gates === 0
    ) {
      return [];
    }
    return interactions().filter((i) => {
      // Direct match
      if (i.entity_id === entity_id) return true;
      // The interaction's entity is a descendant of entity_id (hierarchical prefix match)
      return i.entity_id.startsWith(entity_id + "/");
    });
  }

  return {
    // State
    interactions,
    counts,
    // Polling
    poll,
    startPolling,
    stopPolling,
    resubscribe,
    // Accessors
    interactionsForEntity,
    badgeCountForEntity,
    totalCount,
    interactionsOfType,
    interactionsUnderEntity,
  };
}

export type InteractionStore = ReturnType<typeof createInteractionStore>;
