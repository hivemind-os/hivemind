import { Show, createSignal, For } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { Accessor } from 'solid-js';
import type { Persona } from '../types';
import type { BotStore } from '../stores/botStore';
import { ConfirmDialog } from '~/ui/confirm-dialog';

interface BotConfigPanelProps {
  bot_id: string;
  botStore: BotStore;
  personas?: Accessor<Persona[]>;
}

/**
 * Configuration panel for bot sessions — shown in the "Config" tab when
 * viewing a bot's backing session in the unified session view.
 */
export default function BotConfigPanel(props: BotConfigPanelProps) {
  const bot = () => props.botStore.bots().find(b => b.config.id === props.bot_id);
  const config = () => bot()?.config;
  const status = () => bot()?.status ?? 'unknown';

  const [actionBusy, setActionBusy] = createSignal(false);
  const [actionError, setActionError] = createSignal<string | null>(null);
  const [confirmDeleteOpen, setConfirmDeleteOpen] = createSignal(false);

  const handleDeactivate = async () => {
    setActionBusy(true);
    setActionError(null);
    try {
      await invoke('deactivate_bot', { agent_id: props.bot_id });
    } catch (e: any) {
      setActionError(`Deactivate failed: ${e?.message ?? e}`);
    } finally {
      setActionBusy(false);
    }
  };

  const handleActivate = async () => {
    setActionBusy(true);
    setActionError(null);
    try {
      await invoke('activate_bot', { agent_id: props.bot_id });
    } catch (e: any) {
      setActionError(`Activate failed: ${e?.message ?? e}`);
    } finally {
      setActionBusy(false);
    }
  };

  const handleDelete = async () => {
    setActionBusy(true);
    setActionError(null);
    try {
      await invoke('delete_bot', { agent_id: props.bot_id });
    } catch (e: any) {
      setActionError(`Delete failed: ${e?.message ?? e}`);
    } finally {
      setActionBusy(false);
    }
  };

  const statusColor = () => {
    switch (status()) {
      case 'active': return 'bg-green-400';
      case 'spawning': case 'waiting': case 'paused': return 'bg-yellow-400';
      case 'error': case 'blocked': return 'bg-red-400';
      default: return 'bg-slate-600';
    }
  };

  return (
    <div class="flex flex-col gap-6 p-4 overflow-y-auto" style="max-width: 640px;">
      {/* Status */}
      <section>
        <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Status</h3>
        <div class="flex items-center gap-2">
          <span class={`inline-block size-3 rounded-full ${statusColor()}`} />
          <span class="text-sm font-medium capitalize">{status()}</span>
        </div>
        <Show when={bot()?.last_error}>
          <p class="mt-1 text-xs text-red-400">{bot()!.last_error}</p>
        </Show>
      </section>

      {/* Identity */}
      <Show when={config()}>
        {(cfg) => (
          <>
            <section>
              <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Identity</h3>
              <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
                <dt class="text-muted-foreground">Name</dt>
                <dd>{cfg().friendly_name}</dd>
                <Show when={cfg().avatar}>
                  <dt class="text-muted-foreground">Avatar</dt>
                  <dd>{cfg().avatar}</dd>
                </Show>
                <Show when={cfg().description}>
                  <dt class="text-muted-foreground">Description</dt>
                  <dd class="whitespace-pre-wrap">{cfg().description}</dd>
                </Show>
                <dt class="text-muted-foreground">ID</dt>
                <dd class="font-mono text-xs">{cfg().id}</dd>
                <Show when={cfg().persona_id}>
                  <dt class="text-muted-foreground">Persona</dt>
                  <dd>{(() => {
                    const pid = cfg().persona_id!;
                    const p = (props.personas?.() ?? []).find((pp) => pp.id === pid);
                    return p ? `${p.name} (${pid})` : pid;
                  })()}</dd>
                </Show>
              </dl>
            </section>

            {/* Execution */}
            <section>
              <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Execution</h3>
              <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
                <dt class="text-muted-foreground">Model</dt>
                <dd class="font-mono text-xs">{cfg().model}</dd>
                <dt class="text-muted-foreground">Mode</dt>
                <dd class="capitalize">{cfg().mode.replace('_', ' ')}</dd>
                <Show when={cfg().timeout_secs}>
                  <dt class="text-muted-foreground">Timeout</dt>
                  <dd>{cfg().timeout_secs}s</dd>
                </Show>
                <dt class="text-muted-foreground">Active</dt>
                <dd>{cfg().active ? 'Yes' : 'No'}</dd>
              </dl>
            </section>

            {/* Role / System Prompt */}
            <Show when={cfg().role}>
              <section>
                <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">System Prompt</h3>
                <pre class="max-h-40 overflow-y-auto rounded-md bg-muted p-2 text-xs whitespace-pre-wrap">{cfg().role}</pre>
              </section>
            </Show>

            {/* Launch Prompt */}
            <Show when={cfg().launch_prompt}>
              <section>
                <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Launch Prompt</h3>
                <pre class="max-h-40 overflow-y-auto rounded-md bg-muted p-2 text-xs whitespace-pre-wrap">{cfg().launch_prompt}</pre>
              </section>
            </Show>

            {/* Permissions */}
            <Show when={cfg().allowed_tools && cfg().allowed_tools!.length > 0}>
              <section>
                <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Allowed Tools</h3>
                <div class="flex flex-wrap gap-1">
                  <For each={cfg().allowed_tools}>
                    {(tool) => (
                      <span class="rounded-full bg-muted px-2 py-0.5 text-xs">{tool}</span>
                    )}
                  </For>
                </div>
              </section>
            </Show>

            {/* Actions */}
            <section>
              <h3 class="mb-2 text-sm font-semibold text-muted-foreground uppercase tracking-wide">Actions</h3>
              <Show when={actionError()}>
                <p class="mb-2 text-xs text-red-400">{actionError()}</p>
              </Show>
              <div class="flex gap-2">
                <Show when={cfg().active} fallback={
                  <button
                    class="rounded-md bg-green-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-green-500 disabled:opacity-50"
                    disabled={actionBusy()}
                    onClick={handleActivate}
                  >
                    Activate
                  </button>
                }>
                  <button
                    class="rounded-md bg-yellow-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-yellow-500 disabled:opacity-50"
                    disabled={actionBusy()}
                    onClick={handleDeactivate}
                  >
                    Deactivate
                  </button>
                </Show>
                <button
                  class="rounded-md bg-red-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-red-500 disabled:opacity-50"
                  disabled={actionBusy()}
                  onClick={() => setConfirmDeleteOpen(true)}
                >
                  Delete
                </button>
              </div>
            </section>
          </>
        )}
      </Show>

      <ConfirmDialog
        open={confirmDeleteOpen()}
        onOpenChange={setConfirmDeleteOpen}
        title={`Delete "${config()?.friendly_name}"?`}
        description="This cannot be undone."
        confirmLabel="Delete"
        variant="destructive"
        onConfirm={() => void handleDelete()}
      />
    </div>
  );
}
