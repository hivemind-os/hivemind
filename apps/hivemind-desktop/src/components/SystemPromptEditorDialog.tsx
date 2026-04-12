import { createSignal, createEffect, on } from 'solid-js';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button } from '~/ui/button';
import CodeMirrorEditor from './CodeMirrorEditor';

export interface SystemPromptEditorDialogProps {
  open: boolean;
  value: string;
  onSave: (value: string) => void;
  onCancel: () => void;
}

/**
 * Near-full-screen dialog with a CodeMirror markdown editor for editing
 * a persona's system prompt. Supports find/replace (Ctrl+F), markdown
 * syntax highlighting, code folding, and line numbers.
 */
const SystemPromptEditorDialog = (props: SystemPromptEditorDialogProps) => {
  const [draft, setDraft] = createSignal('');

  // Sync draft when the dialog opens (controlled open prop)
  createEffect(on(() => props.open, (isOpen) => {
    if (isOpen) {
      setDraft(props.value);
    }
  }));

  const handleOpenChange = (isOpen: boolean) => {
    if (!isOpen) {
      props.onCancel();
    }
  };

  const handleSave = () => {
    props.onSave(draft());
  };

  return (
    <Dialog open={props.open} onOpenChange={handleOpenChange}>
      <DialogContent
        class="w-[90vw] max-w-[1200px] p-0 gap-0 overflow-hidden"
        style="height:85vh;max-height:85vh;display:flex;flex-direction:column"
      >
        <DialogHeader class="px-5 py-3" style="flex-shrink:0;border-bottom:1px solid hsl(214 14% 22%)">
          <DialogTitle>Edit System Prompt</DialogTitle>
          <p class="text-sm" style="color:hsl(212 10% 53%);margin-top:2px">
            Markdown supported · Ctrl+F to search · Click gutter arrows to fold sections
          </p>
        </DialogHeader>

        <div style="flex:1;min-height:0;overflow:hidden">
          <CodeMirrorEditor
            value={draft()}
            onChange={setDraft}
            placeholder="Instructions that define this persona's behavior, personality, and capabilities.

Example: You are a senior code reviewer. Focus on security issues and performance problems. Always explain your reasoning."
          />
        </div>

        <DialogFooter class="px-5 py-3" style="flex-shrink:0;border-top:1px solid hsl(214 14% 22%)">
          <span class="text-sm" style="color:hsl(212 10% 53%);margin-right:auto;align-self:center">
            {draft().length} characters
          </span>
          <Button variant="outline" onClick={props.onCancel}>Cancel</Button>
          <Button onClick={handleSave}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default SystemPromptEditorDialog;
