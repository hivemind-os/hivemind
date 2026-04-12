import { For, Show, createSignal, onCleanup, onMount } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { CheckCircle, XCircle, X, Lock } from 'lucide-solid';
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '~/ui/collapsible';
import { logError, logInfo, logWarn } from './ActivityLog';
import { highlightYaml } from './YamlHighlight';
import { Button } from '~/ui';
import { respondToApproval as routeRespondToApproval, type PendingInteraction } from '~/lib/interactionRouting';

/** Payload shape for approval:event */
interface ApprovalEventPayload {
  type: 'added' | 'question_added' | 'resolved';
  session_id: string;
  agent_id: string;
  agent_name?: string;
  request_id: string;
  tool_id: string;
  input?: string;
  reason?: string;
}

/** Payload shape for approval:error */
interface ApprovalErrorPayload {
  error?: string;
}

export interface PendingApproval {
  session_id: string;
  agent_id: string;
  agent_name: string;
  request_id: string;
  tool_id: string;
  input: string;
  reason: string;
}

const [toasts, setToasts] = createSignal<PendingApproval[]>([]);

/** Read-only accessor for pending approval toasts. */
export const pendingApprovalToasts = toasts;

/** Dismiss a toast after the user responds. */
export function dismissAgentApproval(request_id: string) {
  setToasts((prev) => prev.filter((t) => t.request_id !== request_id));
}

const AgentApprovalToast = () => {
  let eventUnlisten: UnlistenFn | undefined;
  let errorUnlisten: UnlistenFn | undefined;
  const [expandedIds, setExpandedIds] = createSignal<Set<string>>(new Set());
  const [busyIds, setBusyIds] = createSignal<Set<string>>(new Set());
  const [errorMap, setErrorMap] = createSignal<Map<string, string>>(new Map());

  const toggleExpanded = (request_id: string) => {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(request_id)) next.delete(request_id);
      else next.add(request_id);
      return next;
    });
  };

  const respond = async (toast: PendingApproval, approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => {
    if (busyIds().has(toast.request_id)) return;
    setBusyIds((prev) => { const next = new Set(prev); next.add(toast.request_id); return next; });
    setErrorMap((prev) => { const next = new Map(prev); next.delete(toast.request_id); return next; });
    try {
      const session_id = toast.session_id && toast.session_id !== '__bot__' && toast.session_id !== '__service__'
        ? toast.session_id : undefined;
      await routeRespondToApproval(
        {
          request_id: toast.request_id,
          entity_id: `agent/${toast.agent_id}`,
          source_name: toast.agent_name || toast.agent_id,
          type: 'tool_approval',
          session_id: session_id,
          agent_id: toast.agent_id,
        } as PendingInteraction,
        { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
      );
      const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
      logInfo('approval', `${approved ? 'Approved' : 'Denied'}${scope} ${toast.tool_id} for ${toast.agent_name || toast.agent_id}`);
      setToasts((prev) => prev.filter((t) => t.request_id !== toast.request_id));
    } catch (err: any) {
      const detail = typeof err === 'string' ? err : err?.message ?? String(err);
      logError('approval', `Failed to ${approved ? 'approve' : 'deny'} ${toast.tool_id}: ${detail}`);
      setErrorMap((prev) => { const next = new Map(prev); next.set(toast.request_id, detail); return next; });
    } finally {
      setBusyIds((prev) => { const next = new Set(prev); next.delete(toast.request_id); return next; });
    }
  };

  const dismiss = (toast: PendingApproval) => {
    setToasts((prev) => prev.filter((t) => t.request_id !== toast.request_id));
    logInfo('approval', `Dismissed approval for ${toast.tool_id} (${toast.agent_name || toast.agent_id})`);
  };

  const dismissAll = () => {
    logInfo('approval', `Dismissed all ${toasts().length} pending approvals`);
    setToasts([]);
  };

  const approveAll = async () => {
    const current = toasts();
    const allIds = current.map((t) => t.request_id);
    setBusyIds(new Set(allIds));
    setErrorMap(new Map<string, string>());
    const failed: PendingApproval[] = [];
    const failedErrors = new Map<string, string>();
    for (const toast of current) {
      try {
        const session_id = toast.session_id && toast.session_id !== '__bot__' && toast.session_id !== '__service__'
          ? toast.session_id : undefined;
        await routeRespondToApproval(
          {
            request_id: toast.request_id,
            entity_id: `agent/${toast.agent_id}`,
            source_name: toast.agent_name || toast.agent_id,
            type: 'tool_approval',
            session_id: session_id,
            agent_id: toast.agent_id,
          } as PendingInteraction,
          { approved: true },
        );
        logInfo('approval', `Approved (all) ${toast.tool_id} for ${toast.agent_name || toast.agent_id}`);
      } catch (err: any) {
        const detail = typeof err === 'string' ? err : err?.message ?? String(err);
        logError('approval', `Failed to approve ${toast.tool_id}: ${detail}`);
        failed.push(toast);
        failedErrors.set(toast.request_id, detail);
      }
    }
    // Keep only the toasts that failed; remove successfully approved ones
    const failedIds = new Set(failed.map((t) => t.request_id));
    setToasts((prev) => prev.filter((t) => failedIds.has(t.request_id)));
    setErrorMap(failedErrors);
    setBusyIds(new Set<string>());
  };

  // Register cleanup BEFORE the async listen() calls to avoid the race
  // where the component unmounts before the await resolves.
  let disposed = false;
  onCleanup(() => {
    disposed = true;
    eventUnlisten?.();
    errorUnlisten?.();
  });

  onMount(async () => {
    try {
      await invoke('subscribe_approval_stream');
      if (!disposed) logInfo('approval', 'Connected to approval event stream');
    } catch (err: any) {
      if (!disposed) logWarn('approval', `Failed to start approval stream: ${err}`);
    }

    const evUn = await listen<ApprovalEventPayload>('approval:event', (e) => {
      if (disposed) return;
      const event = e.payload;
      if (event.type === 'added') {
        setToasts((prev) => {
          if (prev.some((t) => t.request_id === event.request_id)) return prev;
          return [...prev, {
            session_id: event.session_id,
            agent_id: event.agent_id,
            agent_name: event.agent_name ?? event.agent_id,
            request_id: event.request_id,
            tool_id: event.tool_id,
            input: event.input ?? '',
            reason: event.reason ?? 'Tool requires user approval.',
          }];
        });
      } else if (event.type === 'resolved') {
        setToasts((prev) => prev.filter((t) => t.request_id !== event.request_id));
      }
    });
    if (disposed) { evUn(); } else { eventUnlisten = evUn; }

    const errUn = await listen<ApprovalErrorPayload>('approval:error', (e) => {
      if (!disposed) logError('approval', `Approval stream error: ${e.payload?.error}`);
    });
    if (disposed) { errUn(); } else { errorUnlisten = errUn; }
  });

  return (
    <div class="pointer-events-none fixed right-4 top-4 z-[9999] flex max-w-[380px] flex-col gap-2.5">
      <Show when={toasts().length > 1}>
        <div class="pointer-events-auto animate-in slide-in-from-right rounded-[10px] border border-primary/40 bg-card p-3 shadow-lg">
          <div class="mb-1 flex items-center gap-1.5 text-sm font-semibold text-primary">
            <Lock size={14} /> <strong>{toasts().length} pending approvals</strong>
          </div>
          <div class="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              class="flex-1 border-green-500/40 bg-green-500/10 text-green-400 hover:bg-green-500/25"
              onClick={(e: MouseEvent) => { e.stopPropagation(); void approveAll(); }}
            >
              ✅ Approve All
            </Button>
            <Button
              size="sm"
              variant="outline"
              class="flex-1 border-muted-foreground/30 text-muted-foreground hover:bg-muted/40"
              onClick={(e: MouseEvent) => { e.stopPropagation(); dismissAll(); }}
            >
              Dismiss All
            </Button>
          </div>
        </div>
      </Show>
      <For each={toasts()}>
        {(toast) => (
          <div class={`pointer-events-auto animate-in slide-in-from-right rounded-[10px] border p-3 shadow-lg bg-card ${errorMap().has(toast.request_id) ? 'border-red-500/60' : 'border-orange-400/40'}`}>
            <div class="mb-1 flex items-center gap-1.5 text-sm font-semibold text-orange-400">
              <Lock size={14} /> <strong>{toast.agent_name || toast.agent_id}</strong> needs approval
              <button
                class="ml-auto cursor-pointer border-none bg-transparent p-0.5 text-muted-foreground hover:text-foreground"
                onClick={(e: MouseEvent) => { e.stopPropagation(); dismiss(toast); }}
                title="Dismiss"
              >
                <X size={14} />
              </button>
            </div>
            {errorMap().has(toast.request_id) && (
              <p class="mb-1 text-xs text-red-400" title={errorMap().get(toast.request_id)}>
                Failed: {(() => {
                  const msg = errorMap().get(toast.request_id) ?? '';
                  return msg.length > 120 ? msg.slice(0, 120) + '…' : msg;
                })()}
              </p>
            )}
            <div class="mb-2 flex flex-col gap-0.5">
              <span class="font-mono text-xs text-blue-400">{toast.tool_id}</span>
              <span class="text-[0.75rem] leading-relaxed text-muted-foreground">{toast.reason}</span>
            </div>
            <Collapsible
              open={expandedIds().has(toast.request_id)}
              onOpenChange={(open) => {
                const next = new Set(expandedIds());
                if (open) next.add(toast.request_id); else next.delete(toast.request_id);
                setExpandedIds(next);
              }}
            >
              <CollapsibleTrigger
                as="button"
                class="mb-1 cursor-pointer border-none bg-transparent p-0 text-[0.72rem] text-muted-foreground hover:text-foreground"
              >
                {expandedIds().has(toast.request_id) ? '▾ Hide details' : '▸ Show details'}
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre class="yaml-block mb-1.5 max-h-[150px] overflow-auto whitespace-pre-wrap break-all rounded-md p-2 text-[0.7rem]" innerHTML={highlightYaml(toast.input || '(no input)')} />
              </CollapsibleContent>
            </Collapsible>
            <div class="flex flex-col gap-1.5">
              <div class="flex gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  class="flex-1 border-green-500/40 bg-green-500/10 text-green-400 hover:bg-green-500/25"
                  disabled={busyIds().has(toast.request_id)}
                  onClick={(e: MouseEvent) => { e.stopPropagation(); void respond(toast, true); }}
                >
                  <CheckCircle size={14} /> Approve
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  class="flex-1 border-red-400/40 bg-red-400/10 text-red-400 hover:bg-red-400/25"
                  disabled={busyIds().has(toast.request_id)}
                  onClick={(e: MouseEvent) => { e.stopPropagation(); void respond(toast, false); }}
                >
                  <XCircle size={14} /> Deny
                </Button>
              </div>
              <div class="flex gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  class="flex-1 border-blue-400/40 bg-blue-400/10 text-blue-400 hover:bg-blue-400/25 text-[0.7rem]"
                  disabled={busyIds().has(toast.request_id)}
                  onClick={(e: MouseEvent) => { e.stopPropagation(); void respond(toast, true, { allow_agent: true }); }}
                >
                  Allow for Agent
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  class="flex-1 border-purple-400/40 bg-purple-400/10 text-purple-400 hover:bg-purple-400/25 text-[0.7rem]"
                  disabled={busyIds().has(toast.request_id)}
                  onClick={(e: MouseEvent) => { e.stopPropagation(); void respond(toast, true, { allow_session: true }); }}
                >
                  Allow for Session
                </Button>
              </div>
            </div>
          </div>
        )}
      </For>
    </div>
  );
};

export default AgentApprovalToast;
