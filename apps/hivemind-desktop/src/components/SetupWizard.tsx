import { Show, createSignal, onCleanup, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { CheckCircle, XCircle, PartyPopper } from 'lucide-solid';
import { Dialog, DialogContent, Button } from '~/ui';
import type { DownloadProgress, InstalledModel, HiveMindConfigData, CapabilityOption } from '../types';
import { formatBytes } from '../utils';

export interface SetupWizardProps {
  localModels: Accessor<InstalledModel[]>;
  startDownloadPolling: () => void;
  loadLocalModels: () => Promise<void>;
  onComplete: () => Promise<void>;
}

const SetupWizard = (props: SetupWizardProps) => {
  const [step, setStep] = createSignal<'welcome' | 'downloading' | 'complete'>('welcome');
  const [chatProgress, setChatProgress] = createSignal({ downloaded: 0, total: 2490000000 });
  const [embedProgress, setEmbedProgress] = createSignal({ downloaded: 0, total: 127000000 });
  const [chatStatus, setChatStatus] = createSignal<'pending' | 'downloading' | 'done' | 'error'>('pending');
  const [embedStatus, setEmbedStatus] = createSignal<'pending' | 'downloading' | 'done' | 'error'>('pending');

  let pollInterval: number | undefined;

  const pct = (p: { downloaded: number; total: number }) =>
    p.total > 0 ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : 0;

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
      const mc = { ...(provider.model_capabilities ?? {}), [bgeModel.id]: ['embedding'] as CapabilityOption[] };
      const updatedProviders = [...cfg.models.providers];
      updatedProviders[providerIdx] = { ...provider, model_capabilities: mc };

      const updated = { ...cfg, models: { ...cfg.models, providers: updatedProviders } };
      await invoke('config_save', { config: updated });
    } catch {
      // best-effort — user can configure manually later
    }
  };

  const startDownloads = async () => {
    setStep('downloading');

    try {
      setChatStatus('downloading');
      await invoke('local_models_install', {
        hub_repo: 'lmstudio-community/gemma-3-4b-it-GGUF',
        filename: 'gemma-3-4b-it-Q4_K_M.gguf',
        runtime: 'llama-cpp',
      });
    } catch {
      setChatStatus('error');
    }

    try {
      setEmbedStatus('downloading');
      await invoke('local_models_install', {
        hub_repo: 'BAAI/bge-small-en-v1.5',
        filename: 'onnx/model.onnx',
        runtime: 'onnx',
      });
    } catch {
      setEmbedStatus('error');
    }

    props.startDownloadPolling();
    clearPoll();

    let setupPollBusy = false;
    pollInterval = window.setInterval(() => {
      if (setupPollBusy) return;
      setupPollBusy = true;
      void (async () => {
        try {
          const downloads = await invoke<DownloadProgress[]>('local_models_downloads');
          for (const d of downloads) {
            if (d.filename?.includes('gemma')) {
              setChatProgress({ downloaded: d.downloaded_bytes, total: d.total_bytes || 2490000000 });
              if (d.status === 'complete' || d.status === 'finalizing') setChatStatus('done');
              if (d.status === 'error' || d.status === 'failed') setChatStatus('error');
            }
            if (d.filename?.includes('model.onnx') || d.repo_id?.includes('bge')) {
              setEmbedProgress({ downloaded: d.downloaded_bytes, total: d.total_bytes || 127000000 });
              if (d.status === 'complete' || d.status === 'finalizing') setEmbedStatus('done');
              if (d.status === 'error' || d.status === 'failed') setEmbedStatus('error');
            }
          }

          // Fallback: if downloads disappeared (cleanup timer) but we
          // never saw 'complete', check the installed models registry.
          if (downloads.length === 0 && (chatStatus() === 'downloading' || embedStatus() === 'downloading')) {
            try {
              const result = await invoke<{ models: { hub_repo: string; status: string }[] }>('local_models_list');
              const models = result?.models ?? [];
              if (chatStatus() === 'downloading' && models.some(m => m.hub_repo?.includes('gemma'))) {
                setChatStatus('done');
              }
              if (embedStatus() === 'downloading' && models.some(m => m.hub_repo?.includes('bge') && m.status === 'available')) {
                setEmbedStatus('done');
              }
            } catch {
              // ignore
            }
          }

          if (chatStatus() !== 'downloading' && embedStatus() !== 'downloading') {
            clearPoll();
            if (chatStatus() === 'done' || embedStatus() === 'done') {
              await props.loadLocalModels();
              if (embedStatus() === 'done') await setBgeEmbeddingCapability();
              setStep('complete');
            }
          }
        } catch {
          // ignore poll errors
        } finally {
          setupPollBusy = false;
        }
      })();
    }, 3_000);
  };

  onCleanup(clearPoll);

  return (
    <Dialog open={true} onOpenChange={() => {}}>
      <DialogContent
        class="max-w-[520px]"
        onInteractOutside={(e: Event) => e.preventDefault()}
        onEscapeKeyDown={(e: KeyboardEvent) => e.preventDefault()}
        data-testid="setup-wizard"
      >
        <Show when={step() === 'welcome'}>
          <h2 class="text-xl font-bold text-foreground">Welcome to HiveMind OS</h2>
          <Show when={props.localModels().length > 0}>
            <p class="text-sm text-muted-foreground">You have {props.localModels().length} model(s) installed. You can download additional recommended models below, or get started right away.</p>
          </Show>
          <Show when={props.localModels().length === 0}>
            <p class="text-sm text-muted-foreground">HiveMind OS needs local AI models to work. We'll download two small models to get you started:</p>
          </Show>

          <div class="space-y-3 my-4">
            <div class="rounded-lg border border-input p-3">
              <label class="flex items-start gap-2 text-sm">
                <input type="checkbox" checked id="chat-model" class="mt-0.5 rounded" />
                <div>
                  <strong class="text-foreground">Gemma 3 4B Instruct</strong>
                  <span class="text-muted-foreground"> — General chat &amp; reasoning</span>
                  <p class="mt-1 text-xs text-muted-foreground">lmstudio-community/gemma-3-4b-it-GGUF · Q4_K_M · 2.3 GB</p>
                </div>
              </label>
            </div>

            <div class="rounded-lg border border-input p-3">
              <label class="flex items-start gap-2 text-sm">
                <input type="checkbox" checked id="embed-model" class="mt-0.5 rounded" />
                <div>
                  <strong class="text-foreground">BGE Small EN v1.5</strong>
                  <span class="text-muted-foreground"> — Text embeddings for memory &amp; search</span>
                  <p class="mt-1 text-xs text-muted-foreground">BAAI/bge-small-en-v1.5 · ONNX · 127 MB</p>
                </div>
              </label>
            </div>
          </div>

          <div class="flex gap-2 justify-end">
            <Button variant="secondary" onClick={() => void props.onComplete()}>
              {props.localModels().length > 0 ? 'Skip — use existing models' : "Skip — I'll configure manually"}
            </Button>
            <Button onClick={() => void startDownloads()}>
              Download &amp; Set Up
            </Button>
          </div>
        </Show>

        <Show when={step() === 'downloading'}>
          <h2 class="text-xl font-bold text-foreground">Setting up HiveMind OS…</h2>
          <p class="text-sm text-muted-foreground">Downloading models. This may take a few minutes.</p>

          <div class="mt-4 space-y-4">
            <div>
              <div class="mb-1 flex justify-between text-xs text-muted-foreground">
                <span>Gemma 3 4B (Q4_K_M)</span>
                <span>{formatBytes(chatProgress().downloaded)} / {formatBytes(chatProgress().total)}</span>
              </div>
              <div class="h-2 w-full overflow-hidden rounded-full bg-secondary">
                <div class="h-full rounded-full bg-primary transition-all" style={`width: ${pct(chatProgress())}%`} />
              </div>
              <Show when={chatStatus() === 'done'}><span class="text-xs text-green-400"><CheckCircle size={14} /> Installed</span></Show>
              <Show when={chatStatus() === 'error'}><span class="text-xs text-red-400"><XCircle size={14} /> Failed — will retry on next start</span></Show>
            </div>

            <div>
              <div class="mb-1 flex justify-between text-xs text-muted-foreground">
                <span>BGE Small EN v1.5</span>
                <span>{formatBytes(embedProgress().downloaded)} / {formatBytes(embedProgress().total)}</span>
              </div>
              <div class="h-2 w-full overflow-hidden rounded-full bg-secondary">
                <div class="h-full rounded-full bg-primary transition-all" style={`width: ${pct(embedProgress())}%`} />
              </div>
              <Show when={embedStatus() === 'done'}><span class="text-xs text-green-400"><CheckCircle size={14} /> Installed</span></Show>
              <Show when={embedStatus() === 'error'}><span class="text-xs text-red-400"><XCircle size={14} /> Failed — will retry on next start</span></Show>
            </div>
          </div>
        </Show>

        <Show when={step() === 'complete'}>
          <h2 class="text-xl font-bold text-foreground flex items-center gap-2"><PartyPopper size={24} /> You're all set!</h2>
          <p class="text-sm text-muted-foreground">HiveMind OS is ready to use with local AI models. Your data stays on your device.</p>
          <div class="mt-4 flex justify-end">
            <Button onClick={() => void props.onComplete()}>Start Using HiveMind OS</Button>
          </div>
        </Show>
      </DialogContent>
    </Dialog>
  );
};

export default SetupWizard;
