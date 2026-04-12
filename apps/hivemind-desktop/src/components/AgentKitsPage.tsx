import { Component, For, Show, createSignal, createEffect, onMount } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import type { AgentKitStore } from '../stores/agentKitStore';
import type { Persona } from '../types';
import AgentKitImportPreview from './AgentKitImportPreview';
import { Package, Upload, Download, AlertTriangle, CheckCircle, XCircle } from 'lucide-solid';

interface Props {
  store: AgentKitStore;
}

const AgentKitsPage: Component<Props> = (props) => {
  const [personas, setPersonas] = createSignal<Persona[]>([]);
  const [workflows, setWorkflows] = createSignal<{ name: string; description?: string }[]>([]);
  const [activeSection, setActiveSection] = createSignal<'export' | 'import'>('export');

  onMount(async () => {
    try {
      const p = await invoke<Persona[]>('list_personas', { include_archived: false });
      setPersonas(p.filter(p => !p.id.startsWith('system/')));
    } catch { /* ignore */ }

    try {
      const defs = await invoke<{ name: string; description?: string }[]>('workflow_list_definitions');
      setWorkflows((defs || []).filter(d => !d.name.startsWith('system/')));
    } catch { /* ignore */ }
  });

  async function handleExport() {
    const result = await props.store.exportKit();
    if (!result) return;

    // Use save dialog to get file path, then write via Tauri command
    try {
      const filePath = await save({
        defaultPath: result.filename,
        filters: [{ name: 'Agent Kit', extensions: ['agentkit'] }],
      });
      if (filePath) {
        await invoke('agent_kit_save_file', { path: filePath, content: result.content });
      }
    } catch (e) {
      console.error('Failed to save file:', e);
    }
  }

  async function handleFileSelect() {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const filePath = await open({
        filters: [{ name: 'Agent Kit', extensions: ['agentkit'] }],
        multiple: false,
      });
      if (filePath && typeof filePath === 'string') {
        const filename = filePath.split('/').pop() || filePath.split('\\').pop() || 'file.agentkit';
        props.store.setImportFileName(filename);
        const content = await invoke<string>('agent_kit_read_file', { path: filePath });
        props.store.setImportFileContent(content);
      }
    } catch (e) {
      console.error('Failed to select file:', e);
    }
  }

  const canExport = () =>
    props.store.kitName().trim() !== '' &&
    (props.store.selectedPersonaIds().length > 0 || props.store.selectedWorkflowNames().length > 0);

  const canPreview = () =>
    props.store.importFileContent() !== null && props.store.targetNamespace().trim() !== '';

  return (
    <div class="flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div class="flex items-center gap-3 px-4 py-3 border-b border-border">
        <Package size={20} class="text-primary" />
        <h1 class="text-lg font-semibold">Agent Kits</h1>
      </div>

      {/* Tab bar */}
      <div class="flex border-b border-border">
        <button
          class={`px-4 py-2 text-sm font-medium transition-colors ${activeSection() === 'export' ? 'border-b-2 border-primary text-primary' : 'text-muted-foreground hover:text-foreground'}`}
          onClick={() => setActiveSection('export')}
        >
          <Download size={14} class="inline mr-1.5" />
          Export
        </button>
        <button
          class={`px-4 py-2 text-sm font-medium transition-colors ${activeSection() === 'import' ? 'border-b-2 border-primary text-primary' : 'text-muted-foreground hover:text-foreground'}`}
          onClick={() => setActiveSection('import')}
        >
          <Upload size={14} class="inline mr-1.5" />
          Import
        </button>
      </div>

      {/* Content */}
      <div class="flex-1 overflow-y-auto p-4">
        <Show when={activeSection() === 'export'}>
          <div class="space-y-6 max-w-2xl">
            {/* Kit metadata */}
            <div class="space-y-3">
              <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">Kit Details</h2>
              <input
                type="text"
                placeholder="Kit name (required)"
                value={props.store.kitName()}
                onInput={(e) => props.store.setKitName(e.currentTarget.value)}
                class="w-full px-3 py-2 rounded-md border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
              />
              <textarea
                placeholder="Description (optional)"
                value={props.store.kitDescription()}
                onInput={(e) => props.store.setKitDescription(e.currentTarget.value)}
                class="w-full px-3 py-2 rounded-md border border-input bg-background text-sm resize-none h-20 focus:outline-none focus:ring-2 focus:ring-ring"
              />
            </div>

            {/* Persona selection */}
            <div class="space-y-2">
              <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
                Personas ({props.store.selectedPersonaIds().length} selected)
              </h2>
              <div class="border border-border rounded-md max-h-48 overflow-y-auto">
                <Show when={personas().length === 0}>
                  <p class="p-3 text-sm text-muted-foreground">No user personas found</p>
                </Show>
                <For each={personas()}>
                  {(persona) => (
                    <label class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer border-b border-border last:border-b-0">
                      <input
                        type="checkbox"
                        checked={props.store.selectedPersonaIds().includes(persona.id)}
                        onChange={() => props.store.togglePersona(persona.id)}
                        class="rounded"
                      />
                      <span class="text-sm">{persona.name}</span>
                      <span class="text-xs text-muted-foreground ml-auto">{persona.id}</span>
                    </label>
                  )}
                </For>
              </div>
            </div>

            {/* Workflow selection */}
            <div class="space-y-2">
              <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
                Workflows ({props.store.selectedWorkflowNames().length} selected)
              </h2>
              <div class="border border-border rounded-md max-h-48 overflow-y-auto">
                <Show when={workflows().length === 0}>
                  <p class="p-3 text-sm text-muted-foreground">No user workflows found</p>
                </Show>
                <For each={workflows()}>
                  {(wf) => (
                    <label class="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 cursor-pointer border-b border-border last:border-b-0">
                      <input
                        type="checkbox"
                        checked={props.store.selectedWorkflowNames().includes(wf.name)}
                        onChange={() => props.store.toggleWorkflow(wf.name)}
                        class="rounded"
                      />
                      <span class="text-sm">{wf.name}</span>
                      <Show when={wf.description}>
                        <span class="text-xs text-muted-foreground ml-auto truncate max-w-[200px]">{wf.description}</span>
                      </Show>
                    </label>
                  )}
                </For>
              </div>
            </div>

            {/* Export button */}
            <div class="flex items-center gap-3">
              <button
                class="px-4 py-2 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={!canExport() || props.store.exporting()}
                onClick={handleExport}
              >
                {props.store.exporting() ? 'Exporting...' : 'Export Agent Kit'}
              </button>
              <button
                class="px-3 py-2 rounded-md border border-input text-sm hover:bg-muted"
                onClick={() => props.store.resetExport()}
              >
                Reset
              </button>
            </div>

            <Show when={props.store.exportError()}>
              <div class="flex items-center gap-2 p-3 rounded-md bg-destructive/10 text-destructive text-sm">
                <XCircle size={16} />
                {props.store.exportError()}
              </div>
            </Show>
          </div>
        </Show>

        <Show when={activeSection() === 'import'}>
          <div class="space-y-6 max-w-2xl">
            <Show when={!props.store.importPreview() && !props.store.importResult()}>
              {/* File selection */}
              <div class="space-y-3">
                <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">Select Agent Kit File</h2>
                <div class="flex items-center gap-3">
                  <button
                    class="px-4 py-2 rounded-md border border-input text-sm cursor-pointer hover:bg-muted"
                    onClick={() => void handleFileSelect()}
                  >
                    Choose File
                  </button>
                  <Show when={props.store.importFileName()}>
                    <span class="text-sm text-muted-foreground">{props.store.importFileName()}</span>
                  </Show>
                </div>
              </div>

              {/* Namespace */}
              <div class="space-y-3">
                <h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">Target Namespace</h2>
                <input
                  type="text"
                  placeholder="e.g. myteam"
                  value={props.store.targetNamespace()}
                  onInput={(e) => props.store.setTargetNamespace(e.currentTarget.value)}
                  class="w-full px-3 py-2 rounded-md border border-input bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring"
                />
                <p class="text-xs text-muted-foreground">
                  All items will be re-namespaced under this prefix. Cannot use "system".
                </p>
              </div>

              {/* Preview button */}
              <div class="flex items-center gap-3">
                <button
                  class="px-4 py-2 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
                  disabled={!canPreview() || props.store.previewing()}
                  onClick={() => props.store.previewImport()}
                >
                  {props.store.previewing() ? 'Analyzing...' : 'Preview Import'}
                </button>
                <button
                  class="px-3 py-2 rounded-md border border-input text-sm hover:bg-muted"
                  onClick={() => props.store.resetImport()}
                >
                  Reset
                </button>
              </div>

              <Show when={props.store.importError()}>
                <div class="flex items-center gap-2 p-3 rounded-md bg-destructive/10 text-destructive text-sm">
                  <XCircle size={16} />
                  {props.store.importError()}
                </div>
              </Show>
            </Show>

            {/* Preview dialog */}
            <Show when={props.store.importPreview()}>
              <AgentKitImportPreview store={props.store} />
            </Show>

            {/* Import result */}
            <Show when={props.store.importResult()}>
              {(result) => (
                <div class="space-y-4">
                  <div class="flex items-center gap-2 text-green-600">
                    <CheckCircle size={20} />
                    <h2 class="text-lg font-semibold">Import Complete</h2>
                  </div>

                  <Show when={result().imported_personas.length > 0}>
                    <div>
                      <h3 class="text-sm font-medium mb-1">Imported Personas</h3>
                      <For each={result().imported_personas}>
                        {(item) => (
                          <div class="flex items-center gap-2 text-sm py-1">
                            <CheckCircle size={14} class="text-green-600" />
                            <span>{item.new_id}</span>
                            <Show when={item.overwritten}>
                              <span class="text-xs text-amber-600">(overwritten)</span>
                            </Show>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>

                  <Show when={result().imported_workflows.length > 0}>
                    <div>
                      <h3 class="text-sm font-medium mb-1">Imported Workflows</h3>
                      <For each={result().imported_workflows}>
                        {(item) => (
                          <div class="flex items-center gap-2 text-sm py-1">
                            <CheckCircle size={14} class="text-green-600" />
                            <span>{item.new_id}</span>
                            <Show when={item.overwritten}>
                              <span class="text-xs text-amber-600">(overwritten)</span>
                            </Show>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>

                  <Show when={result().errors.length > 0}>
                    <div>
                      <h3 class="text-sm font-medium mb-1 text-destructive">Errors</h3>
                      <For each={result().errors}>
                        {(err) => (
                          <div class="flex items-center gap-2 text-sm py-1 text-destructive">
                            <XCircle size={14} />
                            <span>{err.item_id}: {err.message}</span>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>

                  <button
                    class="px-4 py-2 rounded-md border border-input text-sm hover:bg-muted"
                    onClick={() => props.store.resetImport()}
                  >
                    Import Another
                  </button>
                </div>
              )}
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
};

export default AgentKitsPage;
