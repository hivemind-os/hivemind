import { Show, createSignal, onCleanup, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { CheckCircle, Download, AlertCircle, Loader2 } from 'lucide-solid';
import { Button } from '~/ui';
import type { DownloadProgress, InstalledModel, HiveMindConfigData } from '../../types';
import { formatBytes } from '../../utils';

export interface ModelsStepProps {
  startDownloadPolling: () => void;
  loadLocalModels: () => Promise<void>;
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
}

const BGE_REPO = 'BAAI/bge-small-en-v1.5';
const BGE_FILE = 'onnx/model.onnx';
const BGE_SIZE = 127_000_000;

const ModelsStep = (props: ModelsStepProps) => {
  const [status, setStatus] = createSignal<'idle' | 'downloading' | 'done' | 'error'>('idle');
  const [progress, setProgress] = createSignal({ downloaded: 0, total: BGE_SIZE });
  const [errorMsg, setErrorMsg] = createSignal('');

  let pollInterval: number | undefined;

  const pct = () => {
    const p = progress();
    return p.total > 0 ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : 0;
  };

  const clearPoll = () => {
    if (pollInterval) {
      window.clearInterval(pollInterval);
      pollInterval = undefined;
    }
  };

  /** After BGE is installed, set the embedding capability on the local-models provider. */
  const setBgeEmbeddingCapability = async () => {
    try {
      const result = await invoke<{ models: InstalledModel[] }>('local_models_list');
      const bgeModel = (result?.models ?? []).find(m => m.hub_repo?.includes('bge'));
      if (!bgeModel) return;

      const cfg: HiveMindConfigData = await invoke('config_get');
      const providerIdx = cfg.models.providers.findIndex(p => p.kind === 'local-models');
      if (providerIdx === -1) return;

      const provider = cfg.models.providers[providerIdx];
      const mc = { ...(provider.model_capabilities ?? {}), [bgeModel.id]: ['embedding'] as const };
      const updatedProviders = [...cfg.models.providers];
      updatedProviders[providerIdx] = { ...provider, model_capabilities: mc };

      const updated = { ...cfg, models: { ...cfg.models, providers: updatedProviders } };
      await invoke('config_save', { config: updated });
    } catch {
      // best-effort — user can configure manually later
    }
  };

  const markInstalled = async () => {
    clearPoll();
    setStatus('done');
    await props.loadLocalModels();
    await setBgeEmbeddingCapability();
  };

  const startDownload = async () => {
    setStatus('downloading');
    setErrorMsg('');

    try {
      await invoke('local_models_install', {
        hub_repo: BGE_REPO,
        filename: BGE_FILE,
        runtime: 'onnx',
      });
    } catch (e: any) {
      setStatus('error');
      setErrorMsg(typeof e === 'string' ? e : e?.message ?? 'Failed to start download');
      return;
    }

    props.startDownloadPolling();
    clearPoll();

    let busy = false;
    let missCount = 0;
    let finalizingCount = 0;
    pollInterval = window.setInterval(() => {
      if (busy) return;
      busy = true;
      void (async () => {
        try {
          const downloads = await invoke<DownloadProgress[]>('local_models_downloads');
          let found = false;
          for (const d of downloads) {
            if (d.filename?.includes('model.onnx') || d.repo_id?.includes('bge')) {
              found = true;
              missCount = 0;
              setProgress({ downloaded: d.downloaded_bytes, total: d.total_bytes || BGE_SIZE });
              if (d.status === 'complete') {
                await markInstalled();
                return;
              }
              if (d.status === 'finalizing') {
                // Backend is doing post-download work (companion files,
                // router rebuild).  Check registry directly — model may
                // already be usable.
                try {
                  const result = await invoke<{ models: InstalledModel[] }>('local_models_list');
                  const models = result?.models ?? [];
                  const installed = models.some(
                    (m) => m.hub_repo?.includes('bge') && m.status === 'available',
                  );
                  if (installed) {
                    await markInstalled();
                    return;
                  }
                } catch {
                  // ignore — keep polling
                }
              }
              if (d.status === 'error' || d.status === 'failed') {
                clearPoll();
                setStatus('error');
                setErrorMsg('Download failed — you can retry from Settings later');
                return;
              }
              // If download bytes reached 100% but status is still
              // "downloading" (backend doing companion files / rebuild),
              // check the registry directly as a fallback so we don't
              // get stuck on "Finalizing installation…" forever.
              const currentPct = d.total_bytes && d.total_bytes > 0
                ? d.downloaded_bytes / d.total_bytes
                : 0;
              if (currentPct >= 1) {
                finalizingCount++;
                if (finalizingCount >= 3) {
                  try {
                    const result = await invoke<{ models: InstalledModel[] }>('local_models_list');
                    const models = result?.models ?? [];
                    const installed = models.some(
                      (m) => m.hub_repo?.includes('bge') && m.status === 'available',
                    );
                    if (installed) {
                      await markInstalled();
                      return;
                    }
                  } catch {
                    // ignore — keep polling
                  }
                }
              }
            }
          }
          // If the entry is not found, the download may have completed
          // and been cleaned up before we polled. Fall back to checking
          // the installed models list.
          if (!found) {
            missCount++;
            if (missCount >= 2) {
              try {
                const result = await invoke<{ models: InstalledModel[] }>('local_models_list');
                const models = result?.models ?? [];
                const installed = models.some(
                  (m) => m.hub_repo?.includes('bge') && m.status === 'available',
                );
                if (installed) {
                  await markInstalled();
                  return;
                }
              } catch {
                // ignore — keep polling
              }
            }
          }
        } catch {
          // ignore poll errors
        } finally {
          busy = false;
        }
      })();
    }, 2_000);
  };

  onCleanup(clearPoll);

  return (
    <div class="flex flex-col items-center w-full max-w-lg mx-auto animate-in fade-in slide-in-from-right-4 duration-400">
      <h2 class="text-2xl font-bold text-foreground">Local Embedding Model</h2>
      <p class="mt-2 text-sm text-muted-foreground text-center">
        HiveMind OS uses a small local model for text embeddings, enabling powerful search and memory features — all running privately on your device.
      </p>

      <div class="mt-6 w-full rounded-xl border bg-card p-5">
        <div class="flex items-start gap-3">
          <div class="flex h-10 w-10 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
            <Download size={20} class="text-primary" />
          </div>
          <div class="flex-1">
            <h3 class="text-sm font-semibold text-foreground">BGE Small EN v1.5</h3>
            <p class="text-xs text-muted-foreground mt-0.5">
              Text embeddings for memory &amp; search · ONNX · ~127 MB
            </p>
            <p class="text-xs text-muted-foreground mt-0.5 font-mono">
              {BGE_REPO}
            </p>
          </div>
        </div>

        <Show when={status() === 'idle'}>
          <div class="mt-4">
            <Button onClick={startDownload} class="w-full">
              <Download size={16} class="mr-2" />
              Download & Install
            </Button>
          </div>
        </Show>

        <Show when={status() === 'downloading'}>
          <div class="mt-4 space-y-2">
            <Show when={pct() < 100} fallback={
              <div class="flex items-center gap-2 text-sm text-muted-foreground">
                <Loader2 size={16} class="animate-spin text-primary" />
                <span>Finalizing installation…</span>
              </div>
            }>
              <div class="flex justify-between text-xs text-muted-foreground">
                <span>Downloading…</span>
                <span>{formatBytes(progress().downloaded)} / {formatBytes(progress().total)} ({pct()}%)</span>
              </div>
              <div class="h-2 w-full overflow-hidden rounded-full bg-secondary">
                <div
                  class="h-full rounded-full bg-primary transition-all duration-300"
                  style={`width: ${pct()}%`}
                />
              </div>
            </Show>
          </div>
        </Show>

        <Show when={status() === 'done'}>
          <div class="mt-4 flex items-center gap-2 text-sm text-green-500 font-medium">
            <CheckCircle size={18} />
            <span>Installed successfully</span>
          </div>
        </Show>

        <Show when={status() === 'error'}>
          <div class="mt-4 space-y-2">
            <div class="flex items-center gap-1.5 text-xs text-destructive">
              <AlertCircle size={14} />
              <span>{errorMsg()}</span>
            </div>
            <Button variant="secondary" size="sm" onClick={startDownload}>
              Retry
            </Button>
          </div>
        </Show>
      </div>

      <div class="mt-8 flex items-center gap-3">
        <Button variant="ghost" onClick={props.onBack}>
          Back
        </Button>
        <Button variant="secondary" onClick={props.onSkip}>
          Skip
        </Button>
        <Button
          onClick={props.onNext}
          disabled={status() === 'downloading'}
        >
          Next
        </Button>
      </div>

      <Show when={status() !== 'done' && status() !== 'downloading'}>
        <p class="mt-2 text-xs text-muted-foreground text-center">
          Skipping this will limit search and memory features until an embedding model is configured.
        </p>
      </Show>
    </div>
  );
};

export default ModelsStep;
