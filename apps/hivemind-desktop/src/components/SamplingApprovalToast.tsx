import { For, Show, createSignal, onCleanup, createEffect } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { CheckCircle, XCircle, Cpu } from 'lucide-solid';
import { Button } from '~/ui';
import { authFetch } from '~/lib/authFetch';
import { logError, logInfo } from './ActivityLog';

/** Payload shape for mcp.sampling.approval events. */
interface SamplingApprovalEvent {
  type: 'requested' | 'resolved' | 'expired';
  id: string;
  server_id?: string;
  message_count?: number;
  max_tokens?: number;
  model_hint?: string | null;
  preview?: string;
  timeout_secs?: number;
  approved?: boolean;
}

export interface PendingSamplingApproval {
  id: string;
  server_id: string;
  message_count: number;
  max_tokens: number;
  model_hint: string | null;
  preview: string;
  remaining_secs: number;
}

const [approvals, setApprovals] = createSignal<PendingSamplingApproval[]>([]);

/** Read-only accessor for pending sampling approvals. */
export const pendingSamplingApprovals = approvals;

/** Dismiss a toast after resolution/expiration. */
function dismissApproval(id: string) {
  setApprovals((prev) => prev.filter((a) => a.id !== id));
}

const SamplingApprovalToast = () => {
  let eventUnlisten: UnlistenFn | undefined;
  const [busyIds, setBusyIds] = createSignal<Set<string>>(new Set());

  const respond = async (id: string, approved: boolean) => {
    if (busyIds().has(id)) return;
    setBusyIds((prev) => { const next = new Set(prev); next.add(id); return next; });
    try {
      const res = await authFetch('/api/v1/mcp/sampling/approve', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ request_id: id, approved }),
      });
      if (!res.ok) {
        const text = await res.text();
        logError(`Failed to respond to sampling approval: ${text}`);
      } else {
        logInfo(`Sampling request ${approved ? 'approved' : 'denied'}`);
        dismissApproval(id);
      }
    } catch (e: any) {
      logError(`Error responding to sampling approval: ${e?.message || e}`);
    } finally {
      setBusyIds((prev) => { const next = new Set(prev); next.delete(id); return next; });
    }
  };

  // Subscribe to MCP events for sampling approvals.
  const setup = async () => {
    // Ensure MCP event subscription is active.
    invoke('mcp_subscribe_events').catch(() => {});

    // Fetch current pending approvals as initial snapshot.
    try {
      const res = await authFetch('/api/v1/mcp/sampling/pending');
      if (res.ok) {
        const data = await res.json();
        if (data.pending) setApprovals(data.pending);
      }
    } catch { /* ignore */ }

    eventUnlisten = await listen('mcp:event', (event) => {
      try {
        const envelope = JSON.parse(event.payload as string);
        if (envelope.topic !== 'mcp.sampling.approval') return;
        const payload = envelope.payload as SamplingApprovalEvent;

        if (payload.type === 'requested') {
          setApprovals((prev) => [
            ...prev,
            {
              id: payload.id,
              server_id: payload.server_id || 'unknown',
              message_count: payload.message_count || 0,
              max_tokens: payload.max_tokens || 0,
              model_hint: payload.model_hint || null,
              preview: payload.preview || '',
              remaining_secs: payload.timeout_secs || 30,
            },
          ]);
        } else if (payload.type === 'resolved' || payload.type === 'expired') {
          dismissApproval(payload.id);
        }
      } catch { /* ignore parse errors */ }
    });
  };

  setup();

  onCleanup(() => {
    eventUnlisten?.();
  });

  // Countdown timer — tick each second to update remaining time.
  const interval = setInterval(() => {
    setApprovals((prev) =>
      prev
        .map((a) => ({ ...a, remaining_secs: a.remaining_secs - 1 }))
        .filter((a) => a.remaining_secs > 0),
    );
  }, 1000);
  onCleanup(() => clearInterval(interval));

  return (
    <Show when={approvals().length > 0}>
      <div class="sampling-approval-toasts" style="position: fixed; bottom: 1rem; right: 1rem; z-index: 9999; display: flex; flex-direction: column; gap: 0.5rem; max-width: 400px;">
        <For each={approvals()}>
          {(approval) => (
            <div
              class="sampling-approval-toast"
              style="background: hsl(var(--card)); border: 1px solid hsl(var(--border)); border-radius: 0.5rem; padding: 0.75rem 1rem; box-shadow: 0 4px 12px rgba(0,0,0,0.15);"
            >
              <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
                <Cpu size={16} style="color: hsl(var(--primary))" />
                <span style="font-weight: 600; font-size: 0.85rem;">
                  MCP Sampling Request
                </span>
                <span style="margin-left: auto; font-size: 0.75rem; color: hsl(var(--muted-foreground));">
                  {approval.remaining_secs}s
                </span>
              </div>
              <div style="font-size: 0.8rem; color: hsl(var(--muted-foreground)); margin-bottom: 0.25rem;">
                Server: <strong>{approval.server_id}</strong>
                {' · '}
                {approval.message_count} message{approval.message_count !== 1 ? 's' : ''}
                {' · '}
                max {approval.max_tokens} tokens
              </div>
              <Show when={approval.model_hint}>
                <div style="font-size: 0.75rem; color: hsl(var(--muted-foreground));">
                  Model hint: {approval.model_hint}
                </div>
              </Show>
              <Show when={approval.preview}>
                <div style="font-size: 0.75rem; color: hsl(var(--foreground)); margin-top: 0.25rem; font-style: italic; max-height: 3rem; overflow: hidden; text-overflow: ellipsis;">
                  "{approval.preview}"
                </div>
              </Show>
              <div style="display: flex; gap: 0.5rem; margin-top: 0.5rem;">
                <Button
                  size="sm"
                  variant="default"
                  disabled={busyIds().has(approval.id)}
                  onClick={() => respond(approval.id, true)}
                >
                  <CheckCircle size={14} />
                  Approve
                </Button>
                <Button
                  size="sm"
                  variant="destructive"
                  disabled={busyIds().has(approval.id)}
                  onClick={() => respond(approval.id, false)}
                >
                  <XCircle size={14} />
                  Deny
                </Button>
              </div>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
};

export default SamplingApprovalToast;
