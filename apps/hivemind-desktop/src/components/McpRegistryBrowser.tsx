import { For, Show, createSignal, createMemo, onCleanup, onMount } from 'solid-js';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { LoaderCircle, ExternalLink, ArrowLeft } from 'lucide-solid';
import type { McpServerConfig } from '../types';
import { openExternal } from '../utils';
import {
  searchRegistryServers,
  getVariants,
  collectRequiredInputs,
  collectRequiredInputsForRemote,
  mapRegistryPackageToConfig,
  mapRegistryRemoteToConfig,
  generateServerId,
  type RegistryServerResponse,
  type RegistryVariant,
  type UserPromptInput,
} from '../lib/mcpRegistry';

export interface McpRegistryBrowserProps {
  existingIds: string[];
  onSelect: (config: Partial<McpServerConfig>, serverInfo: { name: string; description: string }) => void;
  onCancel: () => void;
}

const BADGE_STYLES: Record<string, string> = {
  npm: 'background:#cb3837;color:#fff;',
  pypi: 'background:#3775a9;color:#fff;',
  oci: 'background:#0db7ed;color:#fff;',
  nuget: 'background:#004880;color:#fff;',
  sse: 'background:#22c55e;color:#fff;',
  'streamable-http': 'background:#8b5cf6;color:#fff;',
};

const BADGE_BASE = 'display:inline-block;padding:2px 8px;border-radius:9999px;font-size:0.7em;font-weight:500;';

function truncate(text: string, max: number): string {
  return text.length <= max ? text : text.slice(0, max).trimEnd() + '\u2026';
}

function getTransportBadges(server: RegistryServerResponse): { label: string; style: string }[] {
  const badges: { label: string; style: string }[] = [];
  const seen = new Set<string>();

  for (const pkg of server.server.packages ?? []) {
    if (!seen.has(pkg.registryType)) {
      seen.add(pkg.registryType);
      badges.push({
        label: pkg.registryType,
        style:
          BADGE_BASE +
          (BADGE_STYLES[pkg.registryType] ?? 'background:hsl(var(--secondary));color:hsl(var(--foreground));'),
      });
    }
  }

  for (const remote of server.server.remotes ?? []) {
    if (!seen.has(remote.type)) {
      seen.add(remote.type);
      badges.push({
        label: remote.type,
        style:
          BADGE_BASE + (BADGE_STYLES[remote.type] ?? 'background:hsl(var(--secondary));color:hsl(var(--foreground));'),
      });
    }
  }

  return badges;
}

export function McpRegistryBrowser(props: McpRegistryBrowserProps) {
  const [query, setQuery] = createSignal('');
  const [results, setResults] = createSignal<RegistryServerResponse[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [loadingMore, setLoadingMore] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [nextCursor, setNextCursor] = createSignal<string | undefined>(undefined);
  const [selectedServer, setSelectedServer] = createSignal<RegistryServerResponse | null>(null);
  const [selectedVariant, setSelectedVariant] = createSignal<RegistryVariant | null>(null);
  const [userInputs, setUserInputs] = createSignal<Record<string, string>>({});

  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  let abortController: AbortController | undefined;

  onCleanup(() => {
    if (debounceTimer !== undefined) clearTimeout(debounceTimer);
    abortController?.abort();
  });

  async function fetchServers(search: string, cursor?: string) {
    abortController?.abort();
    abortController = new AbortController();
    const { signal } = abortController;

    if (cursor) {
      setLoadingMore(true);
    } else {
      setLoading(true);
      setResults([]);
      setNextCursor(undefined);
    }
    setError(null);
    try {
      const resp = await searchRegistryServers({ search: search || undefined, cursor, limit: 20 }, signal);
      if (cursor) {
        setResults((prev) => [...prev, ...resp.servers]);
      } else {
        setResults(resp.servers);
      }
      setNextCursor(resp.metadata.nextCursor);
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') return;
      setError(err instanceof Error ? err.message : 'Failed to fetch servers');
    } finally {
      setLoading(false);
      setLoadingMore(false);
    }
  }

  onMount(() => {
    fetchServers('');
  });

  function handleSearchInput(value: string) {
    setQuery(value);
    if (debounceTimer !== undefined) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      setSelectedServer(null);
      setSelectedVariant(null);
      fetchServers(value);
    }, 300);
  }

  function handleSelectVariant(variant: RegistryVariant) {
    setSelectedVariant(variant);
    const inputs =
      variant.kind === 'package'
        ? collectRequiredInputs(variant.pkg)
        : collectRequiredInputsForRemote(variant.remote);
    const defaults: Record<string, string> = {};
    for (const input of inputs) {
      if (input.defaultValue !== undefined) {
        defaults[input.key] = input.defaultValue;
      }
    }
    setUserInputs(defaults);
  }

  function handleSelectServer(server: RegistryServerResponse) {
    setSelectedServer(server);
    setSelectedVariant(null);
    setUserInputs({});
    const v = getVariants(server.server);
    if (v.length === 0) {
      setSelectedServer(null);
      setError('This server has no available installation methods.');
      return;
    }
    if (v.length === 1) {
      handleSelectVariant(v[0]);
    }
  }

  function handleInputChange(key: string, value: string) {
    setUserInputs((prev) => ({ ...prev, [key]: value }));
  }

  function handleBack() {
    setSelectedServer(null);
    setSelectedVariant(null);
    setUserInputs({});
  }

  function handleAddServer() {
    const server = selectedServer();
    const variant = selectedVariant();
    if (!server || !variant) return;

    const inputs = userInputs();
    const config =
      variant.kind === 'package'
        ? mapRegistryPackageToConfig(server.server, variant.pkg, inputs)
        : mapRegistryRemoteToConfig(server.server, variant.remote, inputs);

    config.id = generateServerId(server.server.name, props.existingIds);

    props.onSelect(config, {
      name: server.server.title || server.server.name.split('/').pop() || server.server.name,
      description: server.server.description,
    });
  }

  const variants = createMemo(() => {
    const server = selectedServer();
    return server ? getVariants(server.server) : [];
  });

  const requiredInputs = createMemo<UserPromptInput[]>(() => {
    const variant = selectedVariant();
    if (!variant) return [];
    return variant.kind === 'package'
      ? collectRequiredInputs(variant.pkg)
      : collectRequiredInputsForRemote(variant.remote);
  });

  const canSubmit = createMemo(() => {
    if (!selectedVariant()) return false;
    const inputs = requiredInputs();
    const values = userInputs();
    return inputs.filter((i) => i.isRequired).every((i) => (values[i.key] ?? '').trim() !== '');
  });

  return (
    <Dialog open={true} onOpenChange={(open) => { if (!open) props.onCancel(); }}>
      <DialogContent
        class="max-w-[800px] w-[90vw] max-h-[80vh] flex flex-col p-0"
        onInteractOutside={(e: Event) => e.preventDefault()}
      >
        <DialogHeader class="px-6 pt-6 pb-2 flex-shrink-0">
          <DialogTitle class="flex items-center gap-2">
            <Show when={selectedServer()}>
              <button
                onClick={handleBack}
                style="background:none;border:none;cursor:pointer;padding:4px;display:flex;align-items:center;color:hsl(var(--foreground));"
              >
                <ArrowLeft size={18} />
              </button>
            </Show>
            Browse MCP Registry
          </DialogTitle>
        </DialogHeader>

        <Show
          when={selectedServer()}
          fallback={
            <>
              {/* Search input */}
              <div style="padding:0 24px;flex-shrink:0;">
                <input
                  type="text"
                  placeholder="Search MCP servers..."
                  value={query()}
                  onInput={(e) => handleSearchInput(e.currentTarget.value)}
                  style="width:100%;padding:8px 12px;border-radius:6px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.9em;outline:none;box-sizing:border-box;"
                />
              </div>

              {/* Scrollable results area */}
              <div style="flex:1;overflow-y:auto;padding:12px 24px 16px;">
                <Show when={loading()}>
                  <div style="display:flex;justify-content:center;padding:32px;">
                    <LoaderCircle size={24} class="animate-spin" style="color:hsl(var(--muted-foreground));" />
                  </div>
                </Show>

                <Show when={error()}>
                  <div style="text-align:center;padding:24px;">
                    <p style="color:hsl(var(--destructive));margin:0 0 12px;font-size:0.9em;">{error()}</p>
                    <Button variant="outline" size="sm" onClick={() => fetchServers(query())}>
                      Retry
                    </Button>
                  </div>
                </Show>

                <Show when={!loading() && !error() && results().length === 0}>
                  <div style="text-align:center;padding:32px;color:hsl(var(--muted-foreground));font-size:0.9em;">
                    No servers found
                  </div>
                </Show>

                <Show when={!loading() && !error()}>
                  <div style="display:flex;flex-direction:column;gap:6px;">
                    <For each={results()}>
                      {(server) => {
                        const badges = getTransportBadges(server);
                        const displayName =
                          server.server.title || server.server.name.split('/').pop() || server.server.name;
                        return (
                          <button
                            onClick={() => handleSelectServer(server)}
                            style="display:block;width:100%;text-align:left;padding:12px;border-radius:8px;cursor:pointer;background:hsl(var(--secondary));border:1px solid hsl(var(--border));color:hsl(var(--foreground));"
                          >
                            <div style="display:flex;justify-content:space-between;align-items:flex-start;gap:8px;">
                              <div style="flex:1;min-width:0;">
                                <div style="font-weight:500;font-size:0.9em;margin-bottom:2px;">{displayName}</div>
                                <div style="font-size:0.8em;color:hsl(var(--muted-foreground));overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                                  {truncate(server.server.description, 100)}
                                </div>
                              </div>
                              <div style="display:flex;gap:4px;flex-shrink:0;align-items:center;">
                                <For each={badges}>
                                  {(badge) => <span style={badge.style}>{badge.label}</span>}
                                </For>
                                <Show when={server.server.repository?.url}>
                                  <span
                                    role="link"
                                    onClick={(e) => { e.stopPropagation(); openExternal(server.server.repository!.url); }}
                                    style="display:flex;align-items:center;color:hsl(var(--muted-foreground));margin-left:4px;cursor:pointer;"
                                  >
                                    <ExternalLink size={14} />
                                  </span>
                                </Show>
                              </div>
                            </div>
                          </button>
                        );
                      }}
                    </For>
                  </div>

                  <Show when={nextCursor()}>
                    <div style="display:flex;justify-content:center;padding:16px 0 4px;">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={loadingMore()}
                        onClick={() => fetchServers(query(), nextCursor())}
                      >
                        <Show when={loadingMore()} fallback={<>Load more</>}>
                          <LoaderCircle size={14} class="animate-spin" style="margin-right:6px;" />
                          Loading&hellip;
                        </Show>
                      </Button>
                    </div>
                  </Show>
                </Show>
              </div>

              {/* Footer */}
              <div style="padding:0 24px 16px;display:flex;justify-content:flex-end;flex-shrink:0;">
                <Button variant="outline" size="sm" onClick={props.onCancel}>
                  Cancel
                </Button>
              </div>
            </>
          }
        >
          {/* Detail view */}
          <div style="flex:1;overflow-y:auto;padding:0 24px 24px;">
            <div style="margin-bottom:16px;">
              <h3 style="margin:0 0 4px;font-size:1.1em;font-weight:600;">
                {selectedServer()!.server.title || selectedServer()!.server.name.split('/').pop()}
              </h3>
              <p style="margin:0 0 8px;color:hsl(var(--muted-foreground));font-size:0.9em;">
                {selectedServer()!.server.description}
              </p>
              <div style="display:flex;gap:12px;align-items:center;font-size:0.85em;color:hsl(var(--muted-foreground));">
                <span>v{selectedServer()!.server.version}</span>
                <Show when={selectedServer()!.server.repository?.url}>
                  <span
                    role="link"
                    onClick={() => openExternal(selectedServer()!.server.repository!.url)}
                    style="display:inline-flex;align-items:center;gap:4px;color:hsl(var(--primary));text-decoration:none;cursor:pointer;"
                  >
                    <ExternalLink size={14} />
                    Repository
                  </span>
                </Show>
              </div>
            </div>

            {/* Variant picker */}
            <Show when={variants().length > 1}>
              <div style="margin-bottom:16px;">
                <label style="display:block;font-size:0.85em;font-weight:500;margin-bottom:8px;color:hsl(var(--foreground));">
                  Select a variant
                </label>
                <div style="display:flex;flex-direction:column;gap:6px;">
                  <For each={variants()}>
                    {(variant) => (
                      <button
                        onClick={() => handleSelectVariant(variant)}
                        style={`display:block;width:100%;text-align:left;padding:10px 12px;border-radius:6px;cursor:pointer;font-size:0.85em;background:${selectedVariant() === variant ? 'hsl(var(--primary) / 0.1)' : 'hsl(var(--secondary))'};border:1px solid ${selectedVariant() === variant ? 'hsl(var(--primary))' : 'hsl(var(--border))'};color:hsl(var(--foreground));`}
                      >
                        {variant.label}
                      </button>
                    )}
                  </For>
                </div>
              </div>
            </Show>

            {/* Required inputs form */}
            <Show when={selectedVariant() && requiredInputs().length > 0}>
              <div style="margin-bottom:16px;">
                <label style="display:block;font-size:0.85em;font-weight:500;margin-bottom:8px;color:hsl(var(--foreground));">
                  Configuration
                </label>
                <div style="display:flex;flex-direction:column;gap:12px;">
                  <For each={requiredInputs()}>
                    {(input) => (
                      <div>
                        <label style="display:block;font-size:0.8em;font-weight:500;margin-bottom:4px;color:hsl(var(--foreground));">
                          {input.label}
                          <Show when={input.isRequired}>
                            <span style="color:hsl(var(--destructive));margin-left:2px;">*</span>
                          </Show>
                        </label>
                        <Show when={input.description}>
                          <p style="margin:0 0 4px;font-size:0.75em;color:hsl(var(--muted-foreground));">
                            {input.description}
                          </p>
                        </Show>
                        <Show
                          when={input.choices && input.choices.length > 0}
                          fallback={
                            <input
                              type={input.isSecret ? 'password' : 'text'}
                              value={userInputs()[input.key] ?? ''}
                              placeholder={input.placeholder ?? ''}
                              onInput={(e) => handleInputChange(input.key, e.currentTarget.value)}
                              style="width:100%;padding:6px 10px;border-radius:6px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;outline:none;box-sizing:border-box;"
                            />
                          }
                        >
                          <select
                            value={userInputs()[input.key] ?? ''}
                            onChange={(e) => handleInputChange(input.key, e.currentTarget.value)}
                            style="width:100%;padding:6px 10px;border-radius:6px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;outline:none;box-sizing:border-box;"
                          >
                            <option value="">Select&hellip;</option>
                            <For each={input.choices!}>
                              {(choice) => <option value={choice}>{choice}</option>}
                            </For>
                          </select>
                        </Show>
                      </div>
                    )}
                  </For>
                </div>
              </div>
            </Show>

            {/* Add server button */}
            <Show when={selectedVariant()}>
              <div style="display:flex;justify-content:flex-end;gap:8px;padding-top:12px;border-top:1px solid hsl(var(--border));">
                <Button variant="outline" size="sm" onClick={props.onCancel}>
                  Cancel
                </Button>
                <Button size="sm" disabled={!canSubmit()} onClick={handleAddServer}>
                  Add Server
                </Button>
              </div>
            </Show>
          </div>
        </Show>
      </DialogContent>
    </Dialog>
  );
}
