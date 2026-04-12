import { createEffect, createSignal, Show } from 'solid-js';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
} from '~/ui/dialog';

export interface UpdateDialogProps {
  open: boolean;
  update: Update | null;
  onClose: () => void;
}

export function UpdateDialog(props: UpdateDialogProps) {
  const [downloading, setDownloading] = createSignal(false);
  const [progress, setProgress] = createSignal(0);
  const [installed, setInstalled] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // Reset state each time the dialog opens so stale error/progress from a
  // previous attempt doesn't carry over.
  createEffect(() => {
    if (props.open) {
      setDownloading(false);
      setProgress(0);
      setInstalled(false);
      setError(null);
    }
  });

  const handleUpdate = async () => {
    if (!props.update) return;
    setDownloading(true);
    setError(null);
    setProgress(0);

    try {
      let totalLen = 0;
      let downloaded = 0;
      await props.update.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          totalLen = (event.data as any).contentLength ?? 0;
        } else if (event.event === 'Progress') {
          downloaded += (event.data as any).chunkLength ?? 0;
          if (totalLen > 0) {
            setProgress(Math.round((downloaded / totalLen) * 100));
          }
        } else if (event.event === 'Finished') {
          setProgress(100);
        }
      });
      setDownloading(false);
      setInstalled(true);
    } catch (e: any) {
      setError(e?.message ?? String(e));
      setDownloading(false);
    }
  };

  const handleRelaunch = async () => {
    await relaunch();
  };

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open && !downloading()) props.onClose();
      }}
    >
      <DialogContent class="max-w-md" onInteractOutside={(e: Event) => { if (downloading()) e.preventDefault(); }}>
        <DialogHeader>
          <DialogTitle>
            {installed() ? 'Update Installed' : 'Update Available'}
          </DialogTitle>
          <DialogDescription>
            <Show when={!installed()}>
              A new version of HiveMind OS is available.
            </Show>
            <Show when={installed()}>
              HiveMind OS has been updated. Restart to apply the changes.
            </Show>
          </DialogDescription>
        </DialogHeader>

        <div class="space-y-3 text-sm">
          <Show when={props.update && !installed()}>
            <div class="flex justify-between">
              <span class="text-muted-foreground">New version</span>
              <span class="font-medium">{props.update?.version}</span>
            </div>
            <Show when={props.update?.body}>
              <div class="rounded border border-border bg-muted/50 p-3 text-xs max-h-40 overflow-y-auto whitespace-pre-wrap">
                {props.update!.body}
              </div>
            </Show>
          </Show>

          <Show when={downloading() && !installed()}>
            <div class="space-y-1">
              <div class="flex justify-between text-xs text-muted-foreground">
                <span>Downloading…</span>
                <span>{progress()}%</span>
              </div>
              <div class="h-2 rounded-full bg-muted overflow-hidden">
                <div
                  class="h-full rounded-full bg-primary transition-all duration-300"
                  style={`width: ${progress()}%`}
                />
              </div>
            </div>
          </Show>

          <Show when={error()}>
            <div class="rounded border border-destructive/50 bg-destructive/10 p-3 text-xs text-destructive">
              Update failed: {error()}
            </div>
          </Show>
        </div>

        <DialogFooter>
          <Show when={installed()}>
            <button
              class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              onClick={handleRelaunch}
            >
              Restart Now
            </button>
          </Show>
          <Show when={!installed() && !downloading()}>
            <button
              class="inline-flex items-center justify-center rounded-md border border-border px-4 py-2 text-sm font-medium hover:bg-accent"
              onClick={() => props.onClose()}
            >
              Remind Me Later
            </button>
            <button
              class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
              onClick={handleUpdate}
            >
              Update Now
            </button>
          </Show>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
