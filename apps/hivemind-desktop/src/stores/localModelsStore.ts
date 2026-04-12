import { createSignal, type Accessor, type Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type {
  DownloadProgress,
  HardwareInfo,
  HardwareSummary,
  HubFileInfo,
  HubModelInfo,
  HubRepoFilesResult,
  HubSearchResult,
  InferenceParams,
  InstalledModel,
  InstallableItem,
  LocalModelSummary,
  RuntimeResourceUsage,
} from '../types';
import { isNoTokenError, isLicenseError } from '../utils';
import { groupInstallableFiles } from '../types';

export interface LocalModelsStoreDeps {
  setErrorMessage: Setter<string | null>;
  onInstallStarted: () => void;
}

export function createLocalModelsStore(deps: LocalModelsStoreDeps) {
  const { setErrorMessage, onInstallStarted } = deps;

  const [localModels, setLocalModels] = createSignal<InstalledModel[]>([]);
  const [hubSearchResults, setHubSearchResults] = createSignal<HubModelInfo[]>([]);
  const [hubSearchQuery, setHubSearchQuery] = createSignal('');
  const [hubSearchLoading, setHubSearchLoading] = createSignal(false);
  const [hubSearchError, setHubSearchError] = createSignal<string | null>(null);
  const [hardwareInfo, setHardwareInfo] = createSignal<HardwareInfo | null>(null);
  const [resourceUsage, setResourceUsage] = createSignal<RuntimeResourceUsage | null>(null);
  const [storageBytes, setStorageBytes] = createSignal<number>(0);
  const [localModelView, setLocalModelView] = createSignal<'library' | 'search' | 'hardware'>('library');
  const [installTargetRepo, setInstallTargetRepo] = createSignal<HubModelInfo | null>(null);
  const [installRepoFiles, setInstallRepoFiles] = createSignal<HubFileInfo[]>([]);
  const [installableItems, setInstallableItems] = createSignal<InstallableItem[]>([]);
  const [installFilesLoading, setInstallFilesLoading] = createSignal(false);
  const [installInProgress, setInstallInProgress] = createSignal(false);
  const [activeDownloads, setActiveDownloads] = createSignal<DownloadProgress[]>([]);
  const [expandedModel, setExpandedModel] = createSignal<string | null>(null);

  let paramsTimeouts = new Map<string, ReturnType<typeof setTimeout>>();
  let downloadPollInterval: ReturnType<typeof setInterval> | null = null;

  const loadLocalModels = async () => {
    try {
      const summary = await invoke<LocalModelSummary>('local_models_list');
      setLocalModels(summary.models);
      setStorageBytes(summary.total_size_bytes);
    } catch (e) {
      console.error('Failed to load local models:', e);
    }
  };

  const updateModelParams = async (modelId: string, params: InferenceParams) => {
    try {
      await invoke('local_models_update_params', { model_id: modelId, params });
      await loadLocalModels();
    } catch (e) {
      console.error('Failed to update model params:', e);
    }
  };

  const updateModelParamsDebounced = (modelId: string, params: InferenceParams) => {
    const existing = paramsTimeouts.get(modelId);
    if (existing) clearTimeout(existing);
    paramsTimeouts.set(modelId, setTimeout(() => {
      paramsTimeouts.delete(modelId);
      updateModelParams(modelId, params);
    }, 300));
  };

  let downloadPollBusy = false;
  const startDownloadPolling = () => {
    if (downloadPollInterval) return;
    downloadPollInterval = setInterval(() => {
      if (downloadPollBusy) return;
      downloadPollBusy = true;
      void (async () => {
        try {
          const downloads = await invoke<DownloadProgress[]>('local_models_downloads');
          setActiveDownloads(downloads);
          if (downloads.some(d => d.status === 'complete')) {
            await loadLocalModels();
          }
          if (downloads.length === 0 && downloadPollInterval) {
            clearInterval(downloadPollInterval);
            downloadPollInterval = null;
          }
        } catch (e) {
          console.error('Failed to poll downloads:', e);
        } finally {
          downloadPollBusy = false;
        }
      })();
    }, 3_000);
  };

  const stopDownloadPolling = () => {
    if (downloadPollInterval) {
      clearInterval(downloadPollInterval);
      downloadPollInterval = null;
    }
  };

  const cleanup = () => {
    stopDownloadPolling();
    paramsTimeouts.forEach(clearTimeout);
    paramsTimeouts.clear();
  };

  const loadHardwareInfo = async () => {
    const [hw, ru, st] = await Promise.allSettled([
      invoke<HardwareSummary>('local_models_hardware'),
      invoke<RuntimeResourceUsage>('local_models_resource_usage'),
      invoke<number>('local_models_storage'),
    ]);
    if (ru.status === 'fulfilled') {
      setResourceUsage(ru.value);
    } else if (hw.status === 'fulfilled') {
      const u = hw.value.usage;
      setResourceUsage({
        loaded_models: u.models_loaded,
        total_memory_used_bytes: u.ram_used_bytes + u.vram_used_bytes,
        per_model: [],
      });
    }
    if (hw.status === 'fulfilled') {
      setHardwareInfo(hw.value.hardware);
    }
    if (st.status === 'fulfilled') setStorageBytes(st.value);
  };

  const searchHubModels = async () => {
    const query = hubSearchQuery().trim();
    if (!query) {
      setHubSearchResults([]);
      return;
    }
    setHubSearchLoading(true);
    setHubSearchError(null);
    try {
      const result = await invoke<HubSearchResult>('local_models_search', {
        query,
        task: 'text-generation',
        limit: 20,
      });
      setHubSearchResults(result.models ?? []);
    } catch (e) {
      console.error('Hub search failed:', e);
      setHubSearchError(`${e}`);
      setHubSearchResults([]);
    } finally {
      setHubSearchLoading(false);
    }
  };

  const inferRuntime = (filename: string): string => {
    const lower = filename.toLowerCase();
    if (lower.endsWith('.gguf') || lower.endsWith('.ggml')) return 'llama-cpp';
    if (lower.endsWith('.onnx')) return 'onnx';
    if (lower.endsWith('.safetensors') || lower.endsWith('.bin')) return 'candle';
    return 'llama-cpp';
  };

  const openInstallDialog = async (model: HubModelInfo) => {
    setInstallTargetRepo(model);
    setInstallRepoFiles([]);
    setInstallableItems([]);
    setInstallFilesLoading(true);
    try {
      const result = await invoke<HubRepoFilesResult>('local_models_hub_files', {
        repo_id: model.id,
      });
      const modelFiles = result.files.filter((f: HubFileInfo) => {
        const lower = f.filename.toLowerCase();
        return lower.endsWith('.gguf') || lower.endsWith('.ggml');
      });
      setInstallRepoFiles(modelFiles);
      setInstallableItems(groupInstallableFiles(modelFiles));
    } catch (e) {
      const msg = `${e}`;
      if (isNoTokenError(msg)) {
        setErrorMessage('[HF_NO_TOKEN] You need a HuggingFace access token to download this model.');
      } else if (isLicenseError(msg)) {
        setErrorMessage(msg);
      } else {
        setErrorMessage(`Failed to list repo files: ${msg}`);
      }
    } finally {
      setInstallFilesLoading(false);
    }
  };

  const installModelFile = async (repo_id: string, filename: string) => {
    try {
      setInstallInProgress(true);
      const runtime = inferRuntime(filename);
      await invoke('local_models_install', {
        hub_repo: repo_id,
        filename,
        runtime,
      });
      setInstallTargetRepo(null);
      setInstallRepoFiles([]);
      setInstallableItems([]);
      onInstallStarted();
      startDownloadPolling();
    } catch (e) {
      const msg = `${e}`;
      if (isNoTokenError(msg)) {
        setErrorMessage('[HF_NO_TOKEN] You need a HuggingFace access token to download this model.');
      } else if (isLicenseError(msg)) {
        setErrorMessage(msg);
      } else {
        setErrorMessage(`Install failed: ${msg}`);
      }
    } finally {
      setInstallInProgress(false);
    }
  };

  const removeModel = async (modelId: string) => {
    try {
      await invoke('local_models_remove', { model_id: modelId });
      await loadLocalModels();
    } catch (e) {
      console.error('Failed to remove model:', e);
      throw e;
    }
  };

  return {
    localModels, localModelView, setLocalModelView,
    storageBytes, expandedModel, setExpandedModel,
    hubSearchResults, hubSearchQuery, setHubSearchQuery,
    hubSearchLoading, hubSearchError,
    hardwareInfo, resourceUsage,
    installTargetRepo, setInstallTargetRepo,
    installRepoFiles, installableItems, installFilesLoading, installInProgress,
    activeDownloads, setActiveDownloads,
    loadLocalModels, loadHardwareInfo,
    updateModelParamsDebounced, removeModel,
    searchHubModels, openInstallDialog, installModelFile,
    inferRuntime,
    startDownloadPolling, stopDownloadPolling, cleanup,
  };
}

export type LocalModelsStore = ReturnType<typeof createLocalModelsStore>;
