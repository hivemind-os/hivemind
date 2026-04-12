import { Show, createSignal, createEffect, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '~/ui';
import type { AppContext, HiveMindConfigData } from '../../types';

export interface WebSearchStepProps {
  context: Accessor<AppContext | null>;
  onNext: () => void;
  onBack: () => void;
  onSkip: () => void;
}

const WebSearchStep = (props: WebSearchStepProps) => {
  const [provider, setProvider] = createSignal('none');
  const [apiKey, setApiKey] = createSignal('');
  const [error, setError] = createSignal<string | null>(null);

  // Load existing config on mount; also load key from keyring
  createEffect(() => {
    void (async () => {
      try {
        const cfg: HiveMindConfigData = await invoke('config_get');
        if (cfg.web_search?.provider) setProvider(cfg.web_search.provider);
        // Try keyring first, fall back to config api_key
        try {
          const secret = await invoke<string | null>('load_secret', { key: 'web-search:api-key' });
          if (secret) { setApiKey(secret); return; }
        } catch { /* keyring unavailable */ }
        if (cfg.web_search?.api_key) setApiKey(cfg.web_search.api_key);
      } catch { /* ignore load errors in wizard */ }
    })();
  });

  const saveAndContinue = async () => {
    setError(null);
    try {
      const key = apiKey();
      // Save API key to OS keyring if it's a literal (not env: reference)
      if (key && !key.startsWith('env:')) {
        await invoke('save_secret', { key: 'web-search:api-key', value: key });
      }
      const cfg: HiveMindConfigData = await invoke('config_get');
      const isEnvRef = key.startsWith('env:');
      const updated: HiveMindConfigData = {
        ...cfg,
        web_search: provider() === 'none'
          ? undefined
          : { provider: provider(), api_key: isEnvRef ? key : (key ? 'keyring:web-search:api-key' : null) },
      };
      await invoke('config_save', { config: updated });
      props.onNext();
    } catch {
      setError('Failed to save configuration. Please try again.');
    }
  };

  return (
    <div class="flex flex-col items-center w-full max-w-4xl mx-auto animate-in fade-in slide-in-from-right-4 duration-400">
      <h2 class="text-2xl font-bold text-foreground">Web Search (Optional)</h2>
      <p class="mt-2 text-sm text-muted-foreground text-center max-w-md">
        Agents can search the web for libraries, documentation, and best practices if a search provider is configured. You can always set this up later in Settings.
      </p>

      <div class="mt-6 w-full max-w-md space-y-4">
        <div class="settings-field">
          <label class="text-sm font-medium text-foreground">Provider</label>
          <select
            class="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
            value={provider()}
            onChange={(e) => setProvider(e.currentTarget.value)}
          >
            <option value="none">None (disabled)</option>
            <option value="brave">Brave Search</option>
            <option value="tavily">Tavily</option>
          </select>
        </div>

        <Show when={provider() !== 'none'}>
          <div class="settings-field">
            <label class="text-sm font-medium text-foreground">API Key</label>
            <input
              type="password"
              class="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
              placeholder="Enter API key or env:VAR_NAME"
              value={apiKey()}
              onInput={(e) => setApiKey(e.currentTarget.value)}
            />
          </div>
          <p class="text-xs text-muted-foreground">
            {provider() === 'brave'
              ? <>Get an API key at <a href="https://brave.com/search/api/" target="_blank" rel="noopener noreferrer" class="underline">brave.com/search/api</a> ($5/month free credits, ~1,000 queries/month)</>
              : <>Get an API key at <a href="https://app.tavily.com" target="_blank" rel="noopener noreferrer" class="underline">app.tavily.com</a></>
            }
          </p>
          <p class="text-xs text-muted-foreground">
            Tip: Use <code>env:BRAVE_API_KEY</code> or <code>env:TAVILY_API_KEY</code> to read from environment variables.
          </p>
        </Show>
      </div>

      <Show when={error()}>
        <p class="mt-4 text-sm text-destructive">{error()}</p>
      </Show>

      <div class="mt-8 flex items-center gap-3">
        <Button variant="ghost" onClick={props.onBack}>
          Back
        </Button>
        <Button variant="secondary" onClick={props.onSkip}>
          Skip
        </Button>
        <Button onClick={() => void saveAndContinue()}>
          Next
        </Button>
      </div>
    </div>
  );
};

export default WebSearchStep;
