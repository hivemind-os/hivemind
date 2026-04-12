import { For, Show, createSignal } from 'solid-js';
import type { Accessor, Setter } from 'solid-js';
import { FolderOpen, Folder, FileText } from 'lucide-solid';
import type { WorkspaceStore } from '../stores/workspaceStore';

export interface FileTreeNodeProps {
  entry: any;
  depth: number;
  workspace: WorkspaceStore;
}

const FileTreeNode = (props: FileTreeNodeProps) => {
  const [expanded, setExpanded] = createSignal(false);
  const isDropTarget = () => props.entry.is_dir && props.workspace.dragOverPath() === props.entry.path;

  return (
    <div>
      <div
        class={`tree-node ${props.entry.is_dir ? 'dir' : 'file'} ${props.workspace.selectedEntryPath() === props.entry.path ? 'selected' : ''} ${isDropTarget() ? 'drop-target' : ''}`}
        style={`padding-left: ${props.depth * 16 + 8}px`}
        draggable="true"
        onDragStart={(e) => {
          e.dataTransfer?.setData('text/plain', props.entry.path);
          e.dataTransfer!.effectAllowed = 'move';
        }}
        onDragOver={(e) => {
          if (props.entry.is_dir) {
            e.preventDefault();
            e.stopPropagation();
            e.dataTransfer!.dropEffect = 'move';
            props.workspace.setDragOverPath(props.entry.path);
          }
        }}
        onDragLeave={(e) => {
          e.stopPropagation();
          if (props.workspace.dragOverPath() === props.entry.path) props.workspace.setDragOverPath(null);
        }}
        onDrop={(e) => {
          e.preventDefault();
          e.stopPropagation();
          props.workspace.setDragOverPath(null);
          const fromPath = e.dataTransfer?.getData('text/plain');
          if (fromPath && props.entry.is_dir && fromPath !== props.entry.path) {
            void props.workspace.moveEntry(fromPath, props.entry.path);
          }
        }}
        onClick={() => {
          props.workspace.setSelectedEntryPath(props.entry.path);
          if (props.entry.is_dir) {
            setExpanded(!expanded());
          } else {
            void props.workspace.openWorkspaceFile(props.entry.path);
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          props.workspace.setSelectedEntryPath(props.entry.path);
          props.workspace.setContextMenu({ x: e.clientX, y: e.clientY, entry: props.entry });
        }}
      >
        <span class="tree-icon">
          {props.entry.is_dir ? (expanded() ? <FolderOpen size={14} /> : <Folder size={14} />) : <FileText size={14} />}
        </span>
        <span class="tree-name">{props.entry.name}</span>
        {!props.entry.is_dir && (() => {
          const status = props.workspace.indexStatus()[props.entry.path];
          return status ? (
            <span
              class={`index-dot ${status}`}
              title={status === 'indexed' ? 'Indexed' : 'Queued for indexing'}
            />
          ) : null;
        })()}
        {!props.entry.is_dir && props.entry.audit_status && props.entry.audit_status !== 'unaudited' && (
          <span class={`tree-audit-icon ${props.entry.audit_status}`} title={`Audit: ${props.entry.audit_status}`}>
            {props.workspace.auditIcon(props.entry.audit_status)}
          </span>
        )}
        {props.entry.effective_classification && (
          <span
            class={`tree-classification-badge ${props.entry.has_classification_override ? 'override' : 'inherited'} ${props.workspace.classificationColors[props.entry.effective_classification] ?? 'text-foreground'}`}
            title={`${props.entry.effective_classification}${props.entry.has_classification_override ? ' (override)' : ' (inherited)'}`}
          >
            {props.entry.effective_classification.charAt(0)}
          </span>
        )}
        {!props.entry.is_dir && props.entry.size != null && (
          <span class="tree-size">{props.workspace.formatFileSize(props.entry.size)}</span>
        )}
      </div>
      <Show when={props.entry.is_dir && expanded() && props.entry.children}>
        <For each={props.entry.children}>
          {(child: any) => <FileTreeNode entry={child} depth={props.depth + 1} workspace={props.workspace} />}
        </For>
      </Show>
    </div>
  );
};

export default FileTreeNode;
