import { onCleanup } from 'solid-js';

/**
 * Provides setTimeout/setInterval wrappers that automatically clear on component cleanup.
 * Use this instead of raw setTimeout/setInterval to prevent timer leaks.
 */
export function useTimerCleanup() {
  const timeouts = new Set<ReturnType<typeof setTimeout>>();
  const intervals = new Set<ReturnType<typeof setInterval>>();

  onCleanup(() => {
    timeouts.forEach(clearTimeout);
    intervals.forEach(clearInterval);
    timeouts.clear();
    intervals.clear();
  });

  const safeTimeout = (fn: () => void, ms: number): ReturnType<typeof setTimeout> => {
    const id = setTimeout(() => {
      timeouts.delete(id);
      fn();
    }, ms);
    timeouts.add(id);
    return id;
  };

  const safeInterval = (fn: () => void, ms: number): ReturnType<typeof setInterval> => {
    const id = setInterval(fn, ms);
    intervals.add(id);
    return id;
  };

  const clearSafeTimeout = (id: ReturnType<typeof setTimeout>) => {
    clearTimeout(id);
    timeouts.delete(id);
  };

  const clearSafeInterval = (id: ReturnType<typeof setInterval>) => {
    clearInterval(id);
    intervals.delete(id);
  };

  return { safeTimeout, safeInterval, clearSafeTimeout, clearSafeInterval };
}
