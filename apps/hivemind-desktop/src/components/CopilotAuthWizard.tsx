import { Show, createSignal, onCleanup, onMount } from 'solid-js';
import { CheckCircle, Lock, ClipboardList, XCircle } from 'lucide-solid';
import { Button } from '~/ui';
import { invoke } from '@tauri-apps/api/core';

export interface CopilotAuthWizardProps {
  provider_id: string;
  onComplete: (token: string) => void;
  onCancel: () => void;
  onModelsLoaded?: (models: Array<{ id: string; name?: string; version?: string }>) => void;
}

const CopilotAuthWizard = (props: CopilotAuthWizardProps) => {
  const [step, setStep] = createSignal<'checking' | 'authenticated' | 'start' | 'waiting' | 'complete' | 'error'>('checking');
  const [userCode, setUserCode] = createSignal('');
  const [verificationUri, setVerificationUri] = createSignal('');
  const [error, setError] = createSignal('');
  const [username, setUsername] = createSignal('');
  const [copilotModels, setCopilotModels] = createSignal<Array<{ id: string; name?: string; version?: string }>>([]);
  const [loadingModels, setLoadingModels] = createSignal(false);

  let pollTimer: number | undefined;
  let mounted = true;

  const schedulePoll = (fn: () => void, delayMs: number) => {
    if (!mounted) return;
    if (pollTimer !== undefined) window.clearTimeout(pollTimer);
    pollTimer = window.setTimeout(fn, delayMs);
  };

  const checkAuthStatus = async () => {
    try {
      const data = await invoke<{ authenticated: boolean; username?: string }>('github_auth_status');
      if (!mounted) return;
      if (data.authenticated) {
        setUsername(data.username || '');
        setStep('authenticated');
        void fetchModels();
      } else {
        setStep('start');
      }
    } catch {
      if (mounted) setStep('start');
    }
  };

  const fetchModels = async () => {
    setLoadingModels(true);
    try {
      const data = await invoke<unknown>('github_list_models');
      const arr = data as Record<string, unknown>;
      const models = Array.isArray(data) ? data : ((arr.data ?? arr.models ?? []) as Array<{ id: string; name?: string; version?: string }>);
      if (mounted) {
        setCopilotModels(models);
        props.onModelsLoaded?.(models);
      }
    } catch {
      // ignore
    }
    if (mounted) setLoadingModels(false);
  };

  const disconnect = async () => {
    try {
      await invoke('github_disconnect');
    } catch {
      // ignore
    }
    setUsername('');
    setCopilotModels([]);
    setStep('start');
  };

  const pollForToken = (code: string, interval: number) => {
    const poll = async () => {
      try {
        const data = await invoke<{ status: string; access_token?: string; error?: string }>('github_poll_token', { device_code: code });
        if (!mounted) return;

        if (data.status === 'complete') {
          await invoke('github_save_token', { provider_id: props.provider_id, token: data.access_token });
          if (!mounted) return;
          setStep('complete');
          props.onComplete(data.access_token!);
          schedulePoll(() => { void checkAuthStatus(); }, 500);
          return;
        }

        if (data.status === 'failed') {
          setError(data.error || 'Authentication failed');
          setStep('error');
          return;
        }

        schedulePoll(() => { void poll(); }, interval * 1000);
      } catch (e) {
        if (!mounted) return;
        setError(String(e));
        setStep('error');
      }
    };

    schedulePoll(() => { void poll(); }, interval * 1000);
  };

  const startFlow = async () => {
    try {
      const data = await invoke<{ device_code: string; user_code: string; verification_uri: string; expires_in: number; interval: number; error?: string }>('github_start_device_flow');
      if (data.error) {
        setError(data.error);
        setStep('error');
        return;
      }

      setUserCode(data.user_code);
      setVerificationUri(data.verification_uri);
      setStep('waiting');
      pollForToken(data.device_code, data.interval || 5);
    } catch (e) {
      setError(String(e));
      setStep('error');
    }
  };

  onMount(() => {
    void checkAuthStatus();
  });

  onCleanup(() => {
    mounted = false;
    if (pollTimer) window.clearTimeout(pollTimer);
  });

  return (
    <div class="my-2 rounded-lg border border-input p-4" data-testid="copilot-auth-wizard">
      <Show when={step() === 'checking'}>
        <p class="text-sm text-muted-foreground">Checking authentication status…</p>
      </Show>

      <Show when={step() === 'authenticated'}>
        <h3 class="mb-2 flex items-center gap-1.5 text-sm font-semibold text-foreground"><CheckCircle size={14} /> Connected{username() ? ` as ${username()}` : ''}</h3>
        <p class="mb-3 text-sm text-muted-foreground">GitHub Copilot is ready to use.</p>
        <Show when={loadingModels()}>
          <p class="text-xs text-muted-foreground">Loading available models…</p>
        </Show>
        <Show when={!loadingModels() && copilotModels().length > 0}>
          <p class="mb-3 text-xs text-muted-foreground">
            {copilotModels().length} model(s) available — select which ones to enable below.
          </p>
        </Show>
        <Button variant="destructive" size="sm" onClick={disconnect}>Disconnect</Button>
      </Show>

      <Show when={step() === 'start'}>
        <h3 class="mb-2 flex items-center gap-1.5 text-sm font-semibold text-foreground"><Lock size={14} /> Connect GitHub Copilot</h3>
        <p class="mb-3 text-sm text-muted-foreground">
          Sign in with your GitHub account to use Copilot models. This uses the secure device flow — no passwords are shared with HiveMind OS.
        </p>
        <Button size="sm" onClick={startFlow}>Start Authentication</Button>
      </Show>

      <Show when={step() === 'waiting'}>
        <h3 class="mb-2 flex items-center gap-1.5 text-sm font-semibold text-foreground"><ClipboardList size={14} /> Enter Code on GitHub</h3>
        <p class="mb-2 text-sm">1. Copy this code:</p>
        <div class="mb-3 select-all rounded-md bg-secondary p-3 text-center text-2xl font-bold tracking-widest">
          {userCode()}
        </div>
        <p class="mb-3 text-sm">
          2. Open <a href={verificationUri()} target="_blank" class="text-primary underline">{verificationUri()}</a> and paste the code.
        </p>
        <p class="text-xs text-muted-foreground">Waiting for GitHub authentication to complete…</p>
      </Show>

      <Show when={step() === 'complete'}>
        <h3 class="mb-2 flex items-center gap-1.5 text-sm font-semibold text-foreground"><CheckCircle size={14} /> Authentication complete</h3>
        <p class="text-xs text-muted-foreground">Verifying your account…</p>
      </Show>

      <Show when={step() === 'error'}>
        <h3 class="mb-2 flex items-center gap-1.5 text-sm font-semibold text-destructive"><XCircle size={14} /> Authentication error</h3>
        <p class="mb-3 text-sm text-muted-foreground">{error()}</p>
        <div class="flex gap-2">
          <Button size="sm" onClick={startFlow}>Try Again</Button>
          <Button variant="secondary" size="sm" onClick={props.onCancel}>Cancel</Button>
        </div>
      </Show>
    </div>
  );
};

export default CopilotAuthWizard;
