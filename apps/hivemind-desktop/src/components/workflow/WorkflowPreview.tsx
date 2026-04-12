import { For, Show } from 'solid-js';
import { open } from '@tauri-apps/plugin-dialog';
import type { WorkflowAttachment } from './types';

// ── Types ──────────────────────────────────────────────────────────────

export interface AttachmentsPanelProps {
  attachments: WorkflowAttachment[];
  readOnly?: boolean;
  wfId: string;
  wfVersion: string;
  onClose: () => void;
  onUpdateDescription: (idx: number, description: string) => void;
  onDelete: (attachmentId: string) => Promise<void>;
  onUpload: (filePath: string, description: string) => Promise<WorkflowAttachment | undefined>;
  onPushHistory: () => void;
}

// ── Component ──────────────────────────────────────────────────────────

export function AttachmentsPanel(props: AttachmentsPanelProps) {
  return (
    <div style={{
      position: 'absolute', top: '10px', right: '10px', width: '400px',
      background: 'hsl(var(--background))', border: '1px solid hsl(var(--border))',
      'border-radius': '8px', padding: '16px', 'z-index': '100',
      'box-shadow': '0 4px 16px hsl(var(--foreground) / 0.12)', 'max-height': '60vh', 'overflow-y': 'auto',
    }}>
      <div style={{ display: 'flex', 'justify-content': 'space-between', 'align-items': 'center', 'margin-bottom': '12px' }}>
        <span style={{ 'font-weight': '600', 'font-size': '0.9em' }}>Workflow Attachments</span>
        <button style={{ background: 'none', border: 'none', color: 'hsl(var(--foreground))', cursor: 'pointer', 'font-size': '1.2em' }} onClick={() => props.onClose()}>✕</button>
      </div>
      <For each={props.attachments}>
        {(att, idx) => (
          <div style={{ display: 'flex', 'align-items': 'flex-start', gap: '8px', padding: '8px', background: 'hsl(var(--card))', 'border-radius': '6px', 'margin-bottom': '8px' }}>
            <div style={{ flex: '1', 'min-width': '0' }}>
              <div style={{ 'font-weight': '500', 'font-size': '0.8em', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }}>{att.filename}</div>
              <input
                style={{ width: '100%', 'font-size': '0.7em', padding: '4px 6px', 'margin-top': '4px', background: 'hsl(var(--card))', color: 'hsl(var(--foreground))', border: '1px solid hsl(var(--border))', 'border-radius': '4px' }}
                value={att.description}
                placeholder="Describe how an AI agent should use this file..."
                onInput={(e) => {
                  props.onUpdateDescription(idx(), e.currentTarget.value);
                }}
                onBlur={() => props.onPushHistory()}
                disabled={props.readOnly}
              />
              {att.size_bytes != null && <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px' }}>{(att.size_bytes / 1024).toFixed(1)} KB</div>}
            </div>
            <Show when={!props.readOnly}>
              <button
                style={{ background: 'none', border: 'none', color: 'hsl(var(--destructive))', cursor: 'pointer', 'font-size': '0.8em', padding: '2px 4px' }}
                onClick={async () => {
                  try {
                    await props.onDelete(att.id);
                  } catch (e) { /* ignore cleanup errors */ }
                  props.onPushHistory();
                }}
                title="Remove attachment"
              >✕</button>
            </Show>
          </div>
        )}
      </For>
      <Show when={!props.readOnly}>
        <button
          style={{ width: '100%', padding: '8px', 'font-size': '0.8em', background: 'hsl(var(--primary))', color: 'hsl(var(--background))', border: 'none', 'border-radius': '6px', cursor: 'pointer', 'font-weight': '500' }}
          onClick={async () => {
            const filePath = await open({ multiple: false, title: 'Select file to attach' });
            if (!filePath || Array.isArray(filePath)) return;
            const description = prompt('Describe how an AI agent should use this file:') ?? '';
            try {
              const att = await props.onUpload(filePath, description);
              if (att) {
                props.onPushHistory();
              }
            } catch (e: any) {
              console.error('Failed to upload attachment:', e);
            }
          }}
        >
          + Add Attachment
        </button>
      </Show>
    </div>
  );
}
