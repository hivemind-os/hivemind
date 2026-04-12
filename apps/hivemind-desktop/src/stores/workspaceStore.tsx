import { createSignal, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ShieldCheck, TriangleAlert, RefreshCw } from 'lucide-solid';
import { formatBytes, joinPath } from '~/utils';

export interface WorkspaceStoreDeps {
  selectedSessionId: Accessor<string | null>;
  currentWorkspacePath?: Accessor<string | null>;
}

export function createWorkspaceStore(deps: WorkspaceStoreDeps) {
  const { selectedSessionId } = deps;

  const [workspaceFiles, setWorkspaceFiles] = createSignal<any[]>([]);
  const [selectedEntryPath, setSelectedEntryPath] = createSignal<string | null>(null);
  const [selectedFilePath, setSelectedFilePath] = createSignal<string | null>(null);
  const [fileContent, setFileContent] = createSignal<any | null>(null);
  const [fileEditorContent, setFileEditorContent] = createSignal('');
  const [fileSaving, setFileSaving] = createSignal(false);
  const [workspaceLoading, setWorkspaceLoading] = createSignal(false);

  // Tracks which directories have had their children loaded
  const [loadedDirs, setLoadedDirs] = createSignal<Set<string>>(new Set());

  // Context menu & audit state
  const [contextMenu, setContextMenu] = createSignal<{
    x: number; y: number; entry: any;
  } | null>(null);
  const [auditTarget, setAuditTarget] = createSignal<any>(null);
  const [auditModel, setAuditModel] = createSignal('');
  const [auditRunning, setAuditRunning] = createSignal(false);
  const [auditResult, setAuditResult] = createSignal<any>(null);
  const [auditStatus, setAuditStatus] = createSignal('');
  const [newFolderParent, setNewFolderParent] = createSignal<string | null>(null);
  const [newFolderName, setNewFolderName] = createSignal('');
  const [newFileParent, setNewFileParent] = createSignal<string | null>(null);
  const [newFileName, setNewFileName] = createSignal('');
  const [dragOverPath, setDragOverPath] = createSignal<string | null>(null);

  // Index status: tracks which files are queued / indexed
  const [indexStatus, setIndexStatus] = createSignal<Record<string, 'queued' | 'indexed'>>({});
  let indexUnlisten: UnlistenFn | null = null;

  // Paste progress: tracks file copy progress during clipboard paste
  const [pasteProgress, setPasteProgress] = createSignal<{
    current: number; total: number; fileName?: string;
  } | null>(null);
  const [pasteConflict, setPasteConflict] = createSignal<{
    fileName: string; destination: string;
  } | null>(null);
  let pasteUnlisten: UnlistenFn | null = null;
  let pasteConflictUnlisten: UnlistenFn | null = null;

  const classificationColors: Record<string, string> = {
    PUBLIC: 'text-emerald-400',
    INTERNAL: 'text-primary',
    CONFIDENTIAL: 'text-orange-400',
    RESTRICTED: 'text-destructive',
  };

  const auditIcon = (status?: string) => {
    switch (status) {
      case 'safe': return <ShieldCheck size={14} />;
      case 'risky': return <TriangleAlert size={14} />;
      case 'stale': return <RefreshCw size={14} />;
      default: return null;
    }
  };

  const formatFileSize = (bytes: number): string => formatBytes(bytes);

  const resolveWorkspacePath = async () => {
    const currentPath = deps.currentWorkspacePath?.();
    if (currentPath) return currentPath;
    const session_id = selectedSessionId();
    if (!session_id) return null;
    try {
      const snapshot = await invoke<any>('chat_get_session', { session_id });
      return snapshot?.workspace_path ?? null;
    } catch (error) {
      console.error('Failed to resolve workspace path:', error);
      return null;
    }
  };

  const loadWorkspaceFiles = async () => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    setWorkspaceLoading(true);
    try {
      const files = await invoke<any[]>('workspace_list_files', { session_id });
      setWorkspaceFiles(files);
      setLoadedDirs(new Set<string>());
    } catch (error) {
      console.error('Failed to load workspace files:', error);
      setWorkspaceFiles([]);
      setLoadedDirs(new Set<string>());
    } finally {
      setWorkspaceLoading(false);
    }
  };

  /** Lazily load the children of a directory and merge them into the tree. */
  const loadDirectoryChildren = async (dirPath: string): Promise<void> => {
    if (loadedDirs().has(dirPath)) return;
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      const children = await invoke<any[]>('workspace_list_files', { session_id, path: dirPath });
      // Merge children into the tree by updating the matching directory entry
      setWorkspaceFiles((prev) => mergeChildrenIntoTree(prev, dirPath, children));
      setLoadedDirs((prev) => { const next = new Set(prev); next.add(dirPath); return next; });
    } catch (error) {
      console.error(`Failed to load directory children for ${dirPath}:`, error);
    }
  };

  const openWorkspaceFile = async (filePath: string) => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    setSelectedEntryPath(filePath);
    setSelectedFilePath(filePath);
    setFileContent(null);
    setFileEditorContent('');
    try {
      const content = await invoke<any>('workspace_read_file', { session_id, path: filePath });
      setFileContent(content);
      if (!content.is_binary) {
        setFileEditorContent(content.content);
      }
    } catch (error) {
      console.error('Failed to read file:', error);
    }
  };

  const saveWorkspaceFile = async () => {
    const session_id = selectedSessionId();
    const filePath = selectedFilePath();
    if (!session_id || !filePath) return;
    setFileSaving(true);
    try {
      await invoke('workspace_save_file', { session_id, path: filePath, content: fileEditorContent() });
      await openWorkspaceFile(filePath);
    } catch (error) {
      console.error('Failed to save file:', error);
    } finally {
      setFileSaving(false);
    }
  };

  const runFileAudit = async () => {
    const target = auditTarget();
    if (!target || !auditModel()) return;
    setAuditRunning(true);
    setAuditStatus('Running security audit...');
    setAuditResult(null);
    try {
      const result = await invoke<any>('workspace_audit_file', {
        session_id: selectedSessionId(),
        path: target.path,
        model: auditModel(),
      });
      setAuditResult(result);
      setAuditStatus('Audit complete.');
      void loadWorkspaceFiles();
    } catch (e: any) {
      setAuditStatus(`Audit failed: ${e?.toString()}`);
    } finally {
      setAuditRunning(false);
    }
  };

  const setClassification = async (path: string, data_class: string) => {
    try {
      await invoke('workspace_set_classification_override', {
        session_id: selectedSessionId(),
        path,
        class: data_class,
      });
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to set classification:', e);
    }
    setContextMenu(null);
  };

  const clearClassification = async (path: string) => {
    try {
      await invoke('workspace_clear_classification_override', {
        session_id: selectedSessionId(),
        path,
      });
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to clear classification:', e);
    }
    setContextMenu(null);
  };

  const createNewFolder = async () => {
    const parent = newFolderParent();
    const name = newFolderName().trim();
    if (!parent || !name || !selectedSessionId()) return;
    const folderPath = parent === '.' ? name : joinPath(parent, name);
    try {
      await invoke('workspace_create_directory', { session_id: selectedSessionId(), path: folderPath });
      // Invalidate parent directory cache so it gets re-fetched
      invalidateDir(parent === '.' ? null : parent);
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to create folder:', e);
    }
    setNewFolderParent(null);
    setNewFolderName('');
  };

  const createNewFile = async () => {
    const parent = newFileParent();
    const name = newFileName().trim();
    if (!parent || !name || !selectedSessionId()) return;
    const filePath = parent === '.' ? name : joinPath(parent, name);
    try {
      await invoke('workspace_save_file', { session_id: selectedSessionId(), path: filePath, content: '' });
      invalidateDir(parent === '.' ? null : parent);
      void loadWorkspaceFiles();
      void openWorkspaceFile(filePath);
    } catch (e) {
      console.error('Failed to create file:', e);
    }
    setNewFileParent(null);
    setNewFileName('');
  };

  const deleteEntry = async (path: string) => {
    if (!selectedSessionId()) return;
    try {
      await invoke('workspace_delete_entry', { session_id: selectedSessionId(), path });
      if (selectedEntryPath() === path) {
        setSelectedEntryPath(null);
      }
      if (selectedFilePath() === path) {
        setSelectedFilePath(null);
        setFileContent(null);
      }
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to delete:', e);
    }
    setContextMenu(null);
  };

  const moveEntry = async (fromPath: string, toDir: string) => {
    if (!selectedSessionId()) return;
    const fileName = fromPath.split('/').pop() || fromPath;
    const toPath = toDir === '.' ? fileName : joinPath(toDir, fileName);
    if (fromPath === toPath) return;
    try {
      await invoke('workspace_move_entry', { session_id: selectedSessionId(), from: fromPath, to: toPath });
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to move:', e);
    }
  };

  const copyToClipboard = async (paths: string[]) => {
    const workspace_path = await resolveWorkspacePath();
    if (!workspace_path || paths.length === 0) return;
    const absolutePaths = paths.map((path) => (path === '.' ? workspace_path: joinPath(workspace_path, path)));
    try {
      await invoke('clipboard_copy_files', { paths: absolutePaths });
    } catch (error) {
      console.error('Failed to copy to clipboard:', error);
    }
  };

  const pasteFromClipboard = async (targetDir = '.') => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      const sourcePaths = await invoke<string[]>('clipboard_read_file_paths');
      if (sourcePaths.length === 0) return;

      if (pasteUnlisten) { pasteUnlisten(); pasteUnlisten = null; }
      if (pasteConflictUnlisten) { pasteConflictUnlisten(); pasteConflictUnlisten = null; }
      setPasteProgress({ current: 0, total: sourcePaths.length });

      pasteUnlisten = await listen<{
        session_id: string; current: number; total: number; fileName?: string; done?: boolean; cancelled?: boolean;
      }>('paste:progress', (e) => {
        if (e.payload.session_id !== session_id) return;
        if (e.payload.done || e.payload.cancelled) {
          setPasteProgress(null);
        } else {
          setPasteProgress({
            current: e.payload.current,
            total: e.payload.total,
            fileName: e.payload.fileName,
          });
        }
      });

      pasteConflictUnlisten = await listen<{
        session_id: string; fileName: string; destination: string;
      }>('paste:conflict', (e) => {
        if (e.payload.session_id !== session_id) return;
        setPasteConflict({ fileName: e.payload.fileName, destination: e.payload.destination });
      });

      await invoke('clipboard_paste_files', { session_id, target_dir: targetDir, source_paths: sourcePaths });
      void loadWorkspaceFiles();
    } catch (error) {
      console.error('Failed to paste from clipboard:', error);
    } finally {
      setPasteProgress(null);
      setPasteConflict(null);
      if (pasteUnlisten) { pasteUnlisten(); pasteUnlisten = null; }
      if (pasteConflictUnlisten) { pasteConflictUnlisten(); pasteConflictUnlisten = null; }
    }
  };

  const cancelPaste = async () => {
    try {
      await invoke('clipboard_cancel_paste');
    } catch (_) { /* best-effort */ }
  };

  const resolveConflict = async (resolution: string) => {
    setPasteConflict(null);
    try {
      await invoke('clipboard_resolve_conflict', { resolution });
    } catch (_) { /* best-effort */ }
  };

  /** Invalidate a cached directory so its children are re-fetched on next expand. */
  const invalidateDir = (dirPath: string | null) => {
    const key = dirPath ?? '__root__';
    setLoadedDirs((prev) => {
      if (!prev.has(key)) return prev;
      const next = new Set(prev);
      next.delete(key);
      return next;
    });
  };

  const resetFileState = () => {
    setSelectedEntryPath(null);
    setSelectedFilePath(null);
    setFileContent(null);
    setFileEditorContent('');
    setWorkspaceFiles([]);
    setLoadedDirs(new Set<string>());
  };

  let indexDisposed = false;

  /** Subscribe to workspace index-status SSE for the current session. */
  const subscribeIndexStatus = async () => {
    // Clean up any previous subscription
    if (indexUnlisten) {
      indexUnlisten();
      indexUnlisten = null;
    }
    indexDisposed = false;

    const session_id = selectedSessionId();
    if (!session_id) {
      setIndexStatus({});
      return;
    }

    // Fetch initial snapshot of indexed files
    try {
      const files = await invoke<string[]>('workspace_indexed_files', { session_id });
      if (indexDisposed) return;
      const status: Record<string, 'queued' | 'indexed'> = {};
      for (const f of files) {
        status[f] = 'indexed';
      }
      setIndexStatus(status);
    } catch {
      setIndexStatus({});
    }

    if (indexDisposed) return;

    // Start SSE subscription via Tauri bridge
    try {
      await invoke('workspace_subscribe_index_status', { session_id });
    } catch {
      // Silently fail — the stream may not be available
    }

    if (indexDisposed) return;

    // Listen for emitted Tauri events
    try {
      const ul = await listen<{ session_id: string; event: any }>('index:event', (e) => {
        if (indexDisposed) return;
        if (e.payload.session_id !== selectedSessionId()) return;
        const ev = e.payload.event;
        const status = ev.status;
        const path = ev.path;
        if (!status || !path) return;

        setIndexStatus((prev) => {
          const next = { ...prev };
          if (status === 'removed') {
            delete next[path];
          } else if (status === 'queued') {
            next[path] = 'queued';
          } else if (status === 'indexed') {
            next[path] = 'indexed';
          }
          return next;
        });
      });

      if (indexDisposed) {
        ul();
        return;
      }

      indexUnlisten = ul;
    } catch (e) {
      console.warn('[workspaceStore] Failed to listen for index events:', e);
    }
  };

  /** Force reindex of a single file. */
  const reindexFile = async (path: string) => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      await invoke('workspace_reindex_file', { session_id, path });
    } catch (error) {
      console.error('Failed to reindex file:', error);
    }
  };

  /** Cleanup index status subscription. */
  const cleanupIndexStatus = () => {
    indexDisposed = true;
    if (indexUnlisten) {
      indexUnlisten();
      indexUnlisten = null;
    }
  };

  return {
    workspaceFiles,
    selectedEntryPath,
    setSelectedEntryPath,
    selectedFilePath,
    setSelectedFilePath,
    fileContent,
    fileEditorContent,
    setFileEditorContent,
    fileSaving,
    workspaceLoading,
    contextMenu, setContextMenu,
    auditTarget, setAuditTarget,
    auditModel, setAuditModel,
    auditRunning, auditResult, setAuditResult, auditStatus,
    newFolderParent, setNewFolderParent,
    newFolderName, setNewFolderName,
    newFileParent, setNewFileParent,
    newFileName, setNewFileName,
    dragOverPath, setDragOverPath,
    classificationColors, auditIcon, formatFileSize,
    loadWorkspaceFiles, loadDirectoryChildren, openWorkspaceFile, saveWorkspaceFile,
    runFileAudit, setClassification, clearClassification,
    createNewFolder, createNewFile, deleteEntry, moveEntry,
    copyToClipboard, pasteFromClipboard,
    pasteProgress, pasteConflict, cancelPaste, resolveConflict,
    resetFileState,
    indexStatus, subscribeIndexStatus, reindexFile, cleanupIndexStatus,
  };
}

/** Merge lazily-loaded children into the workspace tree for a given directory path. */
function mergeChildrenIntoTree(tree: any[], dirPath: string, children: any[]): any[] {
  return tree.map((entry: any) => {
    if (entry.path === dirPath && entry.is_dir) {
      return { ...entry, children };
    }
    if (entry.children) {
      return { ...entry, children: mergeChildrenIntoTree(entry.children, dirPath, children) };
    }
    return entry;
  });
}

export type WorkspaceStore = ReturnType<typeof createWorkspaceStore>;
