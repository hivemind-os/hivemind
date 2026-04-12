import { createEffect, onCleanup } from 'solid-js';

/**
 * A createEffect wrapper for async work that automatically cancels stale executions.
 * The callback receives an AbortSignal that is aborted when the effect re-runs or the component unmounts.
 * Return values from stale executions are silently ignored via sequence counter.
 */
export function useAbortableEffect(fn: (signal: AbortSignal) => void | Promise<void>): void {
  let seq = 0;
  let controller: AbortController | undefined;

  createEffect(() => {
    // Abort any previous execution
    controller?.abort();
    const mySeq = ++seq;
    const ctrl = new AbortController();
    controller = ctrl;

    // Run the async work
    const result = fn(ctrl.signal);

    // If it returns a promise, catch abort errors silently
    if (result instanceof Promise) {
      result.catch((err) => {
        if (err?.name !== 'AbortError' && mySeq === seq) {
          console.error('useAbortableEffect error:', err);
        }
      });
    }
  });

  onCleanup(() => {
    seq++;
    controller?.abort();
  });
}
