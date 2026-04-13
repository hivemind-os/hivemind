import { createEffect, createSignal, Show, Switch, Match } from 'solid-js';
import { type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogFooter,
  DialogTitle,
  DialogDescription,
} from '~/ui/dialog';

export type UpdateCheckState = 'idle' | 'checking' | 'up-to-date' | 'update-available' | 'error' | 'unavailable';

export interface UpdateDialogProps {
  open: boolean;
  update: Update | null;
  checkState: UpdateCheckState;
  checkError: string | null;
  onClose: () => void;
  onRetry: () => void;
}

export function UpdateDialog(props: UpdateDialogProps) {
  const [downloading, setDownloading] = createSignal(false);
  const [progress, setProgress] = createSignal(0);
  const [installed, setInstalled] = createSignal(false);
  const [downloadError, setDownloadError] = createSignal<string | null>(null);

  // Reset download state each time the dialog opens so stale error/progress
  // from a previous attempt doesn't carry over.
  createEffect(() => {
    if (props.open) {
      setDownloading(false);
      setProgress(0);
      setInstalled(false);
      setDownloadError(null);
    }
  });

  const handleUpdate = async () => {
    if (!props.update) return;
    setDownloading(true);
    setDownloadError(null);
    setProgress(0);

    try {
      // Stop the daemon before updating so the installer can replace
      // binaries that may be in use (especially on Windows).
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        await invoke('daemon_stop');
      } catch {
        // Daemon might already be stopped — continue with update.
      }

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
      setDownloadError(e?.message ?? String(e));
      setDownloading(false);
    }
  };

  const handleRelaunch = async () => {
    await relaunch();
  };

  const canDismiss = () => !downloading();

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open && canDismiss()) props.onClose();
      }}
    >
      <DialogContent class="max-w-md" onInteractOutside={(e: Event) => { if (!canDismiss()) e.preventDefault(); }}>
        <Switch>
          {/* ── Checking for updates ─────────────────────────────── */}
          <Match when={props.checkState === 'checking'}>
            <DialogHeader>
              <DialogTitle>Checking for Updates</DialogTitle>
              <DialogDescription>Please wait while we check for updates…</DialogDescription>
            </DialogHeader>
            <div class="flex items-center justify-center py-6">
              <div class="h-6 w-6 animate-spin rounded-full border-2 border-muted border-t-primary" />
            </div>
          </Match>

          {/* ── Already up to date ───────────────────────────────── */}
          <Match when={props.checkState === 'up-to-date'}>
            <DialogHeader>
              <DialogTitle>You're Up to Date</DialogTitle>
              <DialogDescription>HiveMind OS is already running the latest version.</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <button
                class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
                onClick={() => props.onClose()}
              >
                OK
              </button>
            </DialogFooter>
          </Match>

          {/* ── Updater not available (dev builds) ───────────────── */}
          <Match when={props.checkState === 'unavailable'}>
            <DialogHeader>
              <DialogTitle>Updates Not Available</DialogTitle>
              <DialogDescription>Auto-updates are not available in this build.</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <button
                class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
                onClick={() => props.onClose()}
              >
                OK
              </button>
            </DialogFooter>
          </Match>

          {/* ── Check failed with error ──────────────────────────── */}
          <Match when={props.checkState === 'error'}>
            <DialogHeader>
              <DialogTitle>Update Check Failed</DialogTitle>
              <DialogDescription>We couldn't check for updates. Please try again later.</DialogDescription>
            </DialogHeader>
            <Show when={props.checkError}>
              <div class="rounded border border-destructive/50 bg-destructive/10 p-3 text-xs text-destructive">
                {props.checkError}
              </div>
            </Show>
            <DialogFooter>
              <button
                class="inline-flex items-center justify-center rounded-md border border-border px-4 py-2 text-sm font-medium hover:bg-accent"
                onClick={() => props.onClose()}
              >
                Close
              </button>
              <button
                class="inline-flex items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground hover:bg-primary/90"
                onClick={() => props.onRetry()}
              >
                Retry
              </button>
            </DialogFooter>
          </Match>

          {/* ── Update available / downloading / installed ────────── */}
          <Match when={props.checkState === 'update-available' || props.checkState === 'idle'}>
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

              <Show when={downloadError()}>
                <div class="rounded border border-destructive/50 bg-destructive/10 p-3 text-xs text-destructive">
                  Update failed: {downloadError()}
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
          </Match>
        </Switch>
      </DialogContent>
    </Dialog>
  );
}
