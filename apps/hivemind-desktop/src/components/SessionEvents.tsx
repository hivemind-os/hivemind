import { createSignal, createEffect, on, onCleanup, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import EventLogList, { type SessionEvent } from './EventLogList';

interface PagedEventsResponse {
  events: SessionEvent[];
  total: number;
}

export interface SessionEventsProps {
  session_id: Accessor<string | null>;
  daemonOnline: Accessor<boolean>;
  entityType?: Accessor<string>;
}

export default function SessionEvents(props: SessionEventsProps) {
  const [events, setEvents] = createSignal<SessionEvent[]>([]);
  const [totalCount, setTotalCount] = createSignal(0);
  const [loading, setLoading] = createSignal(false);

  async function loadEvents(sid: string) {
    setLoading(true);
    try {
      const isBot = props.entityType?.() === 'bot';
      if (isBot) {
        const resp = await invoke<PagedEventsResponse>('get_bot_events', {
          agent_id: sid, offset: 0, limit: 500,
        });
        setEvents(resp.events ?? []);
        setTotalCount(resp.total ?? 0);
      } else {
        const resp = await invoke<PagedEventsResponse>('get_session_events', {
          session_id: sid, offset: 0, limit: 500,
        });
        setEvents(resp.events ?? []);
        setTotalCount(resp.total ?? 0);
      }
    } catch {
      setEvents([]);
      setTotalCount(0);
    } finally {
      setLoading(false);
    }
  }

  // Load events when session changes or daemon comes online,
  // and subscribe to push notifications to refetch on activity.
  createEffect(on(
    () => [props.session_id(), props.daemonOnline()] as const,
    ([sid, online]) => {
      if (sid && online) {
        void loadEvents(sid);
      } else {
        setEvents([]);
        setTotalCount(0);
      }

      // Set up push-based refetch for the active session
      let eventUnlisten: UnlistenFn | null = null;
      let doneUnlisten: UnlistenFn | null = null;
      let debounceTimer: ReturnType<typeof setTimeout> | undefined;
      let disposed = false;

      function debouncedRefetch() {
        if (!sid || !online) return;
        if (debounceTimer) clearTimeout(debounceTimer);
        debounceTimer = setTimeout(() => void loadEvents(sid), 300);
      }

      const cleanup = () => {
        eventUnlisten?.();
        doneUnlisten?.();
        eventUnlisten = null;
        doneUnlisten = null;
        if (debounceTimer) clearTimeout(debounceTimer);
      };

      if (sid && online) {
        const isBot = props.entityType?.() === 'bot';
        void (async () => {
          if (isBot) {
            // For bots, listen to the service-level bot SSE stream
            try { await invoke('ensure_bot_stream'); } catch { /* may already be running */ }
            if (disposed) return;
            eventUnlisten = await listen<{ session_id: string; event: any }>(
              'stage:event',
              (e) => {
                if (e.payload.session_id === '__service__') debouncedRefetch();
              },
            );
          } else {
            eventUnlisten = await listen<{ session_id: string; event: any }>(
              'chat:event',
              (e) => {
                if (e.payload.session_id === sid) debouncedRefetch();
              },
            );
            doneUnlisten = await listen<{ session_id: string }>(
              'chat:done',
              (e) => {
                if (e.payload.session_id === sid) debouncedRefetch();
              },
            );
          }
          if (disposed) cleanup();
        })();
      }

      onCleanup(() => {
        disposed = true;
        cleanup();
      });
    }
  ));

  return (
    <div class="flex flex-1 flex-col overflow-y-auto p-3">
      <EventLogList
        events={events()}
        totalCount={totalCount()}
        loading={loading()}
        hasMore={false}
      />
    </div>
  );
}
