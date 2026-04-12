import { createMemo, type Accessor, type Setter } from 'solid-js';
import PermissionRulesEditor, { type PermissionRule, type ToolDef } from './PermissionRulesEditor';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, Button } from '~/ui';

type SessionPermissions = { rules: PermissionRule[] };

export interface SessionPermsDialogProps {
  open: boolean;
  sessionPerms: Accessor<SessionPermissions>;
  setSessionPerms: Setter<SessionPermissions>;
  saveSessionPerms: () => Promise<void>;
  onClose: () => void;
  toolDefinitions?: ToolDef[];
}

const SessionPermsDialog = (props: SessionPermsDialogProps) => {
  const rules = createMemo(() => props.sessionPerms().rules);

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => { if (!open) props.onClose(); }}
    >
      <DialogContent class="min-w-[500px] max-w-[700px] max-h-[80vh] overflow-y-auto" data-testid="session-perms-dialog">
        <DialogHeader>
          <DialogTitle>Session Permissions</DialogTitle>
        </DialogHeader>

        <PermissionRulesEditor
          rules={rules}
          setRules={(newRules) => props.setSessionPerms({ rules: newRules })}
          toolDefinitions={props.toolDefinitions}
        />

        <DialogFooter class="gap-2 sm:gap-2">
          <Button variant="outline" onClick={props.onClose}>Cancel</Button>
          <Button onClick={async () => {
            await props.saveSessionPerms();
            props.onClose();
          }}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default SessionPermsDialog;
