import { createSignal, createResource, For, Show, type Component } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Search, Download, CheckCircle, Star, ExternalLink } from 'lucide-solid';

interface RegistryPlugin {
  name: string;
  displayName: string;
  description: string;
  npmPackage: string;
  categories: string[];
  verified: boolean;
  featured: boolean;
  icon?: string;
  author: string;
  repository?: string;
  license?: string;
  pluginType: string;
}

interface RegistryData {
  version: number;
  categories: { id: string; label: string; description: string }[];
  plugins: RegistryPlugin[];
}

const REGISTRY_URL = 'https://raw.githubusercontent.com/hivemind-os/hivemind/main/packages/plugin-registry/registry.json';

const PluginBrowser: Component = () => {
  const [registry] = createResource(fetchRegistry);
  const [search, setSearch] = createSignal('');
  const [selectedCategory, setSelectedCategory] = createSignal<string | null>(null);
  const [installing, setInstalling] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  async function fetchRegistry(): Promise<RegistryData | null> {
    try {
      const res = await fetch(REGISTRY_URL);
      if (!res.ok) return null;
      return await res.json();
    } catch {
      return null;
    }
  }

  const filteredPlugins = () => {
    const data = registry();
    if (!data) return [];
    let plugins = data.plugins;

    const q = search().toLowerCase().trim();
    if (q) {
      plugins = plugins.filter(
        (p) =>
          p.displayName.toLowerCase().includes(q) ||
          p.description.toLowerCase().includes(q) ||
          p.name.toLowerCase().includes(q) ||
          p.author.toLowerCase().includes(q)
      );
    }

    const cat = selectedCategory();
    if (cat) {
      plugins = plugins.filter((p) => p.categories.includes(cat));
    }

    // Featured first, then alphabetical
    return plugins.sort((a, b) => {
      if (a.featured !== b.featured) return a.featured ? -1 : 1;
      return a.displayName.localeCompare(b.displayName);
    });
  };

  async function installPlugin(npmPackage: string) {
    setInstalling(npmPackage);
    setError(null);
    try {
      await invoke('plugin_install_npm', { packageName: npmPackage });
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setInstalling(null);
    }
  }

  return (
    <div class="space-y-4">
      <h4 class="text-sm font-semibold">Browse Plugins</h4>

      <Show when={error()}>
        <div class="bg-destructive/10 border border-destructive/30 rounded-md p-2 text-sm text-destructive">
          {error()}
        </div>
      </Show>

      {/* Search bar */}
      <div class="relative">
        <Search size={14} class="absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground" />
        <input
          type="text"
          value={search()}
          onInput={(e) => setSearch(e.currentTarget.value)}
          placeholder="Search plugins..."
          class="w-full pl-9 pr-3 py-1.5 rounded-md border border-input bg-background text-sm"
        />
      </div>

      {/* Categories */}
      <Show when={registry()?.categories}>
        <div class="flex flex-wrap gap-1.5">
          <button
            onClick={() => setSelectedCategory(null)}
            class={`text-xs px-2 py-0.5 rounded-full border ${
              !selectedCategory() ? 'bg-primary text-primary-foreground border-primary' : 'border-input hover:bg-muted'
            }`}
          >
            All
          </button>
          <For each={registry()!.categories}>
            {(cat) => (
              <button
                onClick={() => setSelectedCategory(selectedCategory() === cat.id ? null : cat.id)}
                class={`text-xs px-2 py-0.5 rounded-full border ${
                  selectedCategory() === cat.id
                    ? 'bg-primary text-primary-foreground border-primary'
                    : 'border-input hover:bg-muted'
                }`}
              >
                {cat.label}
              </button>
            )}
          </For>
        </div>
      </Show>

      {/* Plugin grid */}
      <Show when={registry()} fallback={
        <div class="text-sm text-muted-foreground text-center py-8">
          Loading plugin registry...
        </div>
      }>
        <div class="grid gap-3" style="grid-template-columns: repeat(auto-fill, minmax(280px, 1fr))">
          <For each={filteredPlugins()} fallback={
            <div class="text-sm text-muted-foreground text-center py-4 col-span-full">
              No plugins found
            </div>
          }>
            {(plugin) => (
              <div class="border border-border rounded-lg p-4 hover:border-primary/50 transition-colors">
                <div class="flex items-start gap-3">
                  <Show when={plugin.icon} fallback={
                    <div class="w-10 h-10 rounded-md bg-muted flex items-center justify-center text-muted-foreground text-lg font-bold flex-shrink-0">
                      {plugin.displayName[0]}
                    </div>
                  }>
                    <img src={plugin.icon} alt="" class="w-10 h-10 rounded-md flex-shrink-0" />
                  </Show>
                  <div class="min-w-0 flex-1">
                    <div class="flex items-center gap-1.5">
                      <h5 class="text-sm font-semibold truncate">{plugin.displayName}</h5>
                      <Show when={plugin.verified}>
                        <CheckCircle size={12} class="text-green-400 flex-shrink-0" />
                      </Show>
                      <Show when={plugin.featured}>
                        <Star size={12} class="text-yellow-400 flex-shrink-0" />
                      </Show>
                    </div>
                    <p class="text-xs text-muted-foreground mt-0.5 line-clamp-2">{plugin.description}</p>
                  </div>
                </div>

                <div class="flex items-center justify-between mt-3">
                  <div class="flex gap-1.5 text-xs text-muted-foreground">
                    <span>{plugin.author}</span>
                    <span>·</span>
                    <span class="capitalize">{plugin.pluginType}</span>
                    <Show when={plugin.license}>
                      <span>·</span>
                      <span>{plugin.license}</span>
                    </Show>
                  </div>
                  <div class="flex gap-1.5">
                    <Show when={plugin.repository}>
                      <a
                        href={plugin.repository}
                        target="_blank"
                        rel="noopener"
                        class="inline-flex items-center p-1 rounded text-muted-foreground hover:text-foreground"
                        title="Repository"
                      >
                        <ExternalLink size={12} />
                      </a>
                    </Show>
                    <button
                      onClick={() => installPlugin(plugin.npmPackage)}
                      disabled={installing() === plugin.npmPackage}
                      class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs bg-primary text-primary-foreground hover:bg-primary/90 disabled:opacity-50"
                    >
                      <Download size={12} />
                      {installing() === plugin.npmPackage ? 'Installing...' : 'Install'}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
};

export default PluginBrowser;
