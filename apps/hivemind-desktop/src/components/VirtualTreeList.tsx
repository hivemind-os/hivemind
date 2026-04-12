import { For, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import { FolderOpen, Folder, FileText, Loader2 } from 'lucide-solid';
import type { WorkspaceStore } from '../stores/workspaceStore';

const ROW_HEIGHT = 28;
const BUFFER_ROWS = 10;

export interface FlatTreeRow {
  entry: any;
  depth: number;
}

export interface VirtualTreeListProps {
  files: any[];
  workspace: WorkspaceStore;
}

/**
 * Virtualized file tree that flattens the recursive tree structure into
 * a flat list and only renders visible rows within the scroll viewport.
 * Directory children are loaded lazily on first expand.
 */
const VirtualTreeList = (props: VirtualTreeListProps) => {
  const ws = props.workspace;
  const [expandedPaths, setExpandedPaths] = createSignal<Set<string>>(new Set());
  const [loadingPaths, setLoadingPaths] = createSignal<Set<string>>(new Set());
  const [scrollTop, setScrollTop] = createSignal(0);
  const [containerHeight, setContainerHeight] = createSignal(400);
  let containerRef: HTMLDivElement | undefined;

  const toggleExpanded = async (path: string, entry: any) => {
    const isExpanded = expandedPaths().has(path);
    if (isExpanded) {
      setExpandedPaths((prev) => {
        const next = new Set(prev);
        next.delete(path);
        return next;
      });
    } else {
      // Load children lazily if not yet loaded
      if (entry.is_dir && !entry.children) {
        setLoadingPaths((prev) => { const next = new Set(prev); next.add(path); return next; });
        await ws.loadDirectoryChildren(path);
        setLoadingPaths((prev) => { const next = new Set(prev); next.delete(path); return next; });
      }
      setExpandedPaths((prev) => {
        const next = new Set(prev);
        next.add(path);
        return next;
      });
    }
  };

  // Flatten tree into visible rows based on expanded state
  const flatRows = createMemo<FlatTreeRow[]>(() => {
    const rows: FlatTreeRow[] = [];
    const expanded = expandedPaths();

    const walk = (entries: any[], depth: number) => {
      for (const entry of entries) {
        rows.push({ entry, depth });
        if (entry.is_dir && expanded.has(entry.path) && entry.children) {
          walk(entry.children, depth + 1);
        }
      }
    };

    walk(props.files, 0);
    return rows;
  });

  // Visible row range
  const visibleRange = createMemo(() => {
    const top = scrollTop();
    const height = containerHeight();
    const startIdx = Math.max(0, Math.floor(top / ROW_HEIGHT) - BUFFER_ROWS);
    const endIdx = Math.min(flatRows().length, Math.ceil((top + height) / ROW_HEIGHT) + BUFFER_ROWS);
    return { startIdx, endIdx };
  });

  const visibleRows = createMemo(() => {
    const { startIdx, endIdx } = visibleRange();
    return flatRows().slice(startIdx, endIdx).map((row, i) => ({
      ...row,
      index: startIdx + i,
    }));
  });

  const totalHeight = createMemo(() => flatRows().length * ROW_HEIGHT);

  const onScroll = () => {
    if (containerRef) setScrollTop(containerRef.scrollTop);
  };

  onMount(() => {
    if (containerRef) {
      setContainerHeight(containerRef.clientHeight);
      const observer = new ResizeObserver((entries) => {
        for (const e of entries) setContainerHeight(e.contentRect.height);
      });
      observer.observe(containerRef);
      onCleanup(() => observer.disconnect());
    }
  });

  return (
    <div
      ref={containerRef}
      class="virtual-tree-list"
      onScroll={onScroll}
    >
      <div style={`height:${totalHeight()}px;position:relative`}>
        <div style={`position:absolute;top:${visibleRange().startIdx * ROW_HEIGHT}px;left:0;right:0`}>
          <For each={visibleRows()}>
            {(row) => {
              const entry = row.entry;
              const expanded = () => expandedPaths().has(entry.path);
              const loading = () => loadingPaths().has(entry.path);
              const isDropTarget = () => entry.is_dir && ws.dragOverPath() === entry.path;

              return (
                <div
                  class={`tree-node ${entry.is_dir ? 'dir' : 'file'} ${ws.selectedEntryPath() === entry.path ? 'selected' : ''} ${isDropTarget() ? 'drop-target' : ''}`}
                  style={`padding-left:${row.depth * 16 + 8}px;height:${ROW_HEIGHT}px;box-sizing:border-box`}
                  draggable="true"
                  onDragStart={(e) => {
                    e.dataTransfer?.setData('text/plain', entry.path);
                    e.dataTransfer!.effectAllowed = 'move';
                  }}
                  onDragOver={(e) => {
                    if (entry.is_dir) {
                      e.preventDefault();
                      e.stopPropagation();
                      e.dataTransfer!.dropEffect = 'move';
                      ws.setDragOverPath(entry.path);
                    }
                  }}
                  onDragLeave={(e) => {
                    e.stopPropagation();
                    if (ws.dragOverPath() === entry.path) ws.setDragOverPath(null);
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    ws.setDragOverPath(null);
                    const fromPath = e.dataTransfer?.getData('text/plain');
                    if (fromPath && entry.is_dir && fromPath !== entry.path) {
                      void ws.moveEntry(fromPath, entry.path);
                    }
                  }}
                  onClick={() => {
                    ws.setSelectedEntryPath(entry.path);
                    if (entry.is_dir) {
                      void toggleExpanded(entry.path, entry);
                    } else {
                      void ws.openWorkspaceFile(entry.path);
                    }
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    ws.setSelectedEntryPath(entry.path);
                    ws.setContextMenu({ x: e.clientX, y: e.clientY, entry });
                  }}
                >
                  <span class="tree-icon">
                    {entry.is_dir
                      ? (loading()
                          ? <Loader2 size={14} class="animate-spin" />
                          : expanded() ? <FolderOpen size={14} /> : <Folder size={14} />)
                      : <FileText size={14} />}
                  </span>
                  <span class="tree-name">{entry.name}</span>
                  {!entry.is_dir && (() => {
                    const status = ws.indexStatus()[entry.path];
                    return status ? (
                      <span
                        class={`index-dot ${status}`}
                        title={status === 'indexed' ? 'Indexed' : 'Queued for indexing'}
                      />
                    ) : null;
                  })()}
                  {!entry.is_dir && entry.audit_status && entry.audit_status !== 'unaudited' && (
                    <span class={`tree-audit-icon ${entry.audit_status}`} title={`Audit: ${entry.audit_status}`}>
                      {ws.auditIcon(entry.audit_status)}
                    </span>
                  )}
                  {entry.effective_classification && (
                    <span
                      class={`tree-classification-badge ${entry.has_classification_override ? 'override' : 'inherited'} ${ws.classificationColors[entry.effective_classification] ?? 'text-foreground'}`}
                      title={`${entry.effective_classification}${entry.has_classification_override ? ' (override)' : ' (inherited)'}`}
                    >
                      {entry.effective_classification.charAt(0)}
                    </span>
                  )}
                  {!entry.is_dir && entry.size != null && (
                    <span class="tree-size">{ws.formatFileSize(entry.size)}</span>
                  )}
                </div>
              );
            }}
          </For>
        </div>
      </div>
    </div>
  );
};

export default VirtualTreeList;
