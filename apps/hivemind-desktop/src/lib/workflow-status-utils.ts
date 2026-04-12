/**
 * Shared workflow-status formatting utilities.
 *
 * Extracted from WorkflowsPage.tsx / WorkflowDetailPanel.tsx /
 * workflows-columns.tsx so every consumer uses a single source of truth.
 */

// ---------------------------------------------------------------------------
// Step helpers
// ---------------------------------------------------------------------------

/** Extract the task object from a step, handling multiple backend serialisation shapes. */
export function getStepTask(step: any): any | undefined {
  return step.task ?? step.step_type?.Task ?? step.Task;
}

// ---------------------------------------------------------------------------
// Status formatting
// ---------------------------------------------------------------------------

/** Return CSS class-list for a pill badge based on workflow / step status. */
export function statusPill(status: string): string {
  switch (status) {
    case 'completed': return 'pill success';
    case 'running': return 'pill info';
    case 'paused': return 'pill warning';
    case 'waiting_on_input':
    case 'waiting_on_event': return 'pill warning';
    case 'failed': return 'pill danger';
    case 'killed': return 'pill danger';
    case 'pending': return 'pill neutral';
    case 'ready': return 'pill info';
    case 'skipped': return 'pill neutral';
    default: return 'pill neutral';
  }
}

/** Human-readable status label (replaces underscores with spaces). */
export function statusLabel(status: string): string {
  return status.replace(/_/g, ' ');
}

// ---------------------------------------------------------------------------
// Time / duration
// ---------------------------------------------------------------------------

/** Format a millisecond timestamp to a short locale string. */
export function formatTime(ms: number): string {
  if (ms == null) return '—';
  const d = new Date(ms);
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/**
 * Return a human-readable duration string.
 *
 * @param startMs  - start epoch in milliseconds
 * @param endMs    - end epoch in milliseconds (omit for still-running)
 * @param nowMs    - optional "now" override for testing / reactive ticks
 */
export function durationStr(startMs: number, endMs: number | null | undefined, nowMs?: number): string {
  if (startMs == null) return '—';
  const end = endMs ?? nowMs ?? Date.now();
  const secs = Math.floor((end - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
