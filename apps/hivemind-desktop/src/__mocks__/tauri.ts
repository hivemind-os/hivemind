/**
 * Mock for @tauri-apps/api/core used in vitest unit tests.
 *
 * Tests can inspect `_lastInvoke` or replace `invoke` via `vi.fn()`.
 */

export let _lastInvoke: { cmd: string; args: Record<string, unknown> } | null = null;
export const _invokeHistory: { cmd: string; args: Record<string, unknown> }[] = [];

export async function invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  const entry = { cmd, args: args ?? {} };
  _lastInvoke = entry;
  _invokeHistory.push(entry);
  return undefined;
}

/** Reset captured invocations between tests. */
export function _resetInvokes(): void {
  _lastInvoke = null;
  _invokeHistory.length = 0;
}
