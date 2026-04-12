import { Show, createEffect, onCleanup, type Accessor, type Setter } from 'solid-js';
import { ClipboardList, KeyRound, X } from 'lucide-solid';
import { isNoTokenError, isLicenseError, extractRepoFromError, openExternal } from '../utils';
import { Button } from '~/ui';

export interface ErrorBannerProps {
  errorMessage: Accessor<string | null>;
  setErrorMessage: Setter<string | null>;
  /** Optional callback to trigger token-input focus from the parent */
  onFocusTokenInput?: () => void;
}

const ErrorBanner = (props: ErrorBannerProps) => {
  // Auto-dismiss generic errors after 10 seconds (not token/license errors)
  createEffect(() => {
    const msg = props.errorMessage();
    if (msg && !isNoTokenError(msg) && !isLicenseError(msg)) {
      const timer = setTimeout(() => props.setErrorMessage(null), 10_000);
      onCleanup(() => clearTimeout(timer));
    }
  });
  return (
    <Show when={props.errorMessage()}>
      {(message) => (
        <Show when={isNoTokenError(message())} fallback={
          <Show when={isLicenseError(message())} fallback={
            <section class="rounded-md border border-destructive bg-destructive/10 p-3 text-sm text-destructive mb-3 flex items-start gap-2" data-testid="error-banner">
              <span class="flex-1">Error: {message()}</span>
              <button
                class="shrink-0 cursor-pointer border-none bg-transparent p-0.5 text-destructive/60 hover:text-destructive"
                onClick={() => props.setErrorMessage(null)}
                title="Dismiss"
              >
                <X size={14} />
              </button>
            </section>
          }>
            <section class="rounded-md border border-blue-500 bg-secondary p-3 mb-3" data-testid="error-banner-license">
              <p class="mb-2 font-semibold text-blue-400 flex items-center gap-1.5"><ClipboardList size={14} /> License agreement required</p>
              <p class="mb-2 text-sm text-muted-foreground">
                This model is gated. You must agree to its license terms on HuggingFace before downloading.
              </p>
              <div class="flex gap-2 items-center flex-wrap">
                <Show when={extractRepoFromError(message())}>
                  {(repo) => (
                    <Button variant="outline" size="sm" onClick={() => openExternal(`https://huggingface.co/${repo()}`)}>
                      Agree to License →
                    </Button>
                  )}
                </Show>
                <Button variant="ghost" size="sm" onClick={() => props.setErrorMessage(null)}>Dismiss</Button>
              </div>
              <p class="mt-2 text-xs text-muted-foreground">
                After agreeing, click Install again.
              </p>
            </section>
          </Show>
        }>
          <section class="rounded-md border border-yellow-500 bg-secondary p-3 mb-3" data-testid="error-banner-token">
            <p class="mb-2 font-semibold text-yellow-400 flex items-center gap-1.5"><KeyRound size={14} /> HuggingFace token required</p>
            <p class="mb-2 text-sm text-muted-foreground">
              You need a HuggingFace access token to download this model.
            </p>
            <div class="flex gap-2 items-center flex-wrap">
              <Button variant="outline" size="sm" onClick={() => openExternal('https://huggingface.co/settings/tokens')}>
                Create Token →
              </Button>
              <Button variant="ghost" size="sm" onClick={() => {
                if (props.onFocusTokenInput) {
                  props.onFocusTokenInput();
                } else {
                  const el = document.getElementById('hf-token-input');
                  if (el) { el.focus(); el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }
                }
              }}>
                ↑ Add Token Above
              </Button>
              <Button variant="ghost" size="sm" onClick={() => props.setErrorMessage(null)}>Dismiss</Button>
            </div>
          </section>
        </Show>
      )}
    </Show>
  );
};

export default ErrorBanner;
