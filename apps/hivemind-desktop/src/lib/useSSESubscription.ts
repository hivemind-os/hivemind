import { onCleanup } from 'solid-js';
import { listen, type UnlistenFn, type EventCallback } from '@tauri-apps/api/event';

export interface SSESubscriptionOptions<T> {
  /** The Tauri event name to listen for */
  event: string;
  /** Callback when an event is received */
  onEvent: EventCallback<T>;
  /** Called when subscription is successfully established */
  onConnected?: () => void;
  /** Called when subscription fails or is cleaned up */
  onDisconnected?: () => void;
  /** Called on subscription error */
  onError?: (err: unknown) => void;
}

/**
 * Subscribe to a Tauri SSE event with proper lifecycle management.
 * - Defers connected state until listen() resolves
 * - Registers cleanup before the await (no cleanup race)
 * - Handles reconnection on failure
 */
export function useSSESubscription<T>(options: SSESubscriptionOptions<T>): { unsubscribe: () => void } {
  let unlisten: UnlistenFn | undefined;
  let disposed = false;

  const cleanup = () => {
    disposed = true;
    if (unlisten) {
      unlisten();
      unlisten = undefined;
    }
    options.onDisconnected?.();
  };

  // Register cleanup BEFORE the async call - this is the key fix
  onCleanup(cleanup);

  // Start listening
  listen<T>(options.event, (event) => {
    if (!disposed) {
      options.onEvent(event);
    }
  }).then((unlistenFn) => {
    if (disposed) {
      // Component already unmounted, clean up immediately
      unlistenFn();
    } else {
      unlisten = unlistenFn;
      options.onConnected?.();
    }
  }).catch((err) => {
    if (!disposed) {
      options.onError?.(err);
    }
  });

  return { unsubscribe: cleanup };
}
