import { Show } from 'solid-js';
import { XCircle } from 'lucide-solid';
import { Dialog, DialogContent, Button } from '~/ui';

export function TestConnectionModal(props: {
  testing: () => boolean;
  testResult: () => string | null;
  onClose: () => void;
}) {
  return (
    <Dialog
      open={props.testing() || !!props.testResult()}
      onOpenChange={(open) => { if (!open && !props.testing()) props.onClose(); }}
    >
      <DialogContent
        class="min-w-[340px] max-w-[480px] text-center"
        onInteractOutside={(e: Event) => { if (props.testing()) e.preventDefault(); }}
        onEscapeKeyDown={(e: KeyboardEvent) => { if (props.testing()) e.preventDefault(); }}
        data-testid="test-connection-modal"
      >
        <Show when={props.testing()}>
          <div class="mb-4">
            <div class="mx-auto mb-4 h-12 w-12 animate-spin rounded-full border-[3px] border-primary/20 border-t-primary" />
            <p class="text-base font-semibold text-foreground">Testing Connection…</p>
            <p class="mt-1 text-sm text-muted-foreground">Verifying credentials and connectivity</p>
          </div>
        </Show>
        <Show when={!props.testing() && props.testResult()}>
          <div>
            <Show when={!props.testResult()!.startsWith('Error')}>
              <div class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-green-500/10 text-2xl">✅</div>
              <p class="text-base font-semibold text-green-500">Connection Successful</p>
              <p class="mt-1 text-sm text-muted-foreground">The connector is working correctly.</p>
            </Show>
            <Show when={props.testResult()!.startsWith('Error')}>
              <div class="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-full bg-destructive/10 text-2xl">
                <XCircle size={24} />
              </div>
              <p class="text-base font-semibold text-destructive">Connection Failed</p>
              <p class="mt-2 max-h-[120px] overflow-auto break-words whitespace-pre-wrap rounded-lg bg-black/20 px-3 py-2 text-left text-xs text-muted-foreground">
                {props.testResult()!.replace(/^Error:\s*/, '')}
              </p>
            </Show>
            <Button variant="secondary" class="mt-5" onClick={props.onClose}>Close</Button>
          </div>
        </Show>
      </DialogContent>
    </Dialog>
  );
}
