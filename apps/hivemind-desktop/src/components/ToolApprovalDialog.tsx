import { type Accessor, createSignal } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { TriangleAlert } from 'lucide-solid';
import { YamlBlock } from './YamlHighlight';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, Button } from '~/ui';

export interface ToolApprovalDialogProps {
  approval: Accessor<{ request_id: string; tool_id: string; input: string; reason: string } | null>;
  selectedSessionId: Accessor<string | null>;
  onDismiss: () => void;
}

const ToolApprovalDialog = (props: ToolApprovalDialogProps) => {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const respond = async (approved: boolean, allow_session?: boolean) => {
    const current = props.approval();
    if (!current || busy()) return;
    setBusy(true);
    setError(null);
    try {
      await invoke('chat_approve_tool', {
        session_id: props.selectedSessionId(),
        request_id: current.request_id,
        approved,
        ...(allow_session ? { allow_session: true } : {}),
      });
      props.onDismiss();
    } catch (err: any) {
      console.error(err);
      setError(String(err?.message ?? err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={!!props.approval()}
      onOpenChange={() => {}}
    >
      <DialogContent
        class="max-w-[520px] overflow-x-hidden"
        onInteractOutside={(e: Event) => e.preventDefault()}
        onEscapeKeyDown={(e: KeyboardEvent) => e.preventDefault()}
        data-testid="tool-approval-dialog"
      >
        <DialogHeader>
          <DialogTitle class="flex items-center gap-2">
            <TriangleAlert size={24} class="text-yellow-400" />
            Tool Approval Required
          </DialogTitle>
        </DialogHeader>

        <p class="text-sm text-muted-foreground">{props.approval()?.reason}</p>

        <dl class="message-details min-w-0 overflow-hidden">
          <div class="min-w-0"><dt>Tool</dt><dd><code>{props.approval()?.tool_id}</code></dd></div>
          <div class="min-w-0"><dt>Input</dt><dd class="min-w-0 overflow-hidden"><YamlBlock data={props.approval()?.input ?? ''} style="font-size:0.8em;max-height:120px;" /></dd></div>
        </dl>

        {error() && (
          <p class="text-sm text-red-400">{error()}</p>
        )}

        <DialogFooter class="gap-2">
          <Button variant="outline" size="sm" disabled={busy()} onClick={() => void respond(false, true)}>Deny for Session</Button>
          <Button variant="secondary" size="sm" disabled={busy()} onClick={() => void respond(false)}>Deny</Button>
          <Button size="sm" disabled={busy()} onClick={() => void respond(true)}>Approve</Button>
          <Button size="sm" disabled={busy()} onClick={() => void respond(true, true)}>Allow for Session</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default ToolApprovalDialog;
