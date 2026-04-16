import { createSignal, createMemo, createEffect, type Accessor, type Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { HiveMindConfigData, ModelProviderConfig, PolicyAction } from '../types';

// ── Pure utility functions (no signal dependencies) ─────────────

export const isNoTokenError = (msg: string) => msg.includes('[HF_NO_TOKEN]');
export const isLicenseError = (msg: string) => msg.includes('[HF_LICENSE]');
export const isAuthError = (msg: string) => isNoTokenError(msg) || isLicenseError(msg);
export const extractRepoFromError = (msg: string): string | null => {
  const m = msg.match(/repo=([^\s]+)/);
  return m ? m[1] : null;
};
export const openExternal = async (url: string) => {
  try { await invoke('open_url', { url }); } catch { window.open(url, '_blank', 'noopener'); }
};
export const scrollToHfToken = () => {
  const el = document.getElementById('hf-token-input');
  if (el) { el.focus(); el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }
};

// ── Dependency & return interfaces ──────────────────────────────

export interface ConfigStoreDeps {
  activeScreen: Accessor<string>;
  loadPersonas: () => Promise<void>;
  setToolDefinitions: Setter<any[]>;
  setUserStatus: Setter<string>;
}

export interface ConfigStoreReturn {
  editConfig: Accessor<HiveMindConfigData | null>;
  setEditConfig: Setter<HiveMindConfigData | null>;
  savedConfig: Accessor<string>;
  setSavedConfig: Setter<string>;
  configSaveMsg: Accessor<string | null>;
  setConfigSaveMsg: Setter<string | null>;
  editingProviderIdx: Accessor<number | null>;
  setEditingProviderIdx: Setter<number | null>;
  pendingKeyringDeletes: Accessor<string[]>;
  setPendingKeyringDeletes: Setter<string[]>;
  configDirty: Accessor<boolean>;
  configLoadError: Accessor<string | null>;
  loadEditConfig: (retries?: number) => Promise<void>;
  loadToolDefinitions: () => Promise<void>;
  saveConfig: () => Promise<void>;
  updateDaemon: (field: string, value: unknown) => void;
  updateApi: (field: string, value: unknown) => void;
  updateOverridePolicy: (field: string, value: PolicyAction) => void;
  updatePromptInjection: (field: string, value: unknown) => void;
  updateLocalModels: (field: string, value: unknown) => void;
  updateCompaction: (field: string, value: unknown) => void;
  updateAfk: (field: string, value: unknown) => void;
  handleSetUserStatus: (status: string) => Promise<void>;
  updateProvider: (idx: number, field: string, value: unknown) => void;
  addProvider: () => void;
  removeProvider: (idx: number) => void;
  moveProvider: (fromIdx: number, toIdx: number) => void;
  addModelToProvider: (idx: number, model: string) => void;
  removeModelFromProvider: (pIdx: number, mIdx: number) => void;
}

// ── Factory ─────────────────────────────────────────────────────

export function createConfigStore(deps: ConfigStoreDeps): ConfigStoreReturn {
  const { activeScreen, loadPersonas, setToolDefinitions, setUserStatus } = deps;

  const [editConfig, setEditConfig] = createSignal<HiveMindConfigData | null>(null);
  const [savedConfig, setSavedConfig] = createSignal<string>('');
  const [configSaveMsg, setConfigSaveMsg] = createSignal<string | null>(null);
  const [editingProviderIdx, setEditingProviderIdx] = createSignal<number | null>(null);
  const [pendingKeyringDeletes, setPendingKeyringDeletes] = createSignal<string[]>([]);

  const configDirty = createMemo(() => {
    const ec = editConfig();
    return ec !== null && JSON.stringify(ec) !== savedConfig();
  });

  const [configLoadError, setConfigLoadError] = createSignal<string | null>(null);

  const MIGRATION_KEY = 'hivemind-provider-id-migration';

  function getOrCreateMigrationId(oldId: string): string {
    const stored = localStorage.getItem(MIGRATION_KEY);
    const map: Record<string, string> = stored ? JSON.parse(stored) : {};
    if (map[oldId]) return map[oldId];
    const newId = crypto.randomUUID();
    map[oldId] = newId;
    localStorage.setItem(MIGRATION_KEY, JSON.stringify(map));
    return newId;
  }

  const loadEditConfig = async (retries = 2) => {
    setConfigLoadError(null);
    try {
      const rawCfg: HiveMindConfigData = await invoke('config_get');
      const isUuid = (s: string) => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s);

      // Migrate providers: assign GUID IDs to old name-based providers
      const idMigrationMap: Record<string, string> = {};
      const migratedProviders = (rawCfg?.models?.providers ?? []).map((provider) => {
        if (isUuid(provider.id)) {
          return { ...provider, name: provider.name || provider.id };
        }
        // Old-style name-based ID — assign a deterministic GUID and keep old id as name
        const newId = getOrCreateMigrationId(provider.id);
        idMigrationMap[provider.id] = newId;
        return { ...provider, id: newId, name: provider.name || provider.id };
      });

      // Migrate keyring secrets for renamed providers (copy only — old keys deleted on save)
      const pendingDeletes: string[] = [];
      for (const [oldId, newId] of Object.entries(idMigrationMap)) {
        try {
          const oldKey = `provider:${oldId}:api-key`;
          const secret = await invoke<string | null>('load_secret', { key: oldKey });
          if (secret) {
            await invoke('save_secret', { key: `provider:${newId}:api-key`, value: secret });
            pendingDeletes.push(oldKey);
          }
        } catch { /* best-effort migration */ }
      }
      setPendingKeyringDeletes(pendingDeletes);

      const cfg: HiveMindConfigData = {
        ...rawCfg,
        models: { ...rawCfg.models, providers: migratedProviders },
      };
      setEditConfig(cfg);
      setSavedConfig(JSON.stringify(cfg));
      setConfigSaveMsg(null);
      setConfigLoadError(null);
    } catch (e) {
      if (retries > 0) {
        await new Promise(r => setTimeout(r, 1000));
        return loadEditConfig(retries - 1);
      }
      setConfigLoadError(String(e) || 'Failed to load configuration');
    }
  };

  const loadToolDefinitions = async () => {
    try {
      setToolDefinitions(await invoke('tools_list'));
    } catch (e) { console.error('Failed to load tools:', e); }
  };

  // Auto-load config when navigating to settings page
  createEffect(() => {
    if (activeScreen() === 'settings') {
      void loadEditConfig();
      void loadToolDefinitions();
    }
  });

  const saveConfig = async () => {
    const cfg = editConfig();
    if (!cfg) return;
    try {
      const result = await invoke<{ saved: boolean; message: string }>('config_save', { config: cfg });
      setSavedConfig(JSON.stringify(cfg));
      setConfigSaveMsg(result.message ?? 'Saved successfully.');
      await loadPersonas();
      // Now safe to delete old keyring keys from ID migration
      for (const oldKey of pendingKeyringDeletes()) {
        try { await invoke('delete_secret', { key: oldKey }); } catch { /* ignore */ }
      }
      setPendingKeyringDeletes([]);
    } catch (e) {
      setConfigSaveMsg(`Save failed: ${e}`);
    }
  };

  const updateDaemon = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, daemon: { ...c.daemon, [field]: value } } : null);
  const updateApi = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, api: { ...c.api, [field]: value } } : null);
  const updateOverridePolicy = (field: string, value: PolicyAction) =>
    setEditConfig((c) => c ? { ...c, security: { ...c.security, override_policy: { ...c.security.override_policy, [field]: value } } } : null);
  const updatePromptInjection = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, security: { ...c.security, prompt_injection: { ...c.security.prompt_injection, [field]: value } } } : null);
  const updateLocalModels = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, local_models: { ...c.local_models, [field]: value } } : null);
  const updateCompaction = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, compaction: { ...c.compaction, [field]: value } } : null);
  const updateAfk = (field: string, value: unknown) =>
    setEditConfig((c) => c ? { ...c, afk: { ...c.afk, [field]: value } } : null);

  const handleSetUserStatus = async (status: string) => {
    try {
      const result = await invoke<{ status: string }>('set_user_status', { status });
      setUserStatus(result.status);
    } catch (e) {
      console.error('Failed to set user status:', e);
    }
  };

  const updateProvider = (idx: number, field: string, value: unknown) =>
    setEditConfig((c) => {
      if (!c) return null;
      const providers = [...c.models.providers];
      providers[idx] = { ...providers[idx], [field]: value };
      return { ...c, models: { ...c.models, providers } };
    });
  const addProvider = () => {
    setEditConfig((c) => {
      if (!c) return null;
      const kind = 'open-ai-compatible';
      // Use a short random id so it doesn't imply a specific provider kind.
      // The user picks the actual kind in the edit dialog afterwards.
      const id = `provider-${crypto.randomUUID().slice(0, 8)}`;
      const newP: ModelProviderConfig = {
        id, name: 'New Provider', kind,
        base_url: 'https://api.openai.com/v1', auth: 'api-key', models: [],
        model_capabilities: {},
        channel_class: 'internal', priority: 50, enabled: true,
        options: { route: null, allow_model_discovery: false, default_api_version: null, response_prefix: null, headers: {} },
      };
      const updated = { ...c, models: { ...c.models, providers: [...c.models.providers, newP] } };
      // Open the edit dialog for the newly added provider
      setTimeout(() => setEditingProviderIdx(updated.models.providers.length - 1), 0);
      return updated;
    });
  };
  const removeProvider = (idx: number) =>
    setEditConfig((c) => c ? { ...c, models: { ...c.models, providers: c.models.providers.filter((_, i) => i !== idx) } } : null);
  const moveProvider = (fromIdx: number, toIdx: number) =>
    setEditConfig((c) => {
      if (!c) return null;
      const providers = [...c.models.providers];
      if (fromIdx < 0 || fromIdx >= providers.length || toIdx < 0 || toIdx >= providers.length) return c;
      const [moved] = providers.splice(fromIdx, 1);
      providers.splice(toIdx, 0, moved);
      // Reassign priority so array order is the source of truth (highest first)
      const updated = providers.map((p, i) => ({ ...p, priority: (providers.length - i) * 10 }));
      return { ...c, models: { ...c.models, providers: updated } };
    });
  const addModelToProvider = (idx: number, model: string) =>
    setEditConfig((c) => {
      if (!c || !model.trim()) return c;
      const providers = [...c.models.providers];
      providers[idx] = { ...providers[idx], models: [...providers[idx].models, model.trim()] };
      return { ...c, models: { ...c.models, providers } };
    });
  const removeModelFromProvider = (pIdx: number, mIdx: number) =>
    setEditConfig((c) => {
      if (!c) return null;
      const providers = [...c.models.providers];
      providers[pIdx] = { ...providers[pIdx], models: providers[pIdx].models.filter((_, i) => i !== mIdx) };
      return { ...c, models: { ...c.models, providers } };
    });

  return {
    editConfig, setEditConfig,
    savedConfig, setSavedConfig,
    configSaveMsg, setConfigSaveMsg,
    editingProviderIdx, setEditingProviderIdx,
    pendingKeyringDeletes, setPendingKeyringDeletes,
    configDirty,
    configLoadError,
    loadEditConfig,
    loadToolDefinitions,
    saveConfig,
    updateDaemon, updateApi, updateOverridePolicy, updatePromptInjection,
    updateLocalModels, updateCompaction, updateAfk,
    handleSetUserStatus,
    updateProvider, addProvider, removeProvider, moveProvider,
    addModelToProvider, removeModelFromProvider,
  };
}
