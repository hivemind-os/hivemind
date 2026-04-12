import { Component, For, Show } from 'solid-js';
import type { AgentKitStore } from '../stores/agentKitStore';
import { AlertTriangle, CheckCircle, XCircle, Package, ArrowLeft } from 'lucide-solid';

interface Props {
  store: AgentKitStore;
}

const AgentKitImportPreview: Component<Props> = (props) => {
  const preview = () => props.store.importPreview()!;
  const hasErrors = () => preview().errors.length > 0;

  const newItems = () => preview().items.filter(i => !i.overwrites_existing);
  const overwriteItems = () => preview().items.filter(i => i.overwrites_existing);

  return (
    <div class="space-y-4">
      {/* Header */}
      <div class="flex items-center gap-3">
        <button
          class="p-1 rounded hover:bg-muted"
          onClick={() => { props.store.resetImport(); }}
          title="Back"
        >
          <ArrowLeft size={16} />
        </button>
        <Package size={20} class="text-primary" />
        <div>
          <h2 class="text-lg font-semibold">Import Preview: {preview().manifest.name}</h2>
          <p class="text-xs text-muted-foreground">
            Target namespace: <span class="font-mono">{preview().target_namespace}/</span>
          </p>
        </div>
      </div>

      <Show when={preview().manifest.description}>
        <p class="text-sm text-muted-foreground">{preview().manifest.description}</p>
      </Show>

      {/* Errors */}
      <Show when={hasErrors()}>
        <div class="p-3 rounded-md bg-destructive/10 border border-destructive/20 space-y-1">
          <For each={preview().errors}>
            {(err) => (
              <div class="flex items-center gap-2 text-sm text-destructive">
                <XCircle size={14} />
                <span>{err}</span>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* Warnings */}
      <Show when={preview().warnings.length > 0}>
        <div class="p-3 rounded-md bg-amber-500/10 border border-amber-500/20 space-y-1">
          <For each={preview().warnings}>
            {(warn) => (
              <div class="flex items-center gap-2 text-sm text-amber-600">
                <AlertTriangle size={14} />
                <span>{warn}</span>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* New items */}
      <Show when={newItems().length > 0}>
        <div>
          <h3 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide mb-2">
            New Items ({newItems().length})
          </h3>
          <div class="border border-border rounded-md">
            <For each={newItems()}>
              {(item) => (
                <label class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer border-b border-border last:border-b-0">
                  <input
                    type="checkbox"
                    checked={props.store.selectedImportItems().has(item.new_id)}
                    onChange={() => props.store.toggleImportItem(item.new_id)}
                    class="rounded"
                    disabled={hasErrors()}
                  />
                  <CheckCircle size={14} class="text-green-600" />
                  <span class="text-sm capitalize">{item.kind}</span>
                  <span class="text-sm font-mono text-muted-foreground">{item.original_id}</span>
                  <span class="text-xs text-muted-foreground">→</span>
                  <span class="text-sm font-mono">{item.new_id}</span>
                </label>
              )}
            </For>
          </div>
        </div>
      </Show>

      {/* Overwrite items */}
      <Show when={overwriteItems().length > 0}>
        <div>
          <h3 class="text-sm font-semibold text-amber-600 uppercase tracking-wide mb-2">
            <AlertTriangle size={14} class="inline mr-1" />
            Overwrites ({overwriteItems().length})
          </h3>
          <div class="border border-amber-500/30 rounded-md">
            <For each={overwriteItems()}>
              {(item) => (
                <label class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer border-b border-border last:border-b-0">
                  <input
                    type="checkbox"
                    checked={props.store.selectedImportItems().has(item.new_id)}
                    onChange={() => props.store.toggleImportItem(item.new_id)}
                    class="rounded"
                    disabled={hasErrors()}
                  />
                  <AlertTriangle size={14} class="text-amber-600" />
                  <span class="text-sm capitalize">{item.kind}</span>
                  <span class="text-sm font-mono text-muted-foreground">{item.original_id}</span>
                  <span class="text-xs text-muted-foreground">→</span>
                  <span class="text-sm font-mono">{item.new_id}</span>
                  <span class="text-xs text-amber-600 ml-auto">will overwrite</span>
                </label>
              )}
            </For>
          </div>
        </div>
      </Show>

      {/* Actions */}
      <div class="flex items-center gap-3 pt-2">
        <button
          class="px-4 py-2 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={hasErrors() || props.store.importing() || props.store.selectedImportItems().size === 0}
          onClick={() => props.store.applyImport()}
        >
          {props.store.importing() ? 'Importing...' : `Import ${props.store.selectedImportItems().size} Items`}
        </button>
        <button
          class="px-3 py-2 rounded-md border border-input text-sm hover:bg-muted"
          onClick={() => props.store.resetImport()}
        >
          Cancel
        </button>
      </div>

      <Show when={props.store.importError()}>
        <div class="flex items-center gap-2 p-3 rounded-md bg-destructive/10 text-destructive text-sm">
          <XCircle size={14} />
          {props.store.importError()}
        </div>
      </Show>
    </div>
  );
};

export default AgentKitImportPreview;
