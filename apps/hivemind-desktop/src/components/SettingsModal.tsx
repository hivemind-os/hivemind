import { For, Show, createEffect, on, createSignal, onMount, onCleanup, lazy, Suspense, type Accessor, type Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { ClipboardList, TriangleAlert, Settings as SettingsIcon, Key, Wrench, Brain, Eye, MessageSquare, Heart, Building2, ScrollText, Globe, BookOpen, Link, XCircle, Zap, ChevronUp, ChevronDown, ChevronRight, X } from 'lucide-solid';
import { currentTheme, setTheme, availableThemes } from '../stores/themeStore';
import { Tabs, TabsIndicator, TabsList, TabsTrigger } from '~/ui/tabs';
import { Dialog, DialogContent, DialogFooter } from '~/ui/dialog';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import PersonasTab from './PersonasTab';
import { Collapsible, CollapsibleContent } from '~/ui/collapsible';
import ProvidersTab from './settings/ProvidersTab';
import RecordingsTab from './settings/RecordingsTab';
import CompactionTab from './settings/CompactionTab';
import RuntimeTab from './settings/RuntimeTab';
import GeneralTab from './settings/GeneralTab';
const ConnectorsTab = lazy(() => import('./ConnectorsTab'));
const AuditViewer = lazy(() => import('./AuditViewer'));
import type {
  AppContext,
  CapabilityOption,
  DaemonStatus,
  DiscoveredSkill,
  DownloadProgress,
  HiveMindConfigData,
  HardwareInfo,
  HubFileInfo,
  HubModelInfo,
  InferenceParams,
  InstalledModel,
  InstalledSkill,
  PolicyAction,
  RuntimeResourceUsage,
  SkillAuditResult,
  SkillSourceConfig,
  ToolDefinition,
  InstallableItem,
} from '../types';
import { extractFileQuantization } from '../types';
import { parseModelMeta, displayModelName, isEmbeddingOnly, modelCapsToProviderCaps } from '../types';
import { formatBytes, formatPayload, formatTime, mcpStatusClass } from '../utils';

type SettingsTab = 'general-appearance' | 'general-daemon' | 'general-recording' | 'providers' | 'security' | 'mcp' | 'local-models' | 'scheduler' | 'downloads' | 'tools' | 'personas' | 'compaction' | 'channels' | 'comm-audit' | 'afk' | 'python' | 'node' | 'web-search';
type LocalModelView = 'library' | 'search' | 'hardware';

interface SettingsCategory {
  id: string;
  label: string;
  tabs: { id: SettingsTab; label: string }[];
}

const SETTINGS_CATEGORIES: SettingsCategory[] = [
  { id: 'general', label: 'General', tabs: [
    { id: 'general-appearance', label: 'Appearance' },
    { id: 'general-daemon', label: 'Daemon' },
    { id: 'general-recording', label: 'Event Recording' },
  ]},
  { id: 'agents-automation', label: 'Agents & Automation', tabs: [
    { id: 'personas', label: 'Personas' },
    { id: 'channels', label: 'Connectors' },
    { id: 'afk', label: 'AFK / Status' },
  ]},
  { id: 'ai-models', label: 'AI & Models', tabs: [
    { id: 'providers', label: 'Providers' },
    { id: 'local-models', label: 'Local Models' },
    { id: 'downloads', label: 'Downloads' },
    { id: 'compaction', label: 'Compaction' },
  ]},
  { id: 'extensions', label: 'Extensions', tabs: [
    { id: 'tools', label: 'Tools' },
    { id: 'web-search', label: 'Web Search' },
    { id: 'python', label: 'Python' },
    { id: 'node', label: 'Node.js' },
  ]},
  { id: 'security', label: 'Security', tabs: [
    { id: 'security', label: 'Policies' },
    { id: 'comm-audit', label: 'Audit Log' },
  ]},
];

function categoryForTab(tab: SettingsTab): string {
  for (const cat of SETTINGS_CATEGORIES) {
    if (cat.tabs.some(t => t.id === tab)) return cat.id;
  }
  return SETTINGS_CATEGORIES[0].id;
}

export interface SettingsModalProps {
  // Config state
  cfg: Accessor<HiveMindConfigData | null>;
  setEditConfig: Setter<HiveMindConfigData | null>;
  configDirty: Accessor<boolean>;
  saveConfig: () => Promise<void>;
  loadEditConfig: () => Promise<void>;
  configLoadError: Accessor<string | null>;
  configSaveMsg: Accessor<string | null>;
  editingProviderIdx: Accessor<number | null>;
  setEditingProviderIdx: Setter<number | null>;

  // Close handler — navigates away from settings
  onClose?: () => void;

  // App/daemon state
  context: Accessor<AppContext | null>;
  daemonOnline: Accessor<boolean>;
  daemonStatus: Accessor<DaemonStatus | null>;
  busyAction: Accessor<string | null>;
  settingsTab: Accessor<SettingsTab>;
  setSettingsTab: Setter<SettingsTab>;

  // Config update helpers
  updateDaemon: (field: string, value: unknown) => void;
  updateApi: (field: string, value: unknown) => void;
  updateOverridePolicy: (field: string, value: PolicyAction) => void;
  updatePromptInjection: (field: string, value: unknown) => void;
  updateLocalModels: (field: string, value: unknown) => void;
  updateCompaction: (field: string, value: unknown) => void;
  updateAfk: (field: string, value: unknown) => void;

  // Provider management
  addProvider: () => void;
  removeProvider: (idx: number) => void;
  moveProvider: (fromIdx: number, toIdx: number) => void;
  updateProvider: (idx: number, field: string, value: unknown) => void;
  addModelToProvider: (idx: number, model: string) => void;
  removeModelFromProvider: (pIdx: number, mIdx: number) => void;

  // Local model state and actions
  localModels: Accessor<InstalledModel[]>;
  localModelView: Accessor<LocalModelView>;
  setLocalModelView: Setter<LocalModelView>;
  storageBytes: Accessor<number>;
  expandedModel: Accessor<string | null>;
  setExpandedModel: Setter<string | null>;
  loadLocalModels: () => Promise<void>;
  loadHardwareInfo: () => Promise<void>;
  updateModelParamsDebounced: (modelId: string, params: InferenceParams) => void;
  removeModel: (modelId: string) => Promise<void>;
  hardwareInfo: Accessor<HardwareInfo | null>;
  resourceUsage: Accessor<RuntimeResourceUsage | null>;

  // Hub search / install state
  hubSearchResults: Accessor<HubModelInfo[]>;
  hubSearchQuery: Accessor<string>;
  setHubSearchQuery: Setter<string>;
  hubSearchLoading: Accessor<boolean>;
  hubSearchError: Accessor<string | null>;
  searchHubModels: () => Promise<void>;
  installTargetRepo: Accessor<HubModelInfo | null>;
  setInstallTargetRepo: Setter<HubModelInfo | null>;
  installRepoFiles: Accessor<HubFileInfo[]>;
  installableItems: Accessor<InstallableItem[]>;
  installFilesLoading: Accessor<boolean>;
  installInProgress: Accessor<boolean>;
  openInstallDialog: (model: HubModelInfo) => Promise<void>;
  installModelFile: (repo_id: string, filename: string) => Promise<void>;
  inferRuntime: (filename: string) => string;

  // Downloads / tools
  activeDownloads: Accessor<DownloadProgress[]>;
  setActiveDownloads: Setter<DownloadProgress[]>;
  startDownloadPolling: () => void;
  toolDefinitions: Accessor<ToolDefinition[]>;
  connectors: Accessor<{ id: string; name: string; provider: string; hasComms: boolean }[]>;
  loadPersonas: () => Promise<void>;

  // Models
  availableModels: { id: string; label: string }[];

  // Misc helpers
  isNoTokenError: (msg: string) => boolean;
  isLicenseError: (msg: string) => boolean;
  extractRepoFromError: (msg: string) => string | null;
  openExternal: (url: string) => Promise<void>;
  scrollToHfToken: () => void;

  // Agent Kit integration
  onExportPersonaToKit?: (persona_id: string) => void;
}

const SettingsModal = (props: SettingsModalProps) => {
  const editConfig = props.cfg;
  const setEditConfig = props.setEditConfig;
  const configDirty = props.configDirty;
  const saveConfig = props.saveConfig;
  const loadEditConfig = props.loadEditConfig;
  const configLoadError = props.configLoadError;
  const configSaveMsg = props.configSaveMsg;
  const editingProviderIdx = props.editingProviderIdx;
  const setEditingProviderIdx = props.setEditingProviderIdx;
  const context = props.context;
  const daemonOnline = props.daemonOnline;
  const daemonStatus = props.daemonStatus;
  const busyAction = props.busyAction;
  const settingsTab = props.settingsTab;
  const setSettingsTab = props.setSettingsTab;

  let modalRef: HTMLDivElement | undefined;

  // Close on Escape key
  onMount(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && props.onClose) {
        e.preventDefault();
        props.onClose();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    onCleanup(() => window.removeEventListener('keydown', handleKeyDown));
  });

  // Prevent browser-native scroll-into-view (triggered by Kobalte Switch
  // focusing its hidden input on label click) from scrolling ancestor
  // overflow:hidden containers. We intercept scroll events on all ancestors
  // and reset scrollTop to 0, since the settings modal manages its own
  // scrolling via .settings-content overflow-y:auto.
  onMount(() => {
    if (!modalRef) return;
    const handlers: Array<[HTMLElement, EventListener]> = [];
    // Start from modalRef itself (not just parentElement) since
    // .settings-modal has overflow:hidden which is scrollable.
    let el: HTMLElement | null = modalRef;
    while (el) {
      const target = el;
      const handler = () => { if (target.scrollTop !== 0) target.scrollTop = 0; };
      target.addEventListener('scroll', handler);
      handlers.push([target, handler]);
      el = el.parentElement;
    }
    onCleanup(() => {
      for (const [target, handler] of handlers) {
        target.removeEventListener('scroll', handler);
      }
    });
  });

  // Local tab signal for content switching — bypasses Kobalte's
  // internal state which has reactivity issues with solid-presence.
  const initialTab = settingsTab();
  const [localTab, setLocalTab] = createSignal<SettingsTab>(initialTab);
  const [expandedCategories, setExpandedCategories] = createSignal<Set<string>>(
    new Set([categoryForTab(initialTab)])
  );

  // Sync external settingsTab changes (e.g. App.tsx switching to 'downloads'
  // after starting a model install) into the local tab state.
  // Uses `on()` to only track settingsTab — avoids tracking localTab which
  // would cause a bounce when switchTab sets both signals.
  createEffect(on(settingsTab, (tab) => {
    setLocalTab(tab);
    const catId = categoryForTab(tab);
    setExpandedCategories(prev => {
      if (prev.has(catId)) return prev;
      const next = new Set(prev);
      next.add(catId);
      return next;
    });
  }, { defer: true }));
  const toggleCategory = (catId: string) => {
    const isExpanded = expandedCategories().has(catId);
    setExpandedCategories(prev => {
      const next = new Set(prev);
      if (next.has(catId)) next.delete(catId);
      else next.add(catId);
      return next;
    });
    if (!isExpanded) {
      const cat = SETTINGS_CATEGORIES.find(c => c.id === catId);
      if (cat?.tabs?.length) switchTab(cat.tabs[0].id);
    }
  };
  const switchTab = (tab: SettingsTab) => {
    setLocalTab(tab);
    setSettingsTab(tab);
    // Auto-expand the category containing this tab
    const catId = categoryForTab(tab);
    setExpandedCategories(prev => {
      if (prev.has(catId)) return prev;
      const next = new Set(prev);
      next.add(catId);
      return next;
    });
  };

  const configLoadingFallback = (label: string) => (
    <Show when={configLoadError()} fallback={
      <p class="muted">{daemonOnline() ? `Loading ${label}…` : `Start the daemon to edit ${label}.`}</p>
    }>
      <div class="flex flex-col items-start gap-2 py-2">
        <p class="text-sm text-destructive">Failed to load configuration: {configLoadError()}</p>
        <button class="primary" style="font-size:0.85rem;padding:0.35rem 0.75rem" onClick={() => void loadEditConfig()}>Retry</button>
      </div>
    </Show>
  );

  const updateDaemon = props.updateDaemon;
  const updateApi = props.updateApi;
  const updateOverridePolicy = props.updateOverridePolicy;
  const updatePromptInjection = props.updatePromptInjection;
  const updateLocalModels = props.updateLocalModels;
  const updateCompaction = props.updateCompaction;
  const updateAfk = props.updateAfk;
  const addProvider = props.addProvider;
  const removeProvider = props.removeProvider;
  const moveProvider = props.moveProvider;
  const updateProvider = props.updateProvider;
  const addModelToProvider = props.addModelToProvider;
  const removeModelFromProvider = props.removeModelFromProvider;
  const localModels = props.localModels;
  const localModelView = props.localModelView;
  const setLocalModelView = props.setLocalModelView;
  const storageBytes = props.storageBytes;
  const expandedModel = props.expandedModel;
  const setExpandedModel = props.setExpandedModel;
  const loadLocalModels = props.loadLocalModels;
  const loadHardwareInfo = props.loadHardwareInfo;
  const updateModelParamsDebounced = props.updateModelParamsDebounced;
  const removeModel = props.removeModel;
  const hardwareInfo = props.hardwareInfo;
  const resourceUsage = props.resourceUsage;
  const hubSearchResults = props.hubSearchResults;
  const hubSearchQuery = props.hubSearchQuery;
  const setHubSearchQuery = props.setHubSearchQuery;
  const hubSearchLoading = props.hubSearchLoading;
  const hubSearchError = props.hubSearchError;
  const searchHubModels = props.searchHubModels;
  const installTargetRepo = props.installTargetRepo;
  const setInstallTargetRepo = props.setInstallTargetRepo;
  const installRepoFiles = props.installRepoFiles;
  const installableItems = props.installableItems;
  const installFilesLoading = props.installFilesLoading;
  const installInProgress = props.installInProgress;
  const openInstallDialog = props.openInstallDialog;
  const installModelFile = props.installModelFile;
  const inferRuntime = props.inferRuntime;
  const activeDownloads = props.activeDownloads;
  const setActiveDownloads = props.setActiveDownloads;
  const startDownloadPolling = props.startDownloadPolling;
  const toolDefinitions = props.toolDefinitions;
  const loadPersonas = props.loadPersonas;
  const isNoTokenError = props.isNoTokenError;
  const isLicenseError = props.isLicenseError;
  const extractRepoFromError = props.extractRepoFromError;
  const openExternal = props.openExternal;
  const scrollToHfToken = props.scrollToHfToken;

  // HuggingFace token state (hoisted from keyed Show)
  const [hfToken, setHfToken] = createSignal<string>('');
  const [hfTokenLoaded, setHfTokenLoaded] = createSignal(false);
  // Load HF token from OS keyring on mount
  onMount(() => {
    invoke<string | null>('load_secret', { key: 'hf_token' })
      .then((val) => { setHfToken(val ?? ''); setHfTokenLoaded(true); })
      .catch(() => { setHfToken(''); setHfTokenLoaded(true); });
  });
  const saveHfToken = async (value: string) => {
    setHfToken(value);
    try {
      if (value) {
        await invoke('save_secret', { key: 'hf_token', value });
      } else {
        await invoke('delete_secret', { key: 'hf_token' });
      }
    } catch (e) { console.error('Failed to save HF token:', e); }
  };

  // Web search API key state — stored in OS keyring, not plaintext config
  const [webSearchApiKey, setWebSearchApiKey] = createSignal<string>('');
  onMount(() => {
    invoke<string | null>('load_secret', { key: 'web-search:api-key' })
      .then((val) => { if (val) setWebSearchApiKey(val); })
      .catch(() => {});
  });

  return (
          <div ref={modalRef} class="settings-modal" data-testid="settings-modal">
            <header class="settings-header">
              <h2>Settings</h2>
              <div class="settings-header-actions">
                <Show when={configDirty()}>
                  <span class="pill processing">unsaved changes</span>
                </Show>
                <Show when={configSaveMsg()}>
                  <span class="pill neutral">{configSaveMsg()}</span>
                </Show>
                <Button variant="outline" data-testid="settings-discard-btn" aria-label="Discard changes" disabled={!configDirty()} onClick={() => void loadEditConfig()}>Discard</Button>
                <Button data-testid="settings-save-btn" aria-label="Save settings" disabled={!configDirty()} onClick={async () => {
                  // Save web search API key to OS keyring before config save
                  const key = webSearchApiKey();
                  if (key && !key.startsWith('env:')) {
                    try { await invoke('save_secret', { key: 'web-search:api-key', value: key }); }
                    catch (e) { console.error('Failed to save web search key:', e); }
                  } else if (!key) {
                    try { await invoke('delete_secret', { key: 'web-search:api-key' }); }
                    catch { /* ignore */ }
                  }
                  await saveConfig();
                }}>Save</Button>
                <Show when={props.onClose}>
                  <Button variant="ghost" data-testid="settings-close-btn" aria-label="Close settings" onClick={() => props.onClose?.()}>✕</Button>
                </Show>
              </div>
            </header>

              <div class="settings-body" data-orientation="vertical">
                <div class="settings-tabs" role="tablist" aria-orientation="vertical">
                  <For each={SETTINGS_CATEGORIES}>{(cat) => (
                    <div class="settings-category" data-testid={`settings-category-${cat.id}`}>
                      <button
                        class="settings-category-header"
                        classList={{ 'settings-category-header-active': cat.tabs.some(t => t.id === localTab()) }}
                        onClick={() => toggleCategory(cat.id)}
                      >
                        <Show when={expandedCategories().has(cat.id)} fallback={<ChevronRight size={14} />}>
                          <ChevronDown size={14} />
                        </Show>
                        {cat.label}
                      </button>
                      <Show when={expandedCategories().has(cat.id)}>
                        <For each={cat.tabs}>{(tab) => (
                          <button
                            role="tab"
                            class="settings-trigger settings-trigger-nested"
                            classList={{ 'settings-trigger-active': localTab() === tab.id }}
                            data-testid={`settings-tab-${tab.id}`}
                            aria-selected={localTab() === tab.id}
                            data-selected={localTab() === tab.id ? '' : undefined}
                            on:click={() => switchTab(tab.id)}
                          >
                            {tab.label}
                            <Show when={tab.id === 'downloads' && activeDownloads().length > 0}>
                              <span class="download-count-badge">{activeDownloads().length}</span>
                            </Show>
                          </button>
                        )}</For>
                      </Show>
                    </div>
                  )}</For>
                </div>
              <div class="settings-content">
                {/* ── Appearance ─────────────────────────────────── */}
                <Show when={localTab() === 'general-appearance'}>
                  <div class="settings-section">
                    <h3>Appearance</h3>
                    <div class="settings-form">
                      <label>
                        <span>Theme</span>
                        <select value={currentTheme()} onChange={(e) => setTheme(e.currentTarget.value as any)}>
                          <For each={availableThemes}>
                            {(t) => <option value={t.name}>{t.label}</option>}
                          </For>
                        </select>
                      </label>
                    </div>
                  </div>
                </Show>

                {/* ── Daemon ─────────────────────────────────────── */}
                <Show when={localTab() === 'general-daemon'}>
                  <div class="settings-section">
                    <h3>Daemon Status</h3>
                    <dl class="details">
                      <div><dt>Status</dt><dd><span class={`pill ${daemonOnline() ? 'online' : 'offline'}`}>{daemonOnline() ? 'Online' : 'Offline'}</span></dd></div>
                      <div><dt>Version</dt><dd>{daemonStatus()?.version ?? '—'}</dd></div>
                      <div><dt>PID</dt><dd>{daemonStatus()?.pid ?? '—'}</dd></div>
                      <div><dt>Uptime</dt><dd>{daemonStatus() ? `${Math.round(daemonStatus()!.uptime_secs)}s` : '—'}</dd></div>
                    </dl>

                    <Show when={editConfig()} fallback={configLoadingFallback('configuration')}>
                        <>
                          <h3>Daemon Configuration</h3>
                          <div class="settings-form">
                            <label>
                              <span>Log level</span>
                              <select value={editConfig()!.daemon.log_level} onChange={(e) => updateDaemon('log_level', e.currentTarget.value)}>
                                <option value="trace">trace</option>
                                <option value="debug">debug</option>
                                <option value="info">info</option>
                                <option value="warn">warn</option>
                                <option value="error">error</option>
                              </select>
                            </label>
                            <label>
                              <span>Event bus capacity</span>
                              <input type="number" min="16" max="65536" value={editConfig()!.daemon.event_bus_capacity} onChange={(e) => updateDaemon('event_bus_capacity', parseInt(e.currentTarget.value) || 512)} />
                            </label>
                          </div>

                          <h3>API</h3>
                          <div class="settings-form">
                            <label>
                              <span>Bind address</span>
                              <input type="text" value={editConfig()!.api.bind} onChange={(e) => updateApi('bind', e.currentTarget.value)} />
                            </label>
                            <Switch checked={editConfig()!.api.http_enabled} onChange={(checked) => updateApi('http_enabled', checked)} class="flex items-center gap-2">
                              <SwitchControl><SwitchThumb /></SwitchControl>
                              <SwitchLabel>HTTP enabled</SwitchLabel>
                            </Switch>
                          </div>

                          <h3>Paths</h3>
                          <dl class="details single-column">
                            <div><dt>Daemon URL</dt><dd>{context()?.daemon_url ?? '—'}</dd></div>
                            <div><dt>Config file</dt><dd>{context()?.config_path ?? '—'}</dd></div>
                            <div><dt>Knowledge graph</dt><dd>{context()?.knowledge_graph_path ?? '—'}</dd></div>
                            <div><dt>Risk ledger</dt><dd>{context()?.risk_ledger_path ?? '—'}</dd></div>
                          </dl>
                        </>
                    </Show>
                  </div>
                </Show>

                {/* ── Event Recording ────────────────────────────── */}
                <Show when={localTab() === 'general-recording'}>
                  <RecordingsTab
                    editConfig={editConfig}
                    daemonOnline={daemonOnline}
                    active={() => localTab() === 'general-recording'}
                    configLoadingFallback={configLoadingFallback}
                  />
                </Show>

                {/* ── Providers ───────────────────────────────────── */}
                <Show when={localTab() === 'providers'}>
                  <ProvidersTab
                    editConfig={editConfig}
                    editingProviderIdx={editingProviderIdx}
                    setEditingProviderIdx={setEditingProviderIdx}
                    addProvider={addProvider}
                    removeProvider={removeProvider}
                    moveProvider={moveProvider}
                    updateProvider={updateProvider}
                    addModelToProvider={addModelToProvider}
                    removeModelFromProvider={removeModelFromProvider}
                    saveConfig={saveConfig}
                    localModels={localModels}
                    context={context}
                    openExternal={openExternal}
                    configLoadingFallback={configLoadingFallback}
                  />
                </Show>


                {/* ── Security ────────────────────────────────────── */}
                <Show when={localTab() === 'security'}>
                  <div class="settings-section">
                    <Show when={editConfig()} fallback={configLoadingFallback('security settings')}>
                        <>
                          <h3>Override Policy</h3>
                          <p class="muted">When data at a given classification level needs to cross a channel that normally wouldn't allow it, this policy determines the action.</p>
                          <div class="settings-form">
                            {(['internal', 'confidential', 'restricted'] as const).map((level) => (
                              <label>
                                <span>{level.charAt(0).toUpperCase() + level.slice(1)}</span>
                                <select value={editConfig()!.security.override_policy[level]}
                                  onChange={(e) => updateOverridePolicy(level, e.currentTarget.value as PolicyAction)}>
                                  <option value="block">Block</option>
                                  <option value="prompt">Prompt user</option>
                                  <option value="warn">Warn — log and continue</option>
                                  <option value="allow">Allow</option>
                                  <option value="redact-and-send">Redact and send</option>
                                </select>
                              </label>
                            ))}
                          </div>

                          <h3 style="margin-top: 1.5rem;">Prompt Injection Protection</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">
                            Scans incoming data for prompt injection attacks. The heuristic scanner runs locally with zero latency.
                          </p>
                          <div class="settings-form">
                            <Switch checked={editConfig()!.security.prompt_injection.enabled} onChange={(checked) => updatePromptInjection('enabled', checked)} class="flex items-center gap-2">

                              <SwitchControl><SwitchThumb /></SwitchControl>

                              <SwitchLabel>Scanning enabled</SwitchLabel>

                            </Switch>
                            <label>
                              <span>Action on detection</span>
                              <select value={editConfig()!.security.prompt_injection.action_on_detection}
                                onChange={(e) => updatePromptInjection('action_on_detection', e.currentTarget.value)}>
                                <option value="block">Block — reject content silently</option>
                                <option value="prompt">Prompt user — ask before proceeding</option>
                                <option value="warn">Warn — log and continue</option>
                                <option value="flag">Flag only — annotate but deliver</option>
                                <option value="allow">Allow — log for audit only</option>
                              </select>
                            </label>
                            <label>
                              <span>Confidence threshold</span>
                              <input type="number" min="0" max="1" step="0.05" value={editConfig()!.security.prompt_injection.confidence_threshold}
                                onChange={(e) => updatePromptInjection('confidence_threshold', parseFloat(e.currentTarget.value) || 0.7)} />
                            </label>
                            <label>
                              <span>Cache TTL (seconds)</span>
                              <input type="number" min="0" value={editConfig()!.security.prompt_injection.cache_ttl_secs}
                                onChange={(e) => updatePromptInjection('cache_ttl_secs', parseInt(e.currentTarget.value) || 3600)} />
                            </label>
                          </div>

                          <h3 style="margin-top: 1.5rem;">LLM-Based Scanning</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">
                            Use an LLM model for deeper analysis. More accurate than heuristic scanning but adds latency and token cost for each scanned payload.
                          </p>
                          <div class="settings-form">
                            <Switch checked={editConfig()!.security.prompt_injection.model_scanning_enabled ?? false} onChange={(checked) => updatePromptInjection('model_scanning_enabled', checked)} class="flex items-center gap-2">

                              <SwitchControl><SwitchThumb /></SwitchControl>

                              <SwitchLabel>Enable model-based scanning</SwitchLabel>

                            </Switch>
                            <Show when={editConfig()!.security.prompt_injection.model_scanning_enabled}>
                              <p class="muted text-yellow-400" style="font-size:11px;margin:4px 0 8px;">
                                ⚠️ Each scan makes an LLM call. Enable caching and batching below to reduce overhead.
                              </p>
                              <label>
                                <span>Max payload tokens</span>
                                <input type="number" min="256" max="32768" step="256" value={editConfig()!.security.prompt_injection.max_payload_tokens ?? 4096}
                                  onChange={(e) => updatePromptInjection('max_payload_tokens', parseInt(e.currentTarget.value) || 4096)} />
                              </label>
                              <Switch checked={editConfig()!.security.prompt_injection.batch_small_payloads ?? true} onChange={(checked) => updatePromptInjection('batch_small_payloads', checked)} class="flex items-center gap-2">

                                <SwitchControl><SwitchThumb /></SwitchControl>

                                <SwitchLabel>Batch small payloads — combine into fewer LLM calls</SwitchLabel>

                              </Switch>
                            </Show>
                          </div>

                          <Show when={editConfig()!.security.prompt_injection.model_scanning_enabled}>
                          <h3 style="margin-top: 1.5rem;">Scanner Models</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">
                            Models used for prompt-injection scanning, in priority order. The first available model will be used.
                          </p>
                          {(() => {
                            const models = () => editConfig()!.security.prompt_injection.scanner_models ?? [];
                            const allModels = () => {
                              const result: { provider: string; providerName: string; model: string }[] = [];
                              for (const p of editConfig()!.models.providers) {
                                if (!p.enabled) continue;
                                if (p.kind === 'local-models') {
                                  for (const lm of localModels()) {
                                    result.push({ provider: p.id, providerName: p.name || p.id, model: lm.id });
                                  }
                                } else {
                                  for (const m of p.models) {
                                    result.push({ provider: p.id, providerName: p.name || p.id, model: m });
                                  }
                                }
                              }
                              return result;
                            };
                            const setScannerModels = (updated: { provider: string; model: string }[]) => {
                              setEditConfig((c) => c ? {
                                ...c,
                                security: {
                                  ...c.security,
                                  prompt_injection: { ...c.security.prompt_injection, scanner_models: updated },
                                },
                              } : null);
                            };
                            return (
                              <div>
                                <Show when={models().length > 0}>
                                  <table style="width:100%;border-collapse:collapse;font-size:13px;margin-bottom:8px;">
                                    <thead>
                                      <tr style="border-bottom:1px solid hsl(var(--border));">
                                        <th style="text-align:left;padding:4px 6px;">#</th>
                                        <th style="text-align:left;padding:4px 6px;">Provider</th>
                                        <th style="text-align:left;padding:4px 6px;">Model</th>
                                        <th style="width:80px;"></th>
                                      </tr>
                                    </thead>
                                    <tbody>
                                      <For each={models()}>
                                        {(entry, idx) => {
                                          const provName = () => editConfig()!.models.providers.find(p => p.id === entry.provider)?.name || entry.provider;
                                          return (
                                            <tr style="border-bottom:1px solid hsl(var(--border));">
                                              <td style="padding:4px 6px;">{idx() + 1}</td>
                                              <td style="padding:4px 6px;">{provName()}</td>
                                              <td style="padding:4px 6px;">{entry.model}</td>
                                              <td style="padding:4px 6px;display:flex;gap:4px;">
                                                <button disabled={idx() === 0} onClick={() => {
                                                  const arr = [...models()];
                                                  [arr[idx() - 1], arr[idx()]] = [arr[idx()], arr[idx() - 1]];
                                                  setScannerModels(arr);
                                                }} title="Move up"><ChevronUp size={14} /></button>
                                                <button disabled={idx() === models().length - 1} onClick={() => {
                                                  const arr = [...models()];
                                                  [arr[idx()], arr[idx() + 1]] = [arr[idx() + 1], arr[idx()]];
                                                  setScannerModels(arr);
                                                }} title="Move down"><ChevronDown size={14} /></button>
                                                <button onClick={() => {
                                                  setScannerModels(models().filter((_, i) => i !== idx()));
                                                }}>✕</button>
                                              </td>
                                            </tr>
                                          );
                                        }}
                                      </For>
                                    </tbody>
                                  </table>
                                </Show>
                                {(() => {
                                  const [addProvider, setAddProvider] = createSignal('');
                                  const [addModel, setAddModel] = createSignal('');
                                  const availableForProvider = () => {
                                    const pid = addProvider();
                                    if (!pid) return [];
                                    const p = editConfig()!.models.providers.find(pr => pr.id === pid);
                                    if (!p) return [];
                                    if (p.kind === 'local-models') return localModels().map(lm => lm.id);
                                    return p.models;
                                  };
                                  return (
                                    <div style="display:flex;gap:0.5rem;align-items:flex-end;">
                                      <label style="flex:1;">
                                        <span style="font-size:12px;">Provider</span>
                                        <select value={addProvider()} onChange={(e) => { setAddProvider(e.currentTarget.value); setAddModel(''); }}>
                                          <option value="">— select —</option>
                                          <For each={editConfig()!.models.providers.filter(p => p.enabled)}>
                                            {(p) => <option value={p.id}>{p.name || p.id}</option>}
                                          </For>
                                        </select>
                                      </label>
                                      <label style="flex:1;">
                                        <span style="font-size:12px;">Model</span>
                                        <select value={addModel()} onChange={(e) => setAddModel(e.currentTarget.value)} disabled={!addProvider()}>
                                          <option value="">— select —</option>
                                          <For each={availableForProvider()}>
                                            {(m) => <option value={m}>{m}</option>}
                                          </For>
                                        </select>
                                      </label>
                                      <button disabled={!addProvider() || !addModel()} onClick={() => {
                                        setScannerModels([...models(), { provider: addProvider(), model: addModel() }]);
                                        setAddProvider('');
                                        setAddModel('');
                                      }}>+ Add</button>
                                    </div>
                                  );
                                })()}
                              </div>
                            );
                          })()}
                          </Show>

                          <h3 style="margin-top: 1.5rem;">Scan Sources</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">
                            Choose which data sources are scanned for prompt injection.
                          </p>
                          {(() => {
                            const ss = () => editConfig()!.security.prompt_injection.scan_sources ?? {
                              workspace_files: true, clipboard: true, messaging_inbound: true, web_content: true, mcp_responses: true, tool_overrides: {}
                            };
                            const updateScanSources = (field: string, value: boolean) => {
                              setEditConfig((c) => c ? {
                                ...c,
                                security: {
                                  ...c.security,
                                  prompt_injection: {
                                    ...c.security.prompt_injection,
                                    scan_sources: { ...ss(), [field]: value },
                                  },
                                },
                              } : null);
                            };
                            return (
                              <div class="settings-form">
                                <Switch checked={ss().workspace_files} onChange={(checked) => updateScanSources('workspace_files', checked)} class="flex items-center gap-2">

                                  <SwitchControl><SwitchThumb /></SwitchControl>

                                  <SwitchLabel>Workspace files — scan file contents read by tools</SwitchLabel>

                                </Switch>
                                <Switch checked={ss().clipboard} onChange={(checked) => updateScanSources('clipboard', checked)} class="flex items-center gap-2">

                                  <SwitchControl><SwitchThumb /></SwitchControl>

                                  <SwitchLabel>Clipboard — scan pasted content</SwitchLabel>

                                </Switch>
                                <Switch checked={ss().messaging_inbound} onChange={(checked) => updateScanSources('messaging_inbound', checked)} class="flex items-center gap-2">

                                  <SwitchControl><SwitchThumb /></SwitchControl>

                                  <SwitchLabel>Chat messages — scan inbound messages</SwitchLabel>

                                </Switch>
                                <Switch checked={ss().web_content} onChange={(checked) => updateScanSources('web_content', checked)} class="flex items-center gap-2">

                                  <SwitchControl><SwitchThumb /></SwitchControl>

                                  <SwitchLabel>Web content — scan HTTP responses</SwitchLabel>

                                </Switch>
                                <Switch checked={ss().mcp_responses} onChange={(checked) => updateScanSources('mcp_responses', checked)} class="flex items-center gap-2">

                                  <SwitchControl><SwitchThumb /></SwitchControl>

                                  <SwitchLabel>MCP responses — scan MCP server results</SwitchLabel>

                                </Switch>
                              </div>
                            );
                          })()}

                          <h3 style="margin-top: 1.5rem;">Default Permissions</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">These rules are applied to new sessions as starting permissions.</p>
                          <table style="width:100%;border-collapse:collapse;font-size:13px;">
                            <thead>
                              <tr style="border-bottom:1px solid hsl(var(--border));">
                                <th style="text-align:left;padding:4px 6px;">Tool Pattern</th>
                                <th style="text-align:left;padding:4px 6px;">Scope</th>
                                <th style="text-align:left;padding:4px 6px;">Decision</th>
                                <th style="width:30px;"></th>
                              </tr>
                            </thead>
                            <tbody>
                              <For each={editConfig()!.security.default_permissions ?? []}>
                                {(rule, idx) => (
                                  <tr style="border-bottom:1px solid hsl(var(--border));">
                                    <td style="padding:4px 6px;">
                                      <input type="text" value={rule.tool_pattern} onInput={(e) => {
                                          const perms = [...(editConfig()!.security.default_permissions ?? [])];
                                          perms[idx()] = { ...perms[idx()], tool_pattern: e.currentTarget.value };
                                          setEditConfig((c) => c ? { ...c, security: { ...c.security, default_permissions: perms } } : null);
                                        }}
                                        style="width:100%;background:hsl(var(--card));color:hsl(var(--foreground));border:1px solid hsl(var(--border));border-radius:4px;padding:2px 6px;"
                                      />
                                    </td>
                                    <td style="padding:4px 6px;">
                                      <input type="text" value={rule.scope} onInput={(e) => {
                                          const perms = [...(editConfig()!.security.default_permissions ?? [])];
                                          perms[idx()] = { ...perms[idx()], scope: e.currentTarget.value };
                                          setEditConfig((c) => c ? { ...c, security: { ...c.security, default_permissions: perms } } : null);
                                        }}
                                        style="width:100%;background:hsl(var(--card));color:hsl(var(--foreground));border:1px solid hsl(var(--border));border-radius:4px;padding:2px 6px;"
                                      />
                                    </td>
                                    <td style="padding:4px 6px;">
                                      <select value={rule.decision} onChange={(e) => {
                                          const perms = [...(editConfig()!.security.default_permissions ?? [])];
                                          perms[idx()] = { ...perms[idx()], decision: e.currentTarget.value };
                                          setEditConfig((c) => c ? { ...c, security: { ...c.security, default_permissions: perms } } : null);
                                        }}>
                                        <option value="auto">Auto (allow)</option>
                                        <option value="ask">Ask</option>
                                        <option value="deny">Deny</option>
                                      </select>
                                    </td>
                                    <td style="padding:4px 6px;">
                                      <button onClick={() => {
                                        const perms = (editConfig()!.security.default_permissions ?? []).filter((_, i) => i !== idx());
                                        setEditConfig((c) => c ? { ...c, security: { ...c.security, default_permissions: perms } } : null);
                                      }}>✕</button>
                                    </td>
                                  </tr>
                                )}
                              </For>
                            </tbody>
                          </table>
                          <button style="margin-top:8px;" onClick={() => {
                            const perms = [...(editConfig()!.security.default_permissions ?? []), { tool_pattern: '*', scope: '*', decision: 'ask' }];
                            setEditConfig((c) => c ? { ...c, security: { ...c.security, default_permissions: perms } } : null);
                          }}>+ Add Default Rule</button>

                          <h3 style="margin-top: 1.5rem;">Sandbox</h3>
                          <p class="muted" style="font-size:12px;margin-bottom:8px;">
                            Restrict shell commands to the workspace and managed Python environment using OS-level sandboxing.
                          </p>
                          <div class="settings-form">
                            <Switch checked={(editConfig()!.security.sandbox ?? { enabled: true }).enabled} onChange={(checked) => {
                              const cur = editConfig()!.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                              setEditConfig((c) => c ? { ...c, security: { ...c.security, sandbox: { ...cur, enabled: checked } } } : null);
                            }} class="flex items-center gap-2">
                              <SwitchControl><SwitchThumb /></SwitchControl>
                              <SwitchLabel>Enable OS-level sandboxing</SwitchLabel>
                            </Switch>
                            <Switch checked={(editConfig()!.security.sandbox ?? { allow_network: true }).allow_network} onChange={(checked) => {
                              const cur = editConfig()!.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                              setEditConfig((c) => c ? { ...c, security: { ...c.security, sandbox: { ...cur, allow_network: checked } } } : null);
                            }} class="flex items-center gap-2">
                              <SwitchControl><SwitchThumb /></SwitchControl>
                              <SwitchLabel>Allow network access</SwitchLabel>
                            </Switch>

                            <div style="margin-top: 8px;">
                              <span class="muted" style="font-size:12px;">Additional read-only paths</span>
                              <For each={editConfig()!.security.sandbox?.extra_read_paths ?? []}>{(p, i) => (
                                <div style="display:flex;gap:4px;margin-top:4px;align-items:center;">
                                  <input type="text" value={p} style="flex:1"
                                    onChange={(e) => {
                                      setEditConfig((c) => {
                                        if (!c) return null;
                                        const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                        const paths = [...(cur.extra_read_paths ?? [])];
                                        paths[i()] = e.currentTarget.value;
                                        return { ...c, security: { ...c.security, sandbox: { ...cur, extra_read_paths: paths } } };
                                      });
                                    }} />
                                  <button style="padding:2px 6px;font-size:12px;" onClick={() => {
                                    setEditConfig((c) => {
                                      if (!c) return null;
                                      const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                      const paths = [...(cur.extra_read_paths ?? [])];
                                      paths.splice(i(), 1);
                                      return { ...c, security: { ...c.security, sandbox: { ...cur, extra_read_paths: paths } } };
                                    });
                                  }}>✕</button>
                                </div>
                              )}</For>
                              <button style="margin-top:4px;font-size:12px;" onClick={() => {
                                setEditConfig((c) => {
                                  if (!c) return null;
                                  const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                  return { ...c, security: { ...c.security, sandbox: { ...cur, extra_read_paths: [...(cur.extra_read_paths ?? []), ''] } } };
                                });
                              }}>+ Add path</button>
                            </div>

                            <div style="margin-top: 8px;">
                              <span class="muted" style="font-size:12px;">Additional read-write paths</span>
                              <For each={editConfig()!.security.sandbox?.extra_write_paths ?? []}>{(p, i) => (
                                <div style="display:flex;gap:4px;margin-top:4px;align-items:center;">
                                  <input type="text" value={p} style="flex:1"
                                    onChange={(e) => {
                                      setEditConfig((c) => {
                                        if (!c) return null;
                                        const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                        const paths = [...(cur.extra_write_paths ?? [])];
                                        paths[i()] = e.currentTarget.value;
                                        return { ...c, security: { ...c.security, sandbox: { ...cur, extra_write_paths: paths } } };
                                      });
                                    }} />
                                  <button style="padding:2px 6px;font-size:12px;" onClick={() => {
                                    setEditConfig((c) => {
                                      if (!c) return null;
                                      const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                      const paths = [...(cur.extra_write_paths ?? [])];
                                      paths.splice(i(), 1);
                                      return { ...c, security: { ...c.security, sandbox: { ...cur, extra_write_paths: paths } } };
                                    });
                                  }}>✕</button>
                                </div>
                              )}</For>
                              <button style="margin-top:4px;font-size:12px;" onClick={() => {
                                setEditConfig((c) => {
                                  if (!c) return null;
                                  const cur = c.security.sandbox ?? { enabled: true, extra_read_paths: [], extra_write_paths: [], allow_network: true };
                                  return { ...c, security: { ...c.security, sandbox: { ...cur, extra_write_paths: [...(cur.extra_write_paths ?? []), ''] } } };
                                });
                              }}>+ Add path</button>
                            </div>
                          </div>

                          <h3 style="margin-top: 1.5rem;">Data Classification Reference</h3>
                          <div class="settings-data-classes">
                            <article class="memory-card"><header><strong>PUBLIC</strong> <span class="badge complete">open</span></header><p class="muted">Safe for any channel. No restrictions.</p></article>
                            <article class="memory-card"><header><strong>INTERNAL</strong> <span class="badge">default</span></header><p class="muted">Internal use. May cross internal and private channels.</p></article>
                            <article class="memory-card"><header><strong>CONFIDENTIAL</strong> <span class="badge processing">restricted</span></header><p class="muted">Requires private or local-only channels.</p></article>
                            <article class="memory-card"><header><strong>RESTRICTED</strong> <span class="badge failed">sensitive</span></header><p class="muted">Highest sensitivity. Local-only unless user explicitly allows.</p></article>
                          </div>
                        </>
                    </Show>
                  </div>
                </Show>

                {/* ── Local Models ────────────────────────────────── */}
                <Show when={localTab() === 'local-models'}>
                  <div class="settings-section">
                    <Show when={editConfig()} fallback={configLoadingFallback('local model settings')}>
                        <>
                          <h3>Local Model Configuration</h3>
                          <div class="settings-form">
                            <Switch checked={editConfig()!.local_models.enabled} onChange={(checked) => updateLocalModels('enabled', checked)} class="flex items-center gap-2">

                              <SwitchControl><SwitchThumb /></SwitchControl>

                              <SwitchLabel>Local models enabled</SwitchLabel>

                            </Switch>
                            <label>
                              <span>Storage path</span>
                              <input type="text" placeholder="~/.hivemind/models" value={editConfig()!.local_models.storage_path ?? ''}
                                onChange={(e) => updateLocalModels('storage_path', e.currentTarget.value || null)} />
                            </label>
                            <label>
                              <span>Max loaded models</span>
                              <input type="number" min="1" max="32" value={editConfig()!.local_models.max_loaded_models}
                                onChange={(e) => updateLocalModels('max_loaded_models', parseInt(e.currentTarget.value) || 2)} />
                            </label>
                            <label>
                              <span>Max concurrent downloads</span>
                              <input type="number" min="1" max="16" value={editConfig()!.local_models.max_download_concurrent}
                                onChange={(e) => updateLocalModels('max_download_concurrent', parseInt(e.currentTarget.value) || 2)} />
                            </label>
                            <Switch checked={editConfig()!.local_models.auto_evict} onChange={(checked) => updateLocalModels('auto_evict', checked)} class="flex items-center gap-2">

                              <SwitchControl><SwitchThumb /></SwitchControl>

                              <SwitchLabel>Auto-evict LRU models when limit reached</SwitchLabel>

                            </Switch>
                          </div>
                        </>
                    </Show>

                    {/* HuggingFace Authentication */}
                        <div class="settings-section" style="margin-top: 1rem;">
                          <h4>HuggingFace Authentication</h4>
                          <p style="font-size: 0.85em; color: hsl(var(--muted-foreground)); margin-bottom: 8px;">
                            Some models (like Gemma) are gated and require a HuggingFace token to download.{' '}
                            <a href="#" onClick={(e) => { e.preventDefault(); openExternal('https://huggingface.co/settings/tokens'); }} style="color: hsl(var(--primary)); cursor: pointer;">Get a token →</a>
                          </p>
                          <div class="settings-form">
                            <label>
                              <span>HF Token</span>
                              <div style="display: flex; align-items: center; gap: 0.5rem;">
                                <input type="password" id="hf-token-input" placeholder="hf_..." value={hfToken()}
                                  onInput={(e) => saveHfToken(e.currentTarget.value)} style="flex: 1;" />
                                <Show when={hfTokenLoaded()}>
                                  <Show when={hfToken()} fallback={
                                    <span class="text-yellow-400" style="font-size: 0.8em; white-space: nowrap;"><TriangleAlert size={14} /> No token — gated models won't be accessible</span>
                                  }>
                                    <span class="text-emerald-400" style="font-size: 0.8em; white-space: nowrap;">✓ Token configured</span>
                                  </Show>
                                </Show>
                              </div>
                            </label>
                          </div>
                        </div>

                    {/* Sub-navigation tabs — only show when local models are enabled */}
                    <Show when={editConfig()?.local_models.enabled}>
                    <Tabs value={localModelView()} onChange={(v) => { setLocalModelView(v as LocalModelView); if (v === 'hardware') loadHardwareInfo(); }} class="mt-4">
                      <TabsList class="relative w-full justify-start rounded-lg border border-border bg-muted/50 p-1">
                        <TabsTrigger value="library" class="rounded-md text-xs data-[selected]:shadow-sm">Installed</TabsTrigger>
                        <TabsTrigger value="search" class="rounded-md text-xs data-[selected]:shadow-sm">Browse Hub</TabsTrigger>
                        <TabsTrigger value="hardware" class="rounded-md text-xs data-[selected]:shadow-sm">Hardware</TabsTrigger>
                      </TabsList>
                    {/* Installed Models */}
                    <Show when={localModelView() === 'library'}>
                      <h3 style="margin-top: 1rem;">Installed Models</h3>
                      <Show when={localModels().length > 0} fallback={<p class="muted">No local models installed. Use the <strong>Browse Hub</strong> tab to search and install models.</p>}>
                        <For each={localModels()}>
                          {(model) => (
                            <Collapsible open={expandedModel() === model.id} onOpenChange={(open) => setExpandedModel(open ? model.id : null)}>
                            <article class="memory-card">
                              <header style="display: flex; justify-content: space-between; align-items: center;">
                                <div>
                                  <strong>{displayModelName(model)}</strong>
                                  <span class="badge">{model.runtime}</span>
                                </div>
                                <Show when={!isEmbeddingOnly(model)}>
                                  <button
                                    class="btn-icon"
                                    title="Inference Settings"
                                    style="background: none; border: none; cursor: pointer; font-size: 16px; padding: 2px 6px;"
                                    onClick={() => setExpandedModel(expandedModel() === model.id ? null : model.id)}
                                  >
                                    <SettingsIcon size={16} />
                                  </button>
                                </Show>
                              </header>
                              <dl class="details">
                                <div><dt>Repo</dt><dd>{model.hub_repo}</dd></div>
                                <Show when={model.runtime === 'onnx' && model.filename}>
                                  <div><dt>File</dt><dd>{model.filename}</dd></div>
                                </Show>
                                <div><dt>Size</dt><dd>{formatBytes(model.size_bytes)}</dd></div>
                              </dl>
                              <Show when={model.capabilities.tasks.length > 0}>
                                <p class="muted">Capabilities: {model.capabilities.tasks.join(', ')}{model.capabilities.can_call_tools ? ', tool-use' : ''}</p>
                              </Show>

                              <Show when={isEmbeddingOnly(model)}>
                                <p class="muted" style="font-size: 0.8em; margin-top: 6px; font-style: italic;">Embedding model — inference parameters are managed automatically.</p>
                              </Show>

                              <Show when={!isEmbeddingOnly(model)}>
                              <CollapsibleContent>
                                <div style="padding: 12px 0 4px 0; border-top: 1px solid hsl(var(--border)); margin-top: 8px;">
                                  <h4 style="margin: 0 0 8px 0; font-size: 13px;">Inference Parameters</h4>

                                  <label style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
                                    <span>Context Length</span>
                                    <div style="display: flex; align-items: center; gap: 8px;">
                                      <input
                                        type="range"
                                        min="512" max={model.capabilities.context_length ?? 32768} step="512"
                                        value={model.inference_params?.context_length ?? model.capabilities.context_length ?? 4096}
                                        onInput={(e) => {
                                          const val = parseInt(e.currentTarget.value);
                                          updateModelParamsDebounced(model.id, { ...model.inference_params, context_length: val } as InferenceParams);
                                        }}
                                      />
                                      <span style="min-width: 50px; text-align: right; font-size: 12px;">
                                        {model.inference_params?.context_length ?? model.capabilities.context_length ?? 4096}
                                      </span>
                                    </div>
                                  </label>

                                  <label style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
                                    <span>Max Tokens</span>
                                    <div style="display: flex; align-items: center; gap: 8px;">
                                      <input
                                        type="range"
                                        min="256" max="8192" step="256"
                                        value={model.inference_params?.max_tokens ?? 2048}
                                        onInput={(e) => {
                                          const val = parseInt(e.currentTarget.value);
                                          updateModelParamsDebounced(model.id, { ...model.inference_params, max_tokens: val } as InferenceParams);
                                        }}
                                      />
                                      <span style="min-width: 50px; text-align: right; font-size: 12px;">
                                        {model.inference_params?.max_tokens ?? 2048}
                                      </span>
                                    </div>
                                  </label>

                                  <label style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
                                    <span>Temperature</span>
                                    <div style="display: flex; align-items: center; gap: 8px;">
                                      <input
                                        type="range"
                                        min="0" max="200" step="5"
                                        value={Math.round((model.inference_params?.temperature ?? 0.7) * 100)}
                                        onInput={(e) => {
                                          const val = parseInt(e.currentTarget.value) / 100;
                                          updateModelParamsDebounced(model.id, { ...model.inference_params, temperature: val } as InferenceParams);
                                        }}
                                      />
                                      <span style="min-width: 50px; text-align: right; font-size: 12px;">
                                        {(model.inference_params?.temperature ?? 0.7).toFixed(2)}
                                      </span>
                                    </div>
                                  </label>

                                  <label style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
                                    <span>Top P</span>
                                    <div style="display: flex; align-items: center; gap: 8px;">
                                      <input
                                        type="range"
                                        min="0" max="100" step="5"
                                        value={Math.round((model.inference_params?.top_p ?? 0.9) * 100)}
                                        onInput={(e) => {
                                          const val = parseInt(e.currentTarget.value) / 100;
                                          updateModelParamsDebounced(model.id, { ...model.inference_params, top_p: val } as InferenceParams);
                                        }}
                                      />
                                      <span style="min-width: 50px; text-align: right; font-size: 12px;">
                                        {(model.inference_params?.top_p ?? 0.9).toFixed(2)}
                                      </span>
                                    </div>
                                  </label>

                                  <label style="display: flex; justify-content: space-between; align-items: center;">
                                    <span>Repeat Penalty</span>
                                    <div style="display: flex; align-items: center; gap: 8px;">
                                      <input
                                        type="range"
                                        min="100" max="200" step="5"
                                        value={Math.round((model.inference_params?.repeat_penalty ?? 1.1) * 100)}
                                        onInput={(e) => {
                                          const val = parseInt(e.currentTarget.value) / 100;
                                          updateModelParamsDebounced(model.id, { ...model.inference_params, repeat_penalty: val } as InferenceParams);
                                        }}
                                      />
                                      <span style="min-width: 50px; text-align: right; font-size: 12px;">
                                        {(model.inference_params?.repeat_penalty ?? 1.1).toFixed(2)}
                                      </span>
                                    </div>
                                  </label>
                                </div>
                              </CollapsibleContent>
                              </Show>

                              <button class="btn-danger" style="margin-top: 0.5rem;" onClick={() => removeModel(model.id)}>Remove</button>
                            </article>
                            </Collapsible>
                          )}
                        </For>
                      </Show>
                      <h4 style="margin-top: 1rem;">Storage</h4>
                      <p class="muted">Total model storage: {formatBytes(storageBytes())}</p>
                    </Show>

                    {/* Browse Hub */}
                    <Show when={localModelView() === 'search'}>
                      <h3 style="margin-top: 1rem;">Search HuggingFace Hub</h3>
                      <div style="display: flex; gap: 0.5rem; margin-bottom: 1rem;">
                        <input
                          type="text"
                          placeholder="Search models (e.g. llama, mistral, phi)..."
                          value={hubSearchQuery()}
                          onInput={(e) => setHubSearchQuery(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === 'Enter') searchHubModels(); }}
                          style="flex: 1;"
                        />
                        <button onClick={searchHubModels} disabled={hubSearchLoading()}>
                          {hubSearchLoading() ? 'Searching...' : 'Search'}
                        </button>
                      </div>

                      <Show when={hubSearchError()}>
                        <Show when={isNoTokenError(hubSearchError()!)} fallback={
                          <Show when={isLicenseError(hubSearchError()!)} fallback={
                            <p class="muted text-destructive">Error: {hubSearchError()}</p>
                          }>
                            <div style="background: hsl(var(--muted)); border: 1px solid hsl(var(--primary)); border-radius: 6px; padding: 0.75rem 1rem; margin-bottom: 0.75rem;">
                              <p class="text-primary" style="margin: 0 0 0.5rem 0; font-weight: 600;"><ClipboardList size={14} /> License agreement required</p>
                              <p style="margin: 0 0 0.5rem 0; font-size: 0.9em; color: hsl(var(--muted-foreground));">
                                This model is gated. You must agree to its license terms on HuggingFace before downloading.
                              </p>
                              <div style="display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap;">
                                <Show when={extractRepoFromError(hubSearchError()!)}>
                                  {(repo) => (
                                    <button onClick={() => openExternal(`https://huggingface.co/${repo()}`)} style="font-size: 0.85em;">
                                      Agree to License →
                                    </button>
                                  )}
                                </Show>
                              </div>
                              <p style="margin: 0.5rem 0 0 0; font-size: 0.85em; color: hsl(var(--muted-foreground));">
                                After agreeing, click Install again.
                              </p>
                            </div>
                          </Show>
                        }>
                          <div style="background: hsl(var(--muted)); border: 1px solid hsl(var(--primary)); border-radius: 6px; padding: 0.75rem 1rem; margin-bottom: 0.75rem;">
                            <p class="text-yellow-400" style="margin: 0 0 0.5rem 0; font-weight: 600;"><Key size={14} /> HuggingFace token required</p>
                            <p style="margin: 0 0 0.5rem 0; font-size: 0.9em; color: hsl(var(--muted-foreground));">
                              You need a HuggingFace access token to download this model.
                            </p>
                            <div style="display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap;">
                              <button onClick={() => openExternal('https://huggingface.co/settings/tokens')} style="font-size: 0.85em;">
                                Create Token →
                              </button>
                              <button onClick={scrollToHfToken} style="font-size: 0.85em;">
                                ↑ Add Token Above
                              </button>
                            </div>
                          </div>
                        </Show>
                      </Show>
                      <Show when={hubSearchResults().length > 0}>
                        <For each={hubSearchResults()}>
                          {(model) => {
                            const meta = parseModelMeta(model);
                            return (
                              <article class="memory-card">
                                <header>
                                  <strong>{model.id}</strong>
                                </header>
                                <div class="model-badges" style="margin-bottom: 0.4rem;">
                                  {meta.runtime && <span class="badge format">{meta.runtime}</span>}
                                  {meta.format && <span class="badge">{meta.format}</span>}
                                  {meta.paramCount && <span class="badge quant">{meta.paramCount}</span>}
                                  {meta.quantization && <span class="badge quant">{meta.quantization}</span>}
                                  {meta.isInstruct && <span class="badge complete">instruct</span>}
                                  {meta.isQuantized && !meta.quantization && <span class="badge">quantized</span>}
                                </div>
                                <div class="model-badges">
                                  {meta.hasToolUse && <span class="badge complete"><Wrench size={14} /> Tools</span>}
                                  {meta.hasReasoning && <span class="badge processing"><Brain size={14} /> Reasoning</span>}
                                  {meta.hasVision && <span class="badge vision"><Eye size={14} /> Vision</span>}
                                  {meta.isConversational && <span class="badge"><MessageSquare size={14} /> Chat</span>}
                                </div>
                                <dl class="details" style="margin-top: 0.5rem;">
                                  <div><dt>Downloads</dt><dd>{model.downloads?.toLocaleString() ?? '—'}</dd></div>
                                  <div><dt>Likes</dt><dd><Heart size={14} /> {model.likes?.toLocaleString() ?? '0'}</dd></div>
                                  <Show when={model.pipeline_tag}><div><dt>Task</dt><dd>{model.pipeline_tag}</dd></div></Show>
                                  <Show when={model.author}><div><dt>Author</dt><dd>{model.author}</dd></div></Show>
                                </dl>
                                <div class="model-meta">
                                  {meta.arch && <span><Building2 size={14} /> {meta.arch}</span>}
                                  {meta.license && <span><ScrollText size={14} /> {meta.license}</span>}
                                  {meta.languages.length > 0 && <span><Globe size={14} /> {meta.languages.join(', ')}</span>}
                                  {model.library_name && <span><BookOpen size={14} /> {model.library_name}</span>}
                                  {meta.baseModel && <span><Link size={14} /> {meta.baseModel}</span>}
                                </div>
                                <button style="margin-top: 0.5rem;" onClick={() => openInstallDialog(model)}>Install</button>
                              </article>
                            );
                          }}
                        </For>
                      </Show>
                      <Show when={!hubSearchLoading() && !hubSearchError() && hubSearchResults().length === 0 && hubSearchQuery().trim()}>
                        <p class="muted">No results found.</p>
                      </Show>

                      {/* Install dialog */}
                      <Dialog
                        open={!!installTargetRepo()}
                        onOpenChange={(open) => { if (!open) setInstallTargetRepo(null); }}
                      >
                      <DialogContent>
                        <Show when={installTargetRepo()}>
                          {(repo) => (
                              <>
                              <h3>Install from {repo().id}</h3>
                              <Show when={installFilesLoading()}>
                                <p class="muted">Loading available files...</p>
                              </Show>
                              <Show when={!installFilesLoading() && installableItems().length === 0}>
                                <p class="muted">No compatible model files found in this repository.</p>
                              </Show>
                              <Show when={!installFilesLoading() && installableItems().length > 0}>
                                <p class="muted" style="margin-bottom: 0.5rem;">Select a variant to download:</p>
                                <div style="max-height: 400px; overflow-y: auto;">
                                <For each={installableItems()}>
                                  {(item) => (
                                    <article class="memory-card" style="cursor: pointer;">
                                      <header>
                                        <strong style="word-break: break-all;">{item.label}</strong>
                                        <div style="display: flex; gap: 4px; flex-wrap: wrap; margin-left: auto;">
                                          <span class="badge format">{item.runtime}</span>
                                          {item.quantization && <span class="badge quant" style="font-weight: 600;">{item.quantization}</span>}
                                          {item.fileCount > 1 && <span class="badge">{item.fileCount} files</span>}
                                        </div>
                                      </header>
                                      <Show when={item.totalSize != null}>
                                        <p class="muted">{formatBytes(item.totalSize!)}</p>
                                      </Show>
                                      <Button
                                        size="sm"
                                        style="margin-top: 0.5rem;"
                                        disabled={installInProgress()}
                                        onClick={() => installModelFile(repo().id, item.installFilename)}
                                      >
                                        {installInProgress() ? 'Installing...' : 'Download & Install'}
                                      </Button>
                                    </article>
                                  )}
                                </For>
                                </div>
                              </Show>
                              <DialogFooter class="mt-4">
                                <Button variant="outline" onClick={() => setInstallTargetRepo(null)}>Close</Button>
                              </DialogFooter>
                              </>
                          )}
                        </Show>
                      </DialogContent></Dialog>
                    </Show>

                    {/* Hardware */}
                    <Show when={localModelView() === 'hardware'}>
                      <h3 style="margin-top: 1rem;">Hardware</h3>
                      <Show when={hardwareInfo()} fallback={<p class="muted">Hardware info unavailable.</p>}>
                        {(hw) => (
                          <dl class="details">
                            <div><dt>CPU</dt><dd>{hw().cpu.name} ({hw().cpu.cores_logical} cores)</dd></div>
                            <div><dt>Total RAM</dt><dd>{formatBytes(hw().memory.total_bytes)}</dd></div>
                            <div><dt>Available RAM</dt><dd>{formatBytes(hw().memory.available_bytes)}</dd></div>
                          </dl>
                        )}
                      </Show>
                      <Show when={hardwareInfo()?.gpus?.length}>
                        <h4>GPUs</h4>
                        <For each={hardwareInfo()!.gpus}>
                          {(gpu) => (
                            <article class="memory-card">
                              <header><strong>{gpu.name}</strong></header>
                              <p class="muted">VRAM: {gpu.vram_bytes ? formatBytes(gpu.vram_bytes) : 'N/A'} · Driver: {gpu.driver_version ?? 'N/A'}</p>
                            </article>
                          )}
                        </For>
                      </Show>

                      <Show when={resourceUsage()}>
                        {(usage) => (
                          <>
                            <h4 style="margin-top: 1rem;">Runtime Usage</h4>
                            <dl class="details">
                              <div><dt>Models loaded</dt><dd>{usage().loaded_models}</dd></div>
                              <div><dt>Memory used</dt><dd>{formatBytes(usage().total_memory_used_bytes)}</dd></div>
                            </dl>
                          </>
                        )}
                      </Show>
                    </Show>
                    </Tabs>
                    </Show>
                  </div>
                </Show>

                {/* ── Downloads ─────────────────────────────────── */}
                <Show when={localTab() === 'downloads'}>
                  <div class="settings-section">
                    <h3>Downloads</h3>
                    <p class="muted">Active and recent model downloads from HuggingFace Hub.</p>
                    <Show when={activeDownloads().length === 0}>
                      <p class="muted" style="margin-top: 1rem;">No active downloads. Go to <strong>Local Models → Browse Hub</strong> to search and install models.</p>
                    </Show>
                    <Show when={activeDownloads().length > 0}>
                      <For each={activeDownloads()}>
                        {(dl) => {
                          const isFailed = () => dl.status === 'error' || dl.status === 'failed';
                          return (
                            <article
                              class="memory-card"
                              style={`margin-top: 0.75rem;${isFailed() ? ' border-left: 3px solid hsl(var(--destructive)); background: hsl(var(--destructive) / 0.06);' : ''}`}
                            >
                              <header>
                                <strong>{dl.filename}</strong>
                                <div style="display: flex; gap: 4px; align-items: center;">
                                  {(() => { const q = extractFileQuantization(dl.filename); return q ? <span class="badge quant" style="font-weight: 600;">{q}</span> : null; })()}
                                  <span class={`badge ${isFailed() ? 'failed' : dl.status === 'complete' ? 'complete' : 'processing'}`}>
                                    {isFailed() ? <><XCircle size={14} /> Failed</> : dl.status}
                                  </span>
                                </div>
                              </header>
                              <p class="muted">{dl.repo_id}</p>
                              <Show when={!isFailed()}>
                                <div style="margin-top: 0.5rem;">
                                  <div class="progress-bar-bg" style="height: 8px;">
                                    <div
                                      class="progress-bar-fill"
                                      style={`width: ${dl.total_bytes ? Math.round((dl.downloaded_bytes / dl.total_bytes) * 100) : 0}%`}
                                    />
                                  </div>
                                  <p class="muted" style="margin-top: 0.3rem; font-size: 0.82rem;">
                                    {formatBytes(dl.downloaded_bytes)}
                                    {dl.total_bytes ? ` / ${formatBytes(dl.total_bytes)} (${Math.round((dl.downloaded_bytes / dl.total_bytes) * 100)}%)` : ''}
                                  </p>
                                </div>
                              </Show>
                              <Show when={isFailed() && dl.error}>
                                <p
                                  class="text-destructive"
                                  style="margin-top: 0.4rem; font-size: 0.85rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%;"
                                  title={dl.error ?? ''}
                                >
                                  {(dl.error && dl.error.length > 120) ? dl.error.slice(0, 120) + '…' : dl.error}
                                </p>
                              </Show>
                              <Show when={isFailed()}>
                                <div style="margin-top: 0.5rem; display: flex; gap: 0.5rem;">
                                  <button
                                    class="btn-sm"
                                    onClick={async () => {
                                      try {
                                        await invoke('local_models_remove_download', { model_id: dl.model_id });
                                        const runtime = inferRuntime(dl.filename);
                                        await invoke('local_models_install', {
                                          hub_repo: dl.repo_id,
                                          filename: dl.filename,
                                          runtime,
                                        });
                                        startDownloadPolling();
                                      } catch (e) {
                                        console.error('Retry failed:', e);
                                      }
                                    }}
                                  >
                                    ↻ Retry
                                  </button>
                                  <button
                                    class="btn-sm"
                                    style="opacity: 0.7;"
                                    onClick={async () => {
                                      try {
                                        await invoke('local_models_remove_download', { model_id: dl.model_id });
                                        const downloads = await invoke<DownloadProgress[]>('local_models_downloads');
                                        setActiveDownloads(downloads);
                                      } catch (e) {
                                        console.error('Dismiss failed:', e);
                                      }
                                    }}
                                  >
                                    ✕ Dismiss
                                  </button>
                                </div>
                              </Show>
                            </article>
                          );
                        }}
                      </For>
                    </Show>
                  </div>
                </Show>

                {/* ── Tools──────────────────────────────────────── */}
                <Show when={localTab() === 'tools'}>
                  <div class="settings-section">
                    <div class="settings-section-header">
                      <h3>Registered Tools</h3>
                      <span class="pill neutral">{toolDefinitions().length} tools</span>
                    </div>
                    <p class="muted">These tools are available to the AI agent. Approval controls whether the agent can use a tool automatically or needs your permission first.</p>
                    <Show when={toolDefinitions().length > 0} fallback={<p class="muted">{daemonOnline() ? 'No tools available.' : 'Start the daemon to view tools.'}</p>}>
                      <div style="display:flex;flex-direction:column;gap:8px;">
                        <For each={toolDefinitions()}>
                          {(tool) => (
                            <article class="memory-card" style="padding:12px;">
                              <header style="display:flex;align-items:center;justify-content:space-between;">
                                <strong><code>{tool.id}</code></strong>
                                <div style="display:flex;gap:6px;">
                                  <span class={`badge ${tool.approval === 'auto' ? 'complete' : tool.approval === 'ask' ? 'processing' : 'failed'}`}>{tool.approval}</span>
                                  <span class="badge">{tool.channel_class}</span>
                                </div>
                              </header>
                              <p class="text-muted-foreground" style="margin:4px 0 0;font-size:0.85em;">{tool.description}</p>
                              <Show when={tool.side_effects}>
                                <span class="pill" style="margin-top:4px;font-size:0.75em;"><Zap size={14} /> side effects</span>
                              </Show>
                            </article>
                          )}
                        </For>
                      </div>
                    </Show>
                  </div>
                </Show>

                {/* ── Web Search ─────────────────────────────────── */}
                <Show when={localTab() === 'web-search'}>
                  <div class="settings-section">
                    <Show when={editConfig()} fallback={configLoadingFallback('web search settings')}>
                      <>
                        <h3>Web Search Provider</h3>
                        <p class="muted">Configure a search provider to enable the <code>web.search</code> tool for agents. This allows agents to search the web for libraries, documentation, and best practices.</p>

                        <div class="settings-field">
                          <label>Provider</label>
                          <select
                            value={editConfig()?.web_search?.provider ?? 'none'}
                            onChange={(e) => {
                              const prov = e.currentTarget.value;
                              setEditConfig((c) => c ? { ...c, web_search: { provider: prov, api_key: prov === 'none' ? undefined : c.web_search?.api_key } } : null);
                            }}
                          >
                            <option value="none">None (disabled)</option>
                            <option value="brave">Brave Search</option>
                            <option value="tavily">Tavily</option>
                          </select>
                        </div>

                        <Show when={(editConfig()?.web_search?.provider ?? 'none') !== 'none'}>
                          <div class="settings-field">
                            <label>API Key</label>
                            <input
                              type="password"
                              placeholder="Enter API key or env:VAR_NAME"
                              value={webSearchApiKey()}
                              onInput={(e) => {
                                const val = e.currentTarget.value;
                                setWebSearchApiKey(val);
                                // Store env: references in config directly; literal keys go to keyring on save
                                const isEnvRef = val.startsWith('env:');
                                setEditConfig((c) => c ? { ...c, web_search: { ...c.web_search, provider: c.web_search?.provider ?? 'none', api_key: isEnvRef ? val : (val ? 'keyring:web-search:api-key' : null) } } : null);
                              }}
                            />
                          </div>
                          <p class="settings-hint">
                            {editConfig()?.web_search?.provider === 'brave'
                              ? <>Get an API key at <a href="https://brave.com/search/api/" target="_blank" rel="noopener noreferrer">brave.com/search/api</a> ($5/month free credits, ~1,000 queries/month)</>
                              : <>Get an API key at <a href="https://app.tavily.com" target="_blank" rel="noopener noreferrer">app.tavily.com</a></>
                            }
                          </p>
                          <p class="settings-hint">
                            Tip: Use <code>env:BRAVE_API_KEY</code> or <code>env:TAVILY_API_KEY</code> to read from environment variables.
                          </p>
                        </Show>
                      </>
                    </Show>
                  </div>
                </Show>

                {/* ── Personas ──────────────────────────────────── */}
                <Show when={localTab() === 'personas'}>
                  <Show when={props.context()?.daemon_url} keyed fallback={<p class="muted">Start the daemon to manage personas.</p>}>
                    {(daemonUrl) => (
                      <PersonasTab
                        availableModels={props.availableModels}
                        availableTools={props.toolDefinitions}
                        daemon_url={daemonUrl}
                        onPersonasSaved={loadPersonas}
                        onExportToKit={props.onExportPersonaToKit}
                      />
                    )}
                  </Show>
                </Show>

                {/* ── Connectors ──────────────────────────────────── */}
                <Show when={localTab() === 'channels'}>
                  <Show when={props.context()?.daemon_url} fallback={<p class="muted">Start the daemon to manage connectors.</p>}>
                    {(url) => <Suspense fallback={<p class="muted">Loading…</p>}><ConnectorsTab daemon_url={url()} /></Suspense>}
                  </Show>
                </Show>

                {/* ── Audit Log ────────────────────────────────── */}
                <Show when={localTab() === 'comm-audit'}>
                  <Show when={props.context()?.daemon_url} fallback={<p class="muted">Start the daemon to view audit log.</p>}>
                    {(url) => <Suspense fallback={<p class="muted">Loading…</p>}><AuditViewer daemon_url={url()} /></Suspense>}
                  </Show>
                </Show>


                {/* ── Compaction ─────────────────────────────────── */}
                <Show when={localTab() === 'compaction'}>
                  <CompactionTab
                    editConfig={editConfig}
                    updateCompaction={updateCompaction}
                    configLoadingFallback={configLoadingFallback}
                    availableModels={props.availableModels}
                  />
                </Show>

                {/* ── AFK / Status ─────────────────────────────── */}
                <Show when={localTab() === 'afk'}>
                  <GeneralTab
                    editConfig={editConfig}
                    updateAfk={updateAfk}
                    context={context}
                    daemonOnline={daemonOnline}
                    active={() => localTab() === 'afk'}
                    configLoadingFallback={configLoadingFallback}
                  />
                </Show>

                {/* ── Python Environment ──────────────────────────── */}
                <Show when={localTab() === 'python'}>
                  <RuntimeTab
                    editConfig={editConfig}
                    setEditConfig={setEditConfig}
                    configLoadingFallback={configLoadingFallback}
                    tab="python"
                  />
                </Show>

                {/* ── Node.js Environment ──────────────────────────── */}
                <Show when={localTab() === 'node'}>
                  <RuntimeTab
                    editConfig={editConfig}
                    setEditConfig={setEditConfig}
                    configLoadingFallback={configLoadingFallback}
                    tab="node"
                  />
                </Show>
              </div>
              </div>
          </div>
  );
};

export default SettingsModal;
