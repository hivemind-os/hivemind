import { Show, For, createSignal, createMemo, createEffect, on, type Accessor } from 'solid-js';
import { Brain, Cloud, Server, Globe, CheckCircle, Plus, Pencil, Zap, X } from 'lucide-solid';
import { Button } from '~/ui';
import { createConfigStore, openExternal } from '../../stores/configStore';
import ProvidersTab from '../settings/ProvidersTab';
import type { AppContext, InstalledModel, ProviderKind, ModelProviderConfig } from '../../types';

export interface ProvidersStepProps {
  context: Accessor<AppContext | null>;
  localModels: Accessor<InstalledModel[]>;
  onNext: () => void;
  onBack: () => void;
}

const PROVIDER_TYPES: {
  kind: ProviderKind;
  name: string;
  description: string;
  icon: typeof Brain;
  defaults: Partial<ModelProviderConfig>;
}[] = [
  {
    kind: 'anthropic',
    name: 'Anthropic',
    description: 'Claude models via the Anthropic API',
    icon: Brain,
    defaults: { name: 'Anthropic', auth: 'api-key', base_url: 'https://api.anthropic.com' },
  },
  {
    kind: 'open-ai-compatible',
    name: 'OpenAI Compatible',
    description: 'GPT models or any OpenAI-compatible API',
    icon: Cloud,
    defaults: { name: 'OpenAI', auth: 'api-key', base_url: 'https://api.openai.com/v1' },
  },
  {
    kind: 'github-copilot',
    name: 'GitHub Copilot',
    description: 'Use models through your GitHub Copilot subscription',
    icon: Zap,
    defaults: { name: 'GitHub Copilot', auth: 'github-oauth', base_url: 'https://api.githubcopilot.com' },
  },
  {
    kind: 'microsoft-foundry',
    name: 'Azure AI Foundry',
    description: 'Azure-hosted models via Microsoft AI Foundry',
    icon: Globe,
    defaults: { name: 'Azure AI Foundry', auth: 'api-key', base_url: '' },
  },
  {
    kind: 'ollama-local',
    name: 'Ollama (Local)',
    description: 'Run open-source models locally with Ollama',
    icon: Server,
    defaults: { name: 'Ollama', auth: 'none', base_url: 'http://localhost:11434' },
  },
];

const ProvidersStep = (props: ProvidersStepProps) => {
  const [activeScreen] = createSignal('settings');
  const store = createConfigStore({
    activeScreen,
    loadPersonas: async () => {},
    setToolDefinitions: () => {},
    setUserStatus: () => {},
  });

  const providers = createMemo(() => store.editConfig()?.models?.providers ?? []);

  // Track newly-added providers so we can remove them if the dialog is cancelled
  const [pendingNewIdx, setPendingNewIdx] = createSignal<number | null>(null);

  const configuredKinds = createMemo(() => {
    const kinds = new Set<ProviderKind>();
    for (const p of providers()) kinds.add(p.kind);
    return kinds;
  });

  const hasProviders = createMemo(() => providers().length > 0);

  const addProviderOfKind = (kind: ProviderKind) => {
    const typeDef = PROVIDER_TYPES.find((t) => t.kind === kind);
    if (!typeDef) return;

    store.setEditConfig((c) => {
      if (!c) return null;
      // Generate a readable ID, appending a suffix if one for this kind already exists
      const existing = c.models.providers.filter((p) => p.id === kind || p.id.startsWith(`${kind}-`));
      const id = existing.length === 0 ? kind : `${kind}-${existing.length + 1}`;
      const newP: ModelProviderConfig = {
        id,
        name: typeDef.defaults.name ?? typeDef.name,
        kind,
        base_url: typeDef.defaults.base_url ?? null,
        auth: typeDef.defaults.auth ?? 'api-key',
        models: [],
        model_capabilities: {},
        channel_class: 'internal',
        priority: 50,
        enabled: true,
        options: {
          route: null,
          allow_model_discovery: kind !== 'ollama-local',
          default_api_version: null,
          response_prefix: null,
          headers: {},
        },
      };
      const updated = { ...c, models: { ...c.models, providers: [...c.models.providers, newP] } };
      const newIdx = updated.models.providers.length - 1;
      setPendingNewIdx(newIdx);
      setTimeout(() => store.setEditingProviderIdx(newIdx), 0);
      return updated;
    });
  };

  const editExistingProvider = (kind: ProviderKind) => {
    const idx = providers().findIndex((p) => p.kind === kind);
    if (idx >= 0) store.setEditingProviderIdx(idx);
  };

  const removeProviderOfKind = (e: MouseEvent, kind: ProviderKind) => {
    e.stopPropagation();
    const idx = providers().findIndex((p) => p.kind === kind);
    if (idx >= 0) {
      store.removeProvider(idx);
      void store.saveConfig();
    }
  };

  const handleCardClick = (kind: ProviderKind) => {
    if (configuredKinds().has(kind)) {
      editExistingProvider(kind);
    } else {
      addProviderOfKind(kind);
    }
  };

  const handleProviderSaved = () => {
    setPendingNewIdx(null);
  };

  // When dialog closes and a new provider wasn't saved, remove it
  createEffect(
    on(() => store.editingProviderIdx(), (idx, prevIdx) => {
      if (idx === null && prevIdx !== null && prevIdx !== undefined) {
        const pending = pendingNewIdx();
        if (pending !== null) {
          setPendingNewIdx(null);
          store.removeProvider(pending);
        }
      }
    })
  );

  const configLoadingFallback = (label: string) => (
    <Show when={store.configLoadError()} fallback={
      <p class="text-muted-foreground text-sm">Loading {label}…</p>
    }>
      <div class="flex flex-col items-start gap-2 py-2">
        <p class="text-sm text-destructive">Failed to load: {store.configLoadError()}</p>
        <Button variant="secondary" onClick={() => void store.loadEditConfig()}>Retry</Button>
      </div>
    </Show>
  );

  return (
    <div class="flex flex-col items-center w-full max-w-3xl mx-auto animate-in fade-in slide-in-from-right-4 duration-400">
      <h2 class="text-2xl font-bold text-foreground">Model Providers</h2>
      <p class="mt-2 text-sm text-muted-foreground text-center max-w-md">
        Connect at least one AI model provider to get started.
      </p>

      {/* Provider type cards */}
      <Show when={store.editConfig()} fallback={
        <div class="mt-8">{configLoadingFallback('providers')}</div>
      }>
        <div class="mt-8 w-full grid grid-cols-1 sm:grid-cols-2 gap-4">
          <For each={PROVIDER_TYPES}>
            {(typeDef) => {
              const isConfigured = () => configuredKinds().has(typeDef.kind);
              const Icon = typeDef.icon;
              return (
                <button
                  class={`group relative flex items-start gap-4 rounded-lg border p-4 text-left transition-all hover:shadow-md cursor-pointer ${
                    isConfigured()
                      ? 'border-green-500/50 bg-green-500/5 hover:border-green-500'
                      : 'border-border bg-card hover:border-primary/50'
                  }`}
                  onClick={() => handleCardClick(typeDef.kind)}
                >
                  {/* Remove button for configured providers */}
                  <Show when={isConfigured()}>
                    <span
                      class="absolute top-2 right-2 p-1 rounded-md text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors cursor-pointer"
                      onClick={(e) => removeProviderOfKind(e, typeDef.kind)}
                      title="Remove provider"
                    >
                      <X size={14} />
                    </span>
                  </Show>
                  <div class={`flex h-10 w-10 shrink-0 items-center justify-center rounded-lg ${
                    isConfigured() ? 'bg-green-500/10 text-green-500' : 'bg-muted text-muted-foreground'
                  }`}>
                    <Icon size={20} />
                  </div>
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="font-medium text-foreground">{typeDef.name}</span>
                      <Show when={isConfigured()}>
                        <CheckCircle size={14} class="text-green-500" />
                      </Show>
                    </div>
                    <p class="mt-0.5 text-xs text-muted-foreground">{typeDef.description}</p>
                    <span class={`mt-2 inline-flex items-center gap-1 text-xs font-medium ${
                      isConfigured() ? 'text-green-600' : 'text-primary'
                    }`}>
                      <Show when={isConfigured()} fallback={<><Plus size={12} /> Configure</>}>
                        <Pencil size={12} /> Edit
                      </Show>
                    </span>
                  </div>
                </button>
              );
            }}
          </For>
        </div>
      </Show>

      {/* Hidden ProvidersTab — only renders the edit dialog via portal */}
      <div class="hidden">
        <ProvidersTab
          editConfig={store.editConfig}
          editingProviderIdx={store.editingProviderIdx}
          setEditingProviderIdx={store.setEditingProviderIdx}
          addProvider={store.addProvider}
          removeProvider={store.removeProvider}
          moveProvider={store.moveProvider}
          updateProvider={store.updateProvider}
          addModelToProvider={store.addModelToProvider}
          removeModelFromProvider={store.removeModelFromProvider}
          saveConfig={store.saveConfig}
          localModels={props.localModels}
          context={props.context}
          openExternal={openExternal}
          configLoadingFallback={configLoadingFallback}
          hideDelete
          onSave={handleProviderSaved}
        />
      </div>

      <div class="mt-8 flex flex-col items-center gap-3">
        <div class="flex items-center gap-3">
          <Button variant="ghost" onClick={props.onBack}>
            Back
          </Button>
          <Button onClick={props.onNext} disabled={!hasProviders()}>
            Next
          </Button>
        </div>
        <Show when={!hasProviders()}>
          <p class="text-xs text-muted-foreground">Configure at least one provider to continue</p>
        </Show>
      </div>
    </div>
  );
};

export default ProvidersStep;
