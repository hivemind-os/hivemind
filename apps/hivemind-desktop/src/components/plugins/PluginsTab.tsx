import { createSignal, createResource, For, Show, lazy, Suspense, type Component } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Plug, Settings, Trash2, Power, PowerOff, FolderOpen, RefreshCw, Globe } from 'lucide-solid';
import PluginConfigForm, { type PluginConfigSchema } from './PluginConfigForm';

const PluginBrowser = lazy(() => import('./PluginBrowser'));

/** Installed plugin info returned from the backend. */
interface InstalledPlugin {
  plugin_id: string;
  name: string;
  version: string;
  display_name: string;
  description: string;
  plugin_type: string;
  enabled: boolean;
  config: Record<string, any>;
  config_schema?: PluginConfigSchema | null;
  status?: { state: string; message?: string };
  permissions: string[];
}

const PluginsTab: Component = () => {
  const [plugins, { refetch }] = createResource(fetchPlugins);
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const [selectedPlugin, setSelectedPlugin] = createSignal<InstalledPlugin | null>(null);
  const [configSchema, setConfigSchema] = createSignal<PluginConfigSchema | null>(null);
  const [editConfig, setEditConfig] = createSignal<Record<string, any>>({});
  const [saving, setSaving] = createSignal(false);
  const [linkPath, setLinkPath] = createSignal('');
  const [linking, setLinking] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [viewMode, setViewMode] = createSignal<'installed' | 'browse'>('installed');

  async function fetchPlugins(): Promise<InstalledPlugin[]> {
    try {
      return await invoke<InstalledPlugin[]>('plugin_list');
    } catch {
      return [];
    }
  }

  async function selectPlugin(id: string) {
    const plugin = plugins()?.find(p => p.plugin_id === id) ?? null;
    setSelectedId(id);
    setSelectedPlugin(plugin);
    setError(null);
    if (plugin) {
      setEditConfig({ ...plugin.config });
      // Use inline schema from plugin list (extracted at build time)
      if (plugin.config_schema) {
        setConfigSchema(plugin.config_schema);
      } else {
        // Fall back to querying daemon (requires running plugin process)
        try {
          const schema = await invoke<PluginConfigSchema>('plugin_get_config_schema', { plugin_id: id });
          setConfigSchema(schema);
        } catch (e) {
          setConfigSchema(null);
        }
      }
    } else {
      setConfigSchema(null);
    }
  }

  async function saveConfig() {
    const id = selectedId();
    if (!id) return;
    setSaving(true);
    setError(null);
    try {
      await invoke('plugin_save_config', { plugin_id: id, config: editConfig() });
      await refetch();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setSaving(false);
    }
  }

  async function toggleEnabled(id: string, enabled: boolean) {
    setError(null);
    try {
      await invoke('plugin_set_enabled', { plugin_id: id, enabled });
      await refetch();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    }
  }

  async function uninstallPlugin(id: string) {
    if (!confirm(`Uninstall plugin "${id}"?`)) return;
    setError(null);
    try {
      await invoke('plugin_uninstall', { plugin_id: id });
      if (selectedId() === id) {
        setSelectedId(null);
        setSelectedPlugin(null);
      }
      await refetch();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    }
  }

  async function linkLocal() {
    const path = linkPath().trim();
    if (!path) return;
    setLinking(true);
    setError(null);
    try {
      await invoke('plugin_link_local', { path });
      setLinkPath('');
      await refetch();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setLinking(false);
    }
  }

  function statusBadge(state?: string) {
    if (!state) return null;
    const colors: Record<string, string> = {
      connected: 'bg-green-500/20 text-green-400',
      connecting: 'bg-yellow-500/20 text-yellow-400',
      syncing: 'bg-blue-500/20 text-blue-400',
      disconnected: 'bg-muted text-muted-foreground',
      error: 'bg-destructive/20 text-destructive',
    };
    return (
      <span class={`text-xs px-1.5 py-0.5 rounded ${colors[state] ?? colors.disconnected}`}>
        {state}
      </span>
    );
  }

  return (
    <div class="settings-tab-content">
      <h3 class="settings-section-title">Plugins</h3>
      <p class="text-sm text-muted-foreground mb-4">
        Install and manage TypeScript connector plugins. Plugins provide tools, background sync, and integrations.
      </p>

      {/* View mode toggle */}
      <div class="flex gap-1 bg-muted rounded-md p-1 mb-4 w-fit">
        <button
          onClick={() => setViewMode('installed')}
          class={`px-3 py-1 rounded text-sm ${viewMode() === 'installed' ? 'bg-background shadow-sm font-medium' : 'text-muted-foreground hover:text-foreground'}`}
        >
          <Plug size={14} class="inline mr-1.5" />Installed
        </button>
        <button
          onClick={() => setViewMode('browse')}
          class={`px-3 py-1 rounded text-sm ${viewMode() === 'browse' ? 'bg-background shadow-sm font-medium' : 'text-muted-foreground hover:text-foreground'}`}
        >
          <Globe size={14} class="inline mr-1.5" />Browse
        </button>
      </div>

      <Show when={error()}>
        <div class="bg-destructive/10 border border-destructive/30 rounded-md p-2 mb-3 text-sm text-destructive">
          {error()}
        </div>
      </Show>

      {/* Browse view */}
      <Show when={viewMode() === 'browse'}>
        <Suspense fallback={<p class="text-sm text-muted-foreground">Loading...</p>}>
          <PluginBrowser />
        </Suspense>
      </Show>

      {/* Installed view */}
      <Show when={viewMode() === 'installed'}>

      {/* Link local plugin */}
      <div class="flex gap-2 mb-4">
        <input
          type="text"
          value={linkPath()}
          onInput={(e) => setLinkPath(e.currentTarget.value)}
          placeholder="Path to local plugin directory..."
          class="flex-1 rounded-md border border-input bg-background px-3 py-1.5 text-sm"
        />
        <button
          onClick={linkLocal}
          disabled={linking() || !linkPath().trim()}
          class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md bg-primary text-primary-foreground text-sm hover:bg-primary/90 disabled:opacity-50"
        >
          <FolderOpen size={14} />
          {linking() ? 'Linking...' : 'Link Local'}
        </button>
        <button
          onClick={() => refetch()}
          class="inline-flex items-center gap-1 px-2 py-1.5 rounded-md border border-input text-sm hover:bg-muted"
          title="Refresh"
        >
          <RefreshCw size={14} />
        </button>
      </div>

      <div class="flex gap-4" style="min-height: 300px">
        {/* Plugin list */}
        <div class="w-64 flex-shrink-0 border border-border rounded-md overflow-hidden">
          <Show when={plugins()?.length === 0}>
            <div class="p-4 text-sm text-muted-foreground text-center">
              <Plug size={24} class="mx-auto mb-2 opacity-50" />
              No plugins installed.
            </div>
          </Show>
          <For each={plugins()}>
            {(plugin) => (
              <button
                onClick={() => selectPlugin(plugin.plugin_id)}
                class={`w-full text-left p-3 border-b border-border hover:bg-muted/50 transition-colors ${
                  selectedId() === plugin.plugin_id ? 'bg-muted' : ''
                }`}
              >
                <div class="flex items-center justify-between">
                  <span class="text-sm font-medium truncate">{plugin.display_name}</span>
                  {statusBadge(plugin.status?.state)}
                </div>
                <div class="text-xs text-muted-foreground truncate mt-0.5">
                  {plugin.name} v{plugin.version}
                </div>
                <Show when={!plugin.enabled}>
                  <span class="text-xs text-muted-foreground italic">disabled</span>
                </Show>
              </button>
            )}
          </For>
        </div>

        {/* Plugin detail / config */}
        <div class="flex-1 min-w-0">
          <Show when={selectedPlugin()} fallback={
            <div class="flex items-center justify-center h-full text-sm text-muted-foreground">
              Select a plugin to configure
            </div>
          }>
            {(plugin) => (
              <div class="space-y-4">
                {/* Header */}
                <div class="flex items-start justify-between">
                  <div>
                    <h4 class="text-lg font-semibold">{plugin().display_name}</h4>
                    <p class="text-sm text-muted-foreground">{plugin().description}</p>
                    <div class="flex gap-2 mt-1 text-xs text-muted-foreground">
                      <span>{plugin().name}</span>
                      <span>v{plugin().version}</span>
                      <span class="capitalize">{plugin().plugin_type}</span>
                    </div>
                  </div>
                  <div class="flex gap-1.5">
                    <button
                      onClick={() => toggleEnabled(plugin().plugin_id, !plugin().enabled)}
                      class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs border border-input hover:bg-muted"
                      title={plugin().enabled ? 'Disable' : 'Enable'}
                    >
                      {plugin().enabled ? <PowerOff size={12} /> : <Power size={12} />}
                      {plugin().enabled ? 'Disable' : 'Enable'}
                    </button>
                    <button
                      onClick={() => uninstallPlugin(plugin().plugin_id)}
                      class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs border border-destructive/50 text-destructive hover:bg-destructive/10"
                      title="Uninstall"
                    >
                      <Trash2 size={12} /> Uninstall
                    </button>
                  </div>
                </div>

                {/* Status */}
                <Show when={plugin().status}>
                  <div class="flex items-center gap-2 text-sm">
                    <span class="text-muted-foreground">Status:</span>
                    {statusBadge(plugin().status?.state)}
                    <Show when={plugin().status?.message}>
                      <span class="text-muted-foreground">{plugin().status!.message}</span>
                    </Show>
                  </div>
                </Show>

                {/* Permissions */}
                <Show when={plugin().permissions.length > 0}>
                  <div>
                    <h5 class="text-sm font-medium mb-1">Permissions</h5>
                    <div class="flex flex-wrap gap-1">
                      <For each={plugin().permissions}>
                        {(perm) => (
                          <span class="text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground">
                            {perm}
                          </span>
                        )}
                      </For>
                    </div>
                  </div>
                </Show>

                {/* Config form */}
                <Show when={configSchema()}>
                  <div>
                    <h5 class="text-sm font-medium mb-2">Configuration</h5>
                    <PluginConfigForm
                      schema={configSchema()!}
                      values={editConfig()}
                      onChange={(key, value) => setEditConfig(prev => ({ ...prev, [key]: value }))}
                      disabled={!plugin().enabled}
                    />
                    <div class="flex justify-end mt-3">
                      <button
                        onClick={saveConfig}
                        disabled={saving()}
                        class="inline-flex items-center gap-1.5 px-4 py-1.5 rounded-md bg-primary text-primary-foreground text-sm hover:bg-primary/90 disabled:opacity-50"
                      >
                        <Settings size={14} />
                        {saving() ? 'Saving...' : 'Save Config'}
                      </button>
                    </div>
                  </div>
                </Show>
              </div>
            )}
          </Show>
        </div>
      </div>

      </Show> {/* end installed view */}
    </div>
  );
};

export default PluginsTab;
