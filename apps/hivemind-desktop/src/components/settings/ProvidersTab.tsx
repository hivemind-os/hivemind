import { Show, For, createSignal, createEffect, createMemo, on, batch, ErrorBoundary } from 'solid-js';
import type { Accessor, Setter, JSX } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Hourglass, RefreshCw, X, Search, ChevronUp, ChevronDown } from 'lucide-solid';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Button } from '~/ui/button';
import CopilotAuthWizard from '../CopilotAuthWizard';
import type { AppContext, CapabilityOption, HiveMindConfigData, InstalledModel } from '../../types';
import { displayModelName, modelCapsToProviderCaps } from '../../types';

// Priority patterns for model sorting — popular models appear first
const MODEL_PRIORITY: [RegExp, number][] = [
  [/^gpt-5\.\d+(?!.*(?:mini|codex))/i, 100],
  [/^claude[_-]?opus/i, 100],
  [/^claude[_-]?sonnet/i, 95],
  [/^gpt-5[\d.]*-codex/i, 90],
  [/^gpt-5[\d.]*-mini/i, 85],
  [/^claude[_-]?haiku/i, 85],
  [/^gpt-4/i, 70],
  [/^o\d/i, 65],
];

function getModelPriority(id: string): number {
  for (const [re, p] of MODEL_PRIORITY) if (re.test(id)) return p;
  return 0;
}

function extractVersion(id: string): number {
  const m = id.match(/(\d+)\.(\d+)/);
  if (m) return parseFloat(`${m[1]}.${m[2]}`);
  const s = id.match(/(\d+)/);
  return s ? parseInt(s[1]) : 0;
}

function sortModelIds(models: string[]): string[] {
  return [...models].sort((a, b) => {
    const dp = getModelPriority(b) - getModelPriority(a);
    if (dp !== 0) return dp;
    const dv = extractVersion(b) - extractVersion(a);
    if (dv !== 0) return dv;
    return a.localeCompare(b);
  });
}

type CopilotModel = { id: string; name?: string; version?: string };

function sortCopilotModels(models: CopilotModel[]): CopilotModel[] {
  return [...models].sort((a, b) => {
    const dp = getModelPriority(b.id) - getModelPriority(a.id);
    if (dp !== 0) return dp;
    const dv = extractVersion(b.id) - extractVersion(a.id);
    if (dv !== 0) return dv;
    return a.id.localeCompare(b.id);
  });
}

const ALL_CAPABILITIES: CapabilityOption[] = ['chat', 'code', 'vision', 'embedding', 'tool-use'];

/** A single model row: enable/disable switch + per-model capability toggles. */
function ModelToggleItem(props: {
  modelId: string;
  label?: JSX.Element;
  enabled: boolean;
  caps: CapabilityOption[];
  onToggle: (checked: boolean) => void;
  onToggleCap: (cap: CapabilityOption) => void;
}) {
  return (
    <div style={{
      padding: '8px 10px',
      'border-radius': '6px',
      border: '1px solid hsl(var(--border))',
      background: props.enabled ? 'hsl(var(--card))' : 'transparent',
      opacity: props.enabled ? '1' : '0.6',
    }}>
      <Switch checked={props.enabled} onChange={props.onToggle} class="flex items-center gap-2">
        <SwitchControl><SwitchThumb /></SwitchControl>
        <SwitchLabel>{props.label ?? <span style="font-weight: 600;">{props.modelId}</span>}</SwitchLabel>
      </Switch>
      <Show when={props.enabled}>
        <div style="margin-top: 6px; margin-left: 24px; display: flex; flex-wrap: wrap; gap: 6px;">
          {ALL_CAPABILITIES.map((cap) => (
            <Switch checked={props.caps.includes(cap)} onChange={() => props.onToggleCap(cap)} class="flex items-center gap-2">
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>{cap}</SwitchLabel>
            </Switch>
          ))}
        </div>
      </Show>
    </div>
  );
}

/** Fetched model list with search, toggles, count, manual-add, and fallback tags. */
function FetchedModelList(props: {
  allModels: string[];
  enabledModels: string[];
  modelCaps: Record<string, CapabilityOption[]>;
  onToggleModel: (modelId: string, checked: boolean) => void;
  onToggleCap: (modelId: string, cap: CapabilityOption) => void;
  onAddManual: (modelId: string) => void;
  onRemoveManual: (modelIdx: number) => void;
  renderLabel?: (modelId: string) => JSX.Element;
  showFallbackTags?: boolean;
}) {
  const [search, setSearch] = createSignal('');
  const [manualInput, setManualInput] = createSignal('');

  const filtered = createMemo(() => {
    const q = search().toLowerCase();
    if (!q) return props.allModels;
    return props.allModels.filter(id => id.toLowerCase().includes(q));
  });

  const handleManualAdd = () => {
    const m = manualInput().trim();
    if (m) { props.onAddManual(m); setManualInput(''); }
  };

  return (
    <>
      <Show when={props.allModels.length > 0}>
        <p class="muted" style="font-size: 0.8em; margin: 0.25rem 0 0.5rem;">
          Select which models to enable and set capabilities for each.
        </p>
        <Show when={props.allModels.length > 8}>
          <div class="flex items-center gap-2 mb-2" style="position: relative;">
            <Search size={14} style="position: absolute; left: 8px; top: 50%; transform: translateY(-50%); color: hsl(var(--muted-foreground));" />
            <input type="text" placeholder="Filter models…" value={search()} onInput={(e) => setSearch(e.currentTarget.value)}
              style="width: 100%; padding-left: 28px; font-size: 0.85em;" />
          </div>
        </Show>
        <div style="display: flex; flex-direction: column; gap: 8px; max-height: 360px; overflow-y: auto; padding-right: 4px;">
          <For each={filtered()}>
            {(modelId) => (
              <ModelToggleItem
                modelId={modelId}
                label={props.renderLabel?.(modelId)}
                enabled={props.enabledModels.includes(modelId)}
                caps={(props.modelCaps[modelId] ?? []) as CapabilityOption[]}
                onToggle={(checked) => props.onToggleModel(modelId, checked)}
                onToggleCap={(cap) => props.onToggleCap(modelId, cap)}
              />
            )}
          </For>
        </div>
        <p class="muted" style="font-size: 0.8em; margin-top: 0.5rem;">
          {props.enabledModels.length} of {props.allModels.length} model(s) enabled
          <Show when={search()}> · {filtered().length} shown</Show>
        </p>
      </Show>
      <Show when={props.showFallbackTags && props.allModels.length === 0 && props.enabledModels.length > 0}>
        <p class="muted" style="font-size: 0.8em; margin: 0.25rem 0 0.5rem;">
          Use "Fetch Models from API" above to see all available models, or manage manually below.
        </p>
        <div class="settings-tag-list">
          <For each={props.enabledModels}>
            {(_model, mIdx) => (
              <span class="settings-tag">
                {props.enabledModels[mIdx()]}
                <button class="settings-tag-remove" onClick={() => props.onRemoveManual(mIdx())}>×</button>
              </span>
            )}
          </For>
        </div>
      </Show>
      <div class="settings-inline-add" style="margin-top: 8px;">
        <input type="text" placeholder="Add model manually…" value={manualInput()} onInput={(e) => setManualInput(e.currentTarget.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') handleManualAdd(); }} />
        <button onClick={handleManualAdd}>+</button>
      </div>
    </>
  );
}

export interface ProvidersTabProps {
  editConfig: Accessor<HiveMindConfigData | null>;
  editingProviderIdx: Accessor<number | null>;
  setEditingProviderIdx: Setter<number | null>;
  addProvider: () => void;
  removeProvider: (idx: number) => void;
  moveProvider: (fromIdx: number, toIdx: number) => void;
  updateProvider: (idx: number, field: string, value: unknown) => void;
  addModelToProvider: (idx: number, model: string) => void;
  removeModelFromProvider: (pIdx: number, mIdx: number) => void;
  saveConfig: () => Promise<void>;
  localModels: Accessor<InstalledModel[]>;
  context: Accessor<AppContext | null>;
  openExternal: (url: string) => Promise<void>;
  configLoadingFallback: (label: string) => JSX.Element;
  hideDelete?: boolean;
  onSave?: () => void;
}

export default function ProvidersTab(props: ProvidersTabProps) {
  const {
    editConfig,
    editingProviderIdx,
    setEditingProviderIdx,
    addProvider,
    removeProvider,
    moveProvider,
    updateProvider,
    addModelToProvider,
    removeModelFromProvider,
    saveConfig,
    localModels,
    context,
    openExternal,
  } = props;

  const [newModelInput, setNewModelInput] = createSignal('');

  const autoPopulateModelCaps = async (providerIdx: number, modelId: string) => {
    try {
      const result = await invoke('lookup_model_metadata', { model_names: [modelId] }) as Record<string, { context_window: number; max_output_tokens: number; capabilities: string[] }>;
      const meta = result[modelId];
      if (meta && meta.capabilities.length > 0) {
        // Wrap in batch() since this runs from an async callback outside
        // SolidJS's reactive scope — without it, signal updates trigger
        // immediate uncontrolled re-renders that can corrupt dialog state.
        batch(() => {
          const currentProvider = editConfig()?.models?.providers?.[providerIdx];
          if (currentProvider && !(currentProvider.model_capabilities ?? {})[modelId]) {
            const mc = { ...(currentProvider.model_capabilities ?? {}), [modelId]: meta.capabilities as CapabilityOption[] };
            updateProvider(providerIdx, 'model_capabilities', mc);
          }
        });
      }
    } catch (e) {
      console.warn('Failed to lookup model metadata:', e);
    }
  };

  return (
    <div class="settings-section">
      <Show when={editConfig()} fallback={props.configLoadingFallback('provider settings')}>
          <>
            <div class="settings-section-header">
              <h3>Model Providers</h3>
              <button data-testid="settings-add-provider" aria-label="Add provider" onClick={addProvider}>+ Add Provider</button>
            </div>

            {/* Provider list table */}
            <div class="provider-list">
              <div class="provider-list-header">
                <span></span>
                <span>Name</span>
                <span>Kind</span>
                <span>Channel</span>
                <span>Status</span>
                <span></span>
              </div>
              <For each={editConfig()!.models.providers}>
                {(provider, idx) => (
                  <div class="provider-list-row">
                    <span class="provider-list-order">
                      <button
                        class="provider-order-btn"
                        disabled={idx() === 0}
                        onClick={() => moveProvider(idx(), idx() - 1)}
                        aria-label="Move up"
                      ><ChevronUp size={14} /></button>
                      <button
                        class="provider-order-btn"
                        disabled={idx() === editConfig()!.models.providers.length - 1}
                        onClick={() => moveProvider(idx(), idx() + 1)}
                        aria-label="Move down"
                      ><ChevronDown size={14} /></button>
                    </span>
                    <span class="provider-list-id">{provider.name || provider.id}</span>
                    <span><span class="badge">{provider.kind}</span></span>
                    <span><span class="badge">{provider.channel_class ?? '—'}</span></span>
                    <span><span class={provider.enabled ? 'pill pill-green' : 'pill pill-muted'}>{provider.enabled ? 'Enabled' : 'Disabled'}</span></span>
                    <span class="provider-list-actions">
                      <button onClick={() => setEditingProviderIdx(idx())}>Edit</button>
                      <button class="settings-remove-btn" onClick={() => removeProvider(idx())}>✕</button>
                    </span>
                  </div>
                )}
              </For>
            </div>

            {/* Provider edit dialog */}
            <Dialog
              open={editingProviderIdx() !== null && !!editConfig()?.models?.providers?.[editingProviderIdx()!]}
              onOpenChange={(open) => { if (!open) setEditingProviderIdx(null); }}
            >
            <DialogContent class="max-w-[640px] max-h-[85vh] flex flex-col overflow-clip">
            <ErrorBoundary fallback={(err, reset) => (
              <div class="p-6 text-center">
                <p class="text-destructive text-sm mb-4">Something went wrong rendering this dialog.</p>
                <pre class="text-xs text-muted-foreground mb-4 max-w-full overflow-auto">{String(err)}</pre>
                <Button variant="outline" onClick={reset}>Retry</Button>
              </div>
            )}>
            <Show when={editingProviderIdx() !== null && editConfig()?.models?.providers?.[editingProviderIdx()!]}>
              {(() => {
                const idx = () => editingProviderIdx()!;
                const provider = () => {
                  const cfg = editConfig();
                  if (!cfg) throw new Error('config not loaded');
                  const p = cfg.models.providers[idx()];
                  if (!p) throw new Error('provider not found');
                  return p;
                };

                // API key management
                const [apiKey, setApiKey] = createSignal('');
                const [showApiKey, setShowApiKey] = createSignal(false);
                const [apiKeyStatus, setApiKeyStatus] = createSignal<'loading' | 'loaded' | 'empty' | 'error'>('loading');

                // Available models fetched from provider APIs
                const [availableCopilotModels, setAvailableCopilotModels] = createSignal<CopilotModel[]>([]);
                const [availableAnthropicModels, setAvailableAnthropicModels] = createSignal<string[]>([]);
                const [availableOpenAiModels, setAvailableOpenAiModels] = createSignal<string[]>([]);
                const [availableFoundryModels, setAvailableFoundryModels] = createSignal<string[]>([]);

                // Sorted model lists (filtering is handled inside FetchedModelList)
                const sortedCopilotModelIds = createMemo(() =>
                  sortCopilotModels(availableCopilotModels()).map(m => m.id));
                const copilotModelNames = createMemo(() => {
                  const map: Record<string, string | undefined> = {};
                  for (const m of availableCopilotModels()) {
                    if (m.name && m.name !== m.id) map[m.id] = m.name;
                  }
                  return map;
                });
                const sortedAnthropicModels = createMemo(() =>
                  sortModelIds(availableAnthropicModels()));
                const sortedOpenAiModels = createMemo(() =>
                  sortModelIds(availableOpenAiModels()));
                const sortedFoundryModels = createMemo(() =>
                  sortModelIds(availableFoundryModels()));

                // Shared handlers for FetchedModelList callbacks
                const handleToggleModel = (modelId: string, checked: boolean) => {
                  if (checked) {
                    addModelToProvider(idx(), modelId);
                    if (!(provider().model_capabilities ?? {})[modelId]) {
                      autoPopulateModelCaps(idx(), modelId);
                    }
                  } else {
                    const mIdx = provider().models.indexOf(modelId);
                    if (mIdx >= 0) removeModelFromProvider(idx(), mIdx);
                  }
                };
                const handleToggleCap = (modelId: string, cap: CapabilityOption) => {
                  const allCaps = provider().model_capabilities ?? {};
                  const current = (allCaps[modelId] ?? []) as CapabilityOption[];
                  const updated = current.includes(cap) ? current.filter(c => c !== cap) : [...current, cap];
                  updateProvider(idx(), 'model_capabilities', { ...allCaps, [modelId]: updated });
                };
                const handleAddManual = (modelId: string) => {
                  addModelToProvider(idx(), modelId);
                  autoPopulateModelCaps(idx(), modelId);
                };
                const handleRemoveManual = (mIdx: number) => removeModelFromProvider(idx(), mIdx);

                const apiKeyKinds = ['anthropic', 'open-ai-compatible', 'microsoft-foundry'];
                const needsApiKey = () => apiKeyKinds.includes(provider().kind);

                // Stable memos for the fields the API-key effect actually cares about.
                // Without these, the effect would re-run on every editConfig() change
                // (e.g. toggling a model capability) because provider() dereferences the
                // full config signal.  The spurious re-runs fire invoke('load_secret')
                // in a loop, which races with in-flight responses and can leave the
                // dialog in a broken state.
                const providerKind = createMemo(() => provider().kind);
                const providerId = createMemo(() => provider().id);

                // Load API key from OS keyring when dialog opens or kind/id changes
                createEffect(on([providerKind, providerId], ([kind, id]) => {
                  if (apiKeyKinds.includes(kind) && id) {
                    setApiKeyStatus('loading');
                    invoke('load_secret', { key: `provider:${id}:api-key` })
                      .then((key) => {
                        setApiKey((key as string) || '');
                        setApiKeyStatus(key ? 'loaded' : 'empty');
                      })
                      .catch(() => { setApiKey(''); setApiKeyStatus('empty'); });
                  }
                }));

                const saveApiKey = async () => {
                  if (!needsApiKey() || !provider().id) return;
                  const key = apiKey();
                  try {
                    if (key) {
                      await invoke('save_secret', { key: `provider:${provider().id}:api-key`, value: key });
                    }
                  } catch (e) { console.error('Failed to save API key:', e); }
                };

                const handleSave = async () => {
                  if (needsApiKey()) {
                    await saveApiKey();
                    updateProvider(idx(), 'auth', 'api-key');
                  }
                  // Auto-save config so provider ID (UUID) is persisted alongside the keyring secret
                  await saveConfig();
                  props.onSave?.();
                  setEditingProviderIdx(null);
                };

                const handleCancel = () => {
                  setEditingProviderIdx(null);
                };

                // Auto-populate defaults when provider kind changes
                const handleKindChange = (newKind: string) => {
                  updateProvider(idx(), 'kind', newKind);
                  switch (newKind) {
                    case 'anthropic':
                      updateProvider(idx(), 'name', 'Anthropic');
                      updateProvider(idx(), 'auth', 'api-key');
                      updateProvider(idx(), 'base_url', 'https://api.anthropic.com');
                      updateProvider(idx(), 'models', []);
                      updateProvider(idx(), 'model_capabilities', {});
                      break;
                    case 'open-ai-compatible':
                      updateProvider(idx(), 'name', 'OpenAI Compatible');
                      updateProvider(idx(), 'auth', 'api-key');
                      updateProvider(idx(), 'base_url', 'https://api.openai.com/v1');
                      updateProvider(idx(), 'models', []);
                      updateProvider(idx(), 'model_capabilities', {});
                      break;
                    case 'microsoft-foundry':
                      updateProvider(idx(), 'name', 'Microsoft Foundry');
                      updateProvider(idx(), 'auth', 'api-key');
                      updateProvider(idx(), 'base_url', '');
                      updateProvider(idx(), 'models', []);
                      updateProvider(idx(), 'model_capabilities', {});
                      break;
                    case 'github-copilot':
                      updateProvider(idx(), 'name', 'GitHub Copilot');
                      updateProvider(idx(), 'auth', 'github-oauth');
                      updateProvider(idx(), 'base_url', 'https://api.githubcopilot.com');
                      updateProvider(idx(), 'models', []);
                      updateProvider(idx(), 'model_capabilities', {});
                      break;
                    case 'ollama-local':
                      updateProvider(idx(), 'name', 'Ollama (Local)');
                      updateProvider(idx(), 'auth', 'none');
                      updateProvider(idx(), 'base_url', 'http://localhost:11434');
                      updateProvider(idx(), 'models', []);
                      updateProvider(idx(), 'model_capabilities', {});
                      break;
                  }
                };

                // Reusable API key field
                const ApiKeyField = () => (
                  <label>
                    <span>API Key</span>
                    <div style="display: flex; gap: 0.5rem; align-items: center;">
                      <input
                        type={showApiKey() ? 'text' : 'password'}
                        value={apiKey()}
                        onInput={(e) => setApiKey(e.currentTarget.value)}
                        onBlur={saveApiKey}
                        placeholder={apiKeyStatus() === 'loading' ? 'Loading…' : 'Enter API key'}
                        style="flex: 1;"
                      />
                      <button
                        onClick={() => setShowApiKey(!showApiKey())}
                        style="min-width: 50px; padding: 0.35rem 0.5rem; font-size: 0.85em;"
                      >
                        {showApiKey() ? 'Hide' : 'Show'}
                      </button>
                    </div>
                  </label>
                );

                return (
                  <>
                      <header class="flex items-center justify-between mb-4 shrink-0">
                        <h3 class="m-0 text-lg font-semibold">Edit Provider</h3>
                        <Button variant="ghost" size="icon" onClick={handleCancel} aria-label="Close">
                          <X size={16} />
                        </Button>
                      </header>

                      <div class="flex-1 overflow-y-auto min-h-0 -mx-1 px-1">
                      <div class="settings-form">
                        {/* Common fields for all provider types */}
                        <label>
                          <span>Provider Name</span>
                          <input type="text" value={provider().name || ''} onChange={(e) => updateProvider(idx(), 'name', e.currentTarget.value)} />
                        </label>
                        <Switch checked={provider().enabled} onChange={(checked) => updateProvider(idx(), 'enabled', checked)} class="flex items-center gap-2">
                          <SwitchControl><SwitchThumb /></SwitchControl>
                          <SwitchLabel>{provider().enabled ? 'Enabled' : 'Disabled'}</SwitchLabel>
                        </Switch>
                        <label>
                          <span>Kind</span>
                          <select value={provider().kind} onChange={(e) => handleKindChange(e.currentTarget.value)}>
                            <option value="open-ai-compatible">OpenAI Compatible</option>
                            <option value="anthropic">Anthropic</option>
                            <option value="microsoft-foundry">Microsoft Foundry</option>
                            <option value="github-copilot">GitHub Copilot</option>
                            <option value="ollama-local">Ollama (Local)</option>
                            <option value="local-models">Local Models</option>
                            <option value="mock">Mock</option>
                          </select>
                        </label>

                        {/* === Anthropic === */}
                        <Show when={provider().kind === 'anthropic'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            Anthropic uses API keys for authentication. Get yours at{' '}
                            <a href="#" onClick={(e) => { e.preventDefault(); openExternal('https://console.anthropic.com'); }} style="color: hsl(var(--primary));">console.anthropic.com</a>
                          </p>
                          <ApiKeyField />
                          <label>
                            <span>Base URL</span>
                            <input type="text" value={provider().base_url ?? 'https://api.anthropic.com'} onChange={(e) => updateProvider(idx(), 'base_url', e.currentTarget.value || null)} />
                          </label>
                          {/* Fetch models from Anthropic API */}
                          {(() => {
                            const [fetchingModels, setFetchingModels] = createSignal(false);
                            const [fetchError, setFetchError] = createSignal('');
                            const fetchAnthropicModels = async () => {
                              setFetchingModels(true);
                              setFetchError('');
                              try {
                                const key = apiKey();
                                if (!key) { setFetchError('Enter an API key first'); setFetchingModels(false); return; }
                                const baseUrl = (provider().base_url ?? 'https://api.anthropic.com').replace(/\/+$/, '');
                                const models = await invoke('fetch_provider_models', {
                                  base_url: baseUrl,
                                  api_key: key,
                                  provider_kind: 'anthropic',
                                }) as string[];
                                if (models.length > 0) {
                                  setAvailableAnthropicModels(models);
                                } else {
                                  setFetchError('No models returned');
                                }
                              } catch (e: any) {
                                setFetchError(typeof e === 'string' ? e : (e.message ?? 'Failed to fetch models'));
                              } finally {
                                setFetchingModels(false);
                              }
                            };
                            return (
                              <div style="margin-top: 0.5rem;">
                                <button onClick={fetchAnthropicModels} disabled={fetchingModels()} style="font-size: 0.85em;">
                                  {fetchingModels() ? <><Hourglass size={14} /> Fetching…</> : <><RefreshCw size={14} /> Fetch Models from API</>}
                                </button>
                                <Show when={fetchError()}>
                                  <p class="text-destructive" style="font-size: 0.8em; margin: 0.25rem 0;">{fetchError()}</p>
                                </Show>
                              </div>
                            );
                          })()}
                        </Show>

                        {/* === OpenAI Compatible === */}
                        <Show when={provider().kind === 'open-ai-compatible'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            OpenAI uses API keys for authentication. Get yours at{' '}
                            <a href="#" onClick={(e) => { e.preventDefault(); openExternal('https://platform.openai.com'); }} style="color: hsl(var(--primary));">platform.openai.com</a>
                          </p>
                          <ApiKeyField />
                          <label>
                            <span>Base URL</span>
                            <input type="text" value={provider().base_url ?? 'https://api.openai.com/v1'} onChange={(e) => updateProvider(idx(), 'base_url', e.currentTarget.value || null)} />
                          </label>
                          {/* Fetch models from OpenAI API */}
                          {(() => {
                            const [fetchingModels, setFetchingModels] = createSignal(false);
                            const [fetchError, setFetchError] = createSignal('');
                            const fetchOpenAiModels = async () => {
                              setFetchingModels(true);
                              setFetchError('');
                              try {
                                const key = apiKey();
                                if (!key) { setFetchError('Enter an API key first'); setFetchingModels(false); return; }
                                const baseUrl = (provider().base_url ?? 'https://api.openai.com/v1').replace(/\/+$/, '');
                                const models = await invoke('fetch_provider_models', {
                                  base_url: baseUrl,
                                  api_key: key,
                                  provider_kind: 'open-ai-compatible',
                                }) as string[];
                                if (models.length > 0) {
                                  setAvailableOpenAiModels(models);
                                } else {
                                  setFetchError('No models returned');
                                }
                              } catch (e: any) {
                                setFetchError(typeof e === 'string' ? e : (e.message ?? 'Failed to fetch models'));
                              } finally {
                                setFetchingModels(false);
                              }
                            };
                            return (
                              <div style="margin-top: 0.5rem;">
                                <button onClick={fetchOpenAiModels} disabled={fetchingModels()} style="font-size: 0.85em;">
                                  {fetchingModels() ? <><Hourglass size={14} /> Fetching…</> : <><RefreshCw size={14} /> Fetch Models from API</>}
                                </button>
                                <Show when={fetchError()}>
                                  <p class="text-destructive" style="font-size: 0.8em; margin: 0.25rem 0;">{fetchError()}</p>
                                </Show>
                              </div>
                            );
                          })()}
                        </Show>

                        {/* === Microsoft Foundry === */}
                        <Show when={provider().kind === 'microsoft-foundry'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            Enter your Microsoft Foundry endpoint and API key.
                          </p>
                          <label>
                            <span>Endpoint URL</span>
                            <input type="text" placeholder="https://your-resource.openai.azure.com" value={provider().base_url ?? ''} onChange={(e) => updateProvider(idx(), 'base_url', e.currentTarget.value || null)} />
                          </label>
                          <ApiKeyField />
                          <label>
                            <span>API Version</span>
                            <input type="text" value={provider().options?.default_api_version ?? '2024-10-21'} onChange={(e) => updateProvider(idx(), 'options', { ...provider().options, default_api_version: e.currentTarget.value || null })} />
                          </label>
                          {/* Fetch models from Foundry API */}
                          {(() => {
                            const [fetchingModels, setFetchingModels] = createSignal(false);
                            const [fetchError, setFetchError] = createSignal('');
                            const fetchFoundryModels = async () => {
                              setFetchingModels(true);
                              setFetchError('');
                              try {
                                const key = apiKey();
                                if (!key) { setFetchError('Enter an API key first'); setFetchingModels(false); return; }
                                const baseUrl = (provider().base_url ?? '').replace(/\/+$/, '');
                                if (!baseUrl) { setFetchError('Enter an endpoint URL first'); setFetchingModels(false); return; }
                                const models = await invoke('fetch_provider_models', {
                                  base_url: baseUrl,
                                  api_key: key,
                                  provider_kind: 'microsoft-foundry',
                                  api_version: provider().options?.default_api_version || '2024-05-01-preview',
                                }) as string[];
                                if (models.length > 0) {
                                  setAvailableFoundryModels(models);
                                } else {
                                  setFetchError('No models returned');
                                }
                              } catch (e: any) {
                                setFetchError(typeof e === 'string' ? e : (e.message ?? 'Failed to fetch models'));
                              } finally {
                                setFetchingModels(false);
                              }
                            };
                            return (
                              <div style="margin-top: 0.5rem;">
                                <button onClick={fetchFoundryModels} disabled={fetchingModels()} style="font-size: 0.85em;">
                                  {fetchingModels() ? <><Hourglass size={14} /> Fetching…</> : <><RefreshCw size={14} /> Fetch Models from API</>}
                                </button>
                                <Show when={fetchError()}>
                                  <p class="text-destructive" style="font-size: 0.8em; margin: 0.25rem 0;">{fetchError()}</p>
                                </Show>
                              </div>
                            );
                          })()}
                        </Show>

                        {/* === GitHub Copilot === */}
                        <Show when={provider().kind === 'github-copilot'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            Authentication is handled via GitHub OAuth. Make sure you have a GitHub Copilot subscription.
                          </p>
                          <CopilotAuthWizard
                            provider_id={provider().id}
                            onComplete={() => { updateProvider(idx(), 'auth', 'github-oauth'); }}
                            onCancel={() => {}}
                            onModelsLoaded={(models) => setAvailableCopilotModels(models)}
                          />
                          <label>
                            <span>Base URL</span>
                            <input type="text" value={provider().base_url ?? 'https://api.githubcopilot.com'} disabled style="opacity: 0.6;" />
                          </label>
                        </Show>

                        {/* === Ollama Local === */}
                        <Show when={provider().kind === 'ollama-local'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            Connect to a locally running Ollama instance.
                          </p>
                          <label>
                            <span>Base URL</span>
                            <input type="text" value={provider().base_url ?? 'http://localhost:11434'} onChange={(e) => updateProvider(idx(), 'base_url', e.currentTarget.value || null)} />
                          </label>
                        </Show>

                        {/* === Local Models === */}
                        <Show when={provider().kind === 'local-models'}>
                          <p class="muted" style="font-size: 0.85em; margin: 0.5rem 0;">
                            Models are auto-discovered from your installed local models. Leave empty to use all installed models, or select specific ones below.
                          </p>
                        </Show>

                        {/* === Generic fallback (mock, etc.) === */}
                        <Show when={!['anthropic', 'open-ai-compatible', 'microsoft-foundry', 'github-copilot', 'ollama-local', 'local-models'].includes(provider().kind)}>
                          <label>
                            <span>Auth</span>
                            <select value={provider().auth.startsWith('env:') ? 'env' : provider().auth}
                              onChange={(e) => {
                                const v = e.currentTarget.value;
                                updateProvider(idx(), 'auth', v === 'env' ? 'env:API_KEY' : v);
                              }}>
                              <option value="none">None</option>
                              <option value="github-oauth">GitHub OAuth</option>
                              <option value="api-key">API Key</option>
                              <option value="env">Environment variable</option>
                            </select>
                          </label>
                          <Show when={provider().auth.startsWith('env:')}>
                            <label>
                              <span>Env variable</span>
                              <input type="text" value={provider().auth.replace('env:', '')} onChange={(e) => updateProvider(idx(), 'auth', `env:${e.currentTarget.value}`)} />
                            </label>
                          </Show>
                          <label>
                            <span>Base URL</span>
                            <input type="text" placeholder="Default for provider kind" value={provider().base_url ?? ''} onChange={(e) => updateProvider(idx(), 'base_url', e.currentTarget.value || null)} />
                          </label>
                        </Show>
                      </div>

                      {/* Models section with per-model capabilities - for providers without specialized model sections */}
                      <Show when={!['local-models', 'github-copilot', 'anthropic', 'open-ai-compatible', 'microsoft-foundry'].includes(provider().kind)}>
                        <div class="settings-subsection">
                          <strong>Models</strong>
                          <div style="display: flex; flex-direction: column; gap: 8px;">
                            <For each={provider().models}>
                              {(modelId, mIdx) => (
                                <div style="padding: 8px 10px; border-radius: 6px; border: 1px solid hsl(var(--border)); background: hsl(var(--card)); display: flex; align-items: center; gap: 8px; flex-wrap: wrap;">
                                  <span style="font-weight: 600; min-width: 120px;">{modelId}</span>
                                  <button class="settings-tag-remove" onClick={() => removeModelFromProvider(idx(), mIdx())}>×</button>
                                  <div style="display: flex; flex-wrap: wrap; gap: 6px; margin-left: auto;">
                                    {ALL_CAPABILITIES.map((cap) => (
                                      <Switch checked={((provider().model_capabilities ?? {})[modelId] ?? []).includes(cap)} onChange={() => handleToggleCap(modelId, cap)} class="flex items-center gap-2">
                                        <SwitchControl><SwitchThumb /></SwitchControl>
                                        <SwitchLabel>{cap}</SwitchLabel>
                                      </Switch>
                                    ))}
                                  </div>
                                </div>
                              )}
                            </For>
                          </div>
                          <div class="settings-inline-add">
                            <input type="text" placeholder="Add model…" value={newModelInput()} onInput={(e) => setNewModelInput(e.currentTarget.value)}
                              onKeyDown={(e) => { if (e.key === 'Enter') { const m = newModelInput(); addModelToProvider(idx(), m); setNewModelInput(''); if (m.trim()) autoPopulateModelCaps(idx(), m.trim()); } }} />
                            <button onClick={() => { const m = newModelInput(); addModelToProvider(idx(), m); setNewModelInput(''); if (m.trim()) autoPopulateModelCaps(idx(), m.trim()); }}>+</button>
                          </div>
                        </div>
                      </Show>

                      {/* GitHub Copilot models section */}
                      <Show when={provider().kind === 'github-copilot'}>
                        <div class="settings-subsection">
                          <strong>Models</strong>
                          <Show when={availableCopilotModels().length > 0} fallback={
                            <p class="muted" style="font-size: 0.85em;">
                              {provider().auth === 'github-oauth' ? 'No models loaded. Models will appear after connecting.' : 'Connect your GitHub account above to see available models.'}
                            </p>
                          }>
                            <FetchedModelList
                              allModels={sortedCopilotModelIds()}
                              enabledModels={provider().models}
                              modelCaps={provider().model_capabilities ?? {}}
                              onToggleModel={handleToggleModel}
                              onToggleCap={handleToggleCap}
                              onAddManual={handleAddManual}
                              onRemoveManual={handleRemoveManual}
                              renderLabel={(modelId) => {
                                const name = copilotModelNames()[modelId];
                                return <><span style="font-weight: 600;">{modelId}</span>{name ? <span class="muted" style="font-size: 0.8em; margin-left: 6px;">{name}</span> : null}</>;
                              }}
                            />
                          </Show>
                        </div>
                      </Show>

                      {/* Anthropic models section */}
                      <Show when={provider().kind === 'anthropic'}>
                        <div class="settings-subsection">
                          <strong>Models</strong>
                          <FetchedModelList
                            allModels={sortedAnthropicModels()}
                            enabledModels={provider().models}
                            modelCaps={provider().model_capabilities ?? {}}
                            onToggleModel={handleToggleModel}
                            onToggleCap={handleToggleCap}
                            onAddManual={handleAddManual}
                            onRemoveManual={handleRemoveManual}
                            showFallbackTags
                          />
                        </div>
                      </Show>

                      {/* OpenAI models section */}
                      <Show when={provider().kind === 'open-ai-compatible'}>
                        <div class="settings-subsection">
                          <strong>Models</strong>
                          <FetchedModelList
                            allModels={sortedOpenAiModels()}
                            enabledModels={provider().models}
                            modelCaps={provider().model_capabilities ?? {}}
                            onToggleModel={handleToggleModel}
                            onToggleCap={handleToggleCap}
                            onAddManual={handleAddManual}
                            onRemoveManual={handleRemoveManual}
                            showFallbackTags
                          />
                        </div>
                      </Show>

                      {/* Microsoft Foundry models section */}
                      <Show when={provider().kind === 'microsoft-foundry'}>
                        <div class="settings-subsection">
                          <strong>Models</strong>
                          <FetchedModelList
                            allModels={sortedFoundryModels()}
                            enabledModels={provider().models}
                            modelCaps={provider().model_capabilities ?? {}}
                            onToggleModel={handleToggleModel}
                            onToggleCap={handleToggleCap}
                            onAddManual={handleAddManual}
                            onRemoveManual={handleRemoveManual}
                            showFallbackTags
                          />
                        </div>
                      </Show>

                      {/* Local Models installed models section */}
                      <Show when={provider().kind === 'local-models'}>
                        <div class="settings-subsection">
                          <strong>Installed Models</strong>
                          <p class="muted" style="font-size: 0.8em; margin: 0.25rem 0 0.5rem;">
                            Select which models to enable and set capabilities for each.
                          </p>
                          <Show when={localModels().length > 0} fallback={
                            <p class="muted" style="font-size: 0.85em;">No models installed. Go to Local Models → Browse Hub to download models.</p>
                          }>
                            <div style="display: flex; flex-direction: column; gap: 8px;">
                              <For each={localModels()}>
                                {(m) => {
                                  const toggleLocal = (checked: boolean) => {
                                    if (provider().models.length === 0 && !checked) {
                                      const all = localModels().map(lm => lm.id).filter(id => id !== m.id);
                                      updateProvider(idx(), 'models', all);
                                    } else if (checked) {
                                      addModelToProvider(idx(), m.id);
                                      if (!(provider().model_capabilities ?? {})[m.id]) {
                                        const autoCaps = modelCapsToProviderCaps(m);
                                        if (autoCaps.length > 0) {
                                          const mc = { ...(provider().model_capabilities ?? {}), [m.id]: autoCaps };
                                          updateProvider(idx(), 'model_capabilities', mc);
                                        }
                                      }
                                    } else {
                                      const mIdx = provider().models.indexOf(m.id);
                                      if (mIdx >= 0) removeModelFromProvider(idx(), mIdx);
                                    }
                                  };
                                  return (
                                    <ModelToggleItem
                                      modelId={m.id}
                                      label={<><span style="font-weight: 600;">{displayModelName(m)}</span> <span class="muted" style="font-size: 0.8em;">({m.runtime})</span></>}
                                      enabled={provider().models.length === 0 || provider().models.includes(m.id)}
                                      caps={((provider().model_capabilities ?? {})[m.id] ?? []) as CapabilityOption[]}
                                      onToggle={toggleLocal}
                                      onToggleCap={(cap) => handleToggleCap(m.id, cap)}
                                    />
                                  );
                                }}
                              </For>
                            </div>
                            <p class="muted" style="font-size: 0.8em; margin-top: 0.25rem;">
                              {provider().models.length === 0 ? '✓ Using all installed models' : `${provider().models.length} model(s) selected`}
                            </p>
                          </Show>
                        </div>
                      </Show>
                      </div>{/* end scrollable content */}

                      <div class="flex items-center justify-between mt-4 pt-4 border-t border-border shrink-0">
                        <Show when={!props.hideDelete}>
                          <button
                            class="text-xs text-muted-foreground hover:text-destructive transition-colors cursor-pointer"
                            onClick={() => { removeProvider(idx()); setEditingProviderIdx(null); }}
                          >
                            Delete provider
                          </button>
                        </Show>
                        <Show when={props.hideDelete}><span /></Show>
                        <div class="flex gap-2">
                          <Button variant="outline" onClick={handleCancel}>Cancel</Button>
                          <Button onClick={handleSave}>Save Changes</Button>
                        </div>
                      </div>
                  </>
                );
              })()}
            </Show>
            </ErrorBoundary>
            </DialogContent></Dialog>
          </>
      </Show>
    </div>
  );
}
