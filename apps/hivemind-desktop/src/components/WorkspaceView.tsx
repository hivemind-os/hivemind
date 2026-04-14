import { For, Show, Suspense, createEffect, createMemo, createSignal, lazy, onCleanup } from 'solid-js';
import { Portal } from 'solid-js/web';
import { FolderOpen, Link, RefreshCw, FileText, TriangleAlert, Search, ClipboardList, FolderPlus, Trash2, Save, Eye, Pencil, X, Sparkles, ArrowLeft } from 'lucide-solid';
import { invoke } from '@tauri-apps/api/core';
import type { Accessor, Setter } from 'solid-js';
import VirtualTreeList from './VirtualTreeList';
import CodeViewer from './CodeViewer';
import MarkdownViewer from './MarkdownViewer';
const StlViewer = lazy(() => import('./StlViewer'));
import HighlightedEditor from './HighlightedEditor';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '~/ui/dialog';
import { Button } from '~/ui/button';
import FindBar from './FindBar';
import type { WorkspaceStore } from '../stores/workspaceStore';
import type { ChatSessionSnapshot } from '../types';
import { languageFromPath, extensionFromPath } from '../lib/languageMap';
import { buildWorkspacePathSet } from '../lib/importLinks';
import { getThemeFamily } from '../stores/themeStore';

interface WorkspaceSearchResult {
  path: string;
  snippet: string;
  nodeId: number;
  distance?: number;
}

export interface WorkspaceViewProps {
  session: Accessor<ChatSessionSnapshot | null>;
  workspace: WorkspaceStore;
  availableModels: Accessor<{ id: string; label: string }[]>;
}

const WorkspaceView = (props: WorkspaceViewProps) => {
  const ws = props.workspace;
  const [treeWidth, setTreeWidth] = createSignal(240);
  const [editMode, setEditMode] = createSignal(false);
  const MIN_TREE = 120;
  const MAX_TREE = 600;

  // Reset edit mode when a different file is selected
  createEffect(() => {
    ws.selectedEntryPath();
    setEditMode(false);
  });

  // Derive whether the current file is markdown
  const isMarkdown = createMemo(() => {
    const content = ws.fileContent();
    if (!content || content.is_binary) return false;
    const ext = extensionFromPath(content.path);
    return ext === 'md' || ext === 'mdx';
  });

  // Derive the shiki language for the current file
  const currentLanguage = createMemo(() => {
    const content = ws.fileContent();
    if (!content) return undefined;
    return languageFromPath(content.path);
  });

  // Build workspace path set for import resolution
  const workspacePathSet = createMemo(() => buildWorkspacePathSet(ws.workspaceFiles()));

  // ── Find-in-preview state ─────────────────────────────────────────
  const [findOpen, setFindOpen] = createSignal(false);
  const [findQuery, setFindQuery] = createSignal('');
  const [findMatchCount, setFindMatchCount] = createSignal(0);
  const [currentFindMatch, setCurrentFindMatch] = createSignal(0);
  let findInputRef: HTMLInputElement | undefined;

  const openFind = () => {
    setFindOpen(true);
    setTimeout(() => findInputRef?.focus(), 0);
  };
  const closeFind = () => {
    setFindOpen(false);
    setFindQuery('');
    setFindMatchCount(0);
    setCurrentFindMatch(0);
  };
  const findNext = () => {
    const count = findMatchCount();
    if (count === 0) return;
    setCurrentFindMatch((currentFindMatch() + 1) % count);
  };
  const findPrev = () => {
    const count = findMatchCount();
    if (count === 0) return;
    setCurrentFindMatch((currentFindMatch() - 1 + count) % count);
  };
  const handleFindQueryChange = (q: string) => {
    setFindQuery(q);
    setCurrentFindMatch(0);
  };

  // Close find when switching files
  createEffect(() => {
    ws.selectedEntryPath();
    closeFind();
  });

  // ── Workspace search state ──────────────────────────────────────────
  const [treeSearchOpen, setTreeSearchOpen] = createSignal(false);
  const [treeSearchQuery, setTreeSearchQuery] = createSignal('');
  const [treeSearchMode, setTreeSearchMode] = createSignal<'keyword' | 'semantic'>('keyword');
  const [treeSearchResults, setTreeSearchResults] = createSignal<WorkspaceSearchResult[]>([]);
  const [treeSearchLoading, setTreeSearchLoading] = createSignal(false);
  let treeSearchInputRef: HTMLInputElement | undefined;
  let searchDebounceTimer: ReturnType<typeof setTimeout> | undefined;

  const openTreeSearch = () => {
    setTreeSearchOpen(true);
    setTimeout(() => treeSearchInputRef?.focus(), 0);
  };
  const closeTreeSearch = () => {
    setTreeSearchOpen(false);
    setTreeSearchQuery('');
    setTreeSearchResults([]);
    setTreeSearchLoading(false);
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
  };

  const executeTreeSearch = async (query: string) => {
    if (!query.trim()) {
      setTreeSearchResults([]);
      setTreeSearchLoading(false);
      return;
    }
    setTreeSearchLoading(true);
    try {
      let results: WorkspaceSearchResult[];
      if (treeSearchMode() === 'semantic') {
        results = await invoke<WorkspaceSearchResult[]>('workspace_semantic_search', {
          q: query.trim(),
          limit: 20,
        });
      } else {
        results = await invoke<WorkspaceSearchResult[]>('workspace_search_files', {
          q: query.trim(),
          limit: 20,
        });
      }
      // Only update if query hasn't changed while we were waiting
      if (treeSearchQuery() === query) {
        setTreeSearchResults(results);
      }
    } catch (err) {
      console.error('Workspace search failed:', err);
      setTreeSearchResults([]);
    } finally {
      setTreeSearchLoading(false);
    }
  };

  const handleTreeSearchInput = (q: string) => {
    setTreeSearchQuery(q);
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
    if (!q.trim()) {
      setTreeSearchResults([]);
      setTreeSearchLoading(false);
      return;
    }
    setTreeSearchLoading(true);
    searchDebounceTimer = setTimeout(() => void executeTreeSearch(q), 300);
  };

  const handleSearchResultClick = (result: WorkspaceSearchResult) => {
    ws.setSelectedEntryPath(result.path);
    void ws.openWorkspaceFile(result.path);
  };

  onCleanup(() => {
    if (searchDebounceTimer) clearTimeout(searchDebounceTimer);
  });

  // Handle navigation from import links / markdown links
  const handleNavigate = (targetPath: string) => {
    ws.setSelectedEntryPath(targetPath);
    void ws.openWorkspaceFile(targetPath);
  };

  const onSplitterPointerDown = (e: PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = treeWidth();
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    const onMove = (ev: PointerEvent) => {
      const newWidth = Math.min(MAX_TREE, Math.max(MIN_TREE, startWidth + ev.clientX - startX));
      setTreeWidth(newWidth);
    };
    const onUp = () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', onUp);
    };
    target.addEventListener('pointermove', onMove);
    target.addEventListener('pointerup', onUp);
  };

  // Build a path→entry Map once for O(1) lookups (replaces recursive findEntryByPath)
  const pathEntryMap = createMemo<Map<string, any>>(() => {
    const map = new Map<string, any>();
    const walk = (entries: any[]) => {
      for (const entry of entries) {
        map.set(entry.path, entry);
        if (entry.children?.length) walk(entry.children);
      }
    };
    walk(ws.workspaceFiles());
    return map;
  });

  const selectedEntry = () => {
    const path = ws.selectedEntryPath();
    return path ? pathEntryMap().get(path) ?? null : null;
  };

  const pasteTargetDir = () => (selectedEntry()?.is_dir ? selectedEntry()!.path : '.');

  const isEditableTarget = (target: EventTarget | null) =>
    target instanceof HTMLElement && (
      target.tagName === 'INPUT' ||
      target.tagName === 'TEXTAREA' ||
      target.tagName === 'SELECT' ||
      target.isContentEditable
    );

  return (
    <div class="workspace-browser">
      <div class="workspace-header">
        <span class="workspace-path-label">
          <FolderOpen size={14} /> {props.session()?.workspace_path ?? 'No workspace'}
          {props.session()?.workspace_linked && <> <Link size={14} /></>}
        </span>
        <Button
          variant="ghost"
          size="icon"
          data-testid="workspace-refresh-btn"
          aria-label="Refresh workspace"
          onClick={() => void ws.loadWorkspaceFiles()}
          title="Refresh file tree"
        >
          <RefreshCw size={14} />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          data-testid="workspace-new-folder-btn"
          aria-label="New folder"
          onClick={() => { ws.setNewFolderParent('.'); ws.setNewFolderName(''); }}
          title="New folder"
        >
          <FolderPlus size={14} />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          data-testid="workspace-new-file-btn"
          aria-label="New file"
          onClick={() => { ws.setNewFileParent('.'); ws.setNewFileName(''); }}
          title="New file"
        >
          <FileText size={14} />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          data-testid="workspace-tree-search-btn"
          aria-label="Search files"
          onClick={() => treeSearchOpen() ? closeTreeSearch() : openTreeSearch()}
          title="Search workspace files"
        >
          <Search size={14} />
        </Button>
      </div>

      <div class="workspace-body">
        {/* Paste progress dialog (centered modal) */}
        <Show when={ws.pasteProgress()}>
          {(progress) => (
            <Portal>
              <div class="paste-dialog-backdrop">
                <div class="paste-dialog">
                  <div class="paste-dialog-header">
                    <span class="paste-dialog-title">Pasting Files</span>
                  </div>
                  <div class="paste-dialog-body">
                    <div class="paste-dialog-status">
                      {progress().current} of {progress().total} files
                    </div>
                    <Show when={progress().fileName}>
                      <div class="paste-dialog-filename" title={progress().fileName}>
                        {progress().fileName}
                      </div>
                    </Show>
                    <div class="paste-progress-bar-track">
                      <div
                        class="paste-progress-bar-fill"
                        style={`width: ${Math.round((progress().current / Math.max(progress().total, 1)) * 100)}%`}
                      />
                    </div>
                  </div>

                  {/* Conflict sub-dialog */}
                  <Show when={ws.pasteConflict()}>
                    {(conflict) => (
                      <div class="paste-conflict-section">
                        <div class="paste-conflict-message">
                          <span class="paste-conflict-icon"><TriangleAlert size={16} /></span>
                          <span>
                            <strong>{conflict().fileName}</strong> already exists at{' '}
                            <code>{conflict().destination}</code>
                          </span>
                        </div>
                        <div class="paste-conflict-actions">
                          <button class="paste-btn paste-btn-primary" onClick={() => ws.resolveConflict('replace')}>
                            Replace
                          </button>
                          <button class="paste-btn" onClick={() => ws.resolveConflict('skip')}>
                            Skip
                          </button>
                          <button class="paste-btn paste-btn-primary" onClick={() => ws.resolveConflict('replace_all')}>
                            Replace All
                          </button>
                          <button class="paste-btn" onClick={() => ws.resolveConflict('skip_all')}>
                            Skip All
                          </button>
                        </div>
                      </div>
                    )}
                  </Show>

                  <div class="paste-dialog-footer">
                    <button class="paste-btn paste-btn-cancel" onClick={() => ws.cancelPaste()}>
                      Cancel
                    </button>
                  </div>
                </div>
              </div>
            </Portal>
          )}
        </Show>
        <div
          class={`workspace-tree ${ws.dragOverPath() === '.' ? 'drop-target' : ''}`}
          data-testid="workspace-tree"
          style={`width:${treeWidth()}px`}
          tabIndex={0}
          onMouseDown={(e) => e.currentTarget.focus()}
          onContextMenu={(e) => {
            // Only trigger for clicks on the tree background itself, not on child nodes
            if (e.target === e.currentTarget || (e.target as HTMLElement).classList?.contains('empty-copy')) {
              e.preventDefault();
              ws.setContextMenu({
                x: e.clientX,
                y: e.clientY,
                entry: { path: '.', name: 'Workspace', is_dir: true, _isRoot: true },
              });
            }
          }}
          onKeyDown={(e) => {
            if (isEditableTarget(e.target)) return;
            const shortcutPressed = e.metaKey || e.ctrlKey;
            if (!shortcutPressed) return;
            const key = e.key.toLowerCase();
            if (key === 'c') {
              // If user has text selected, allow default browser copy
              const sel = window.getSelection();
              if (sel && sel.toString().length > 0) return;
              const entry = selectedEntry();
              if (!entry) return;
              e.preventDefault();
              void ws.copyToClipboard([entry.path]);
            }
            if (key === 'v') {
              e.preventDefault();
              void ws.pasteFromClipboard(pasteTargetDir());
            }
            if (key === 'f') {
              e.preventDefault();
              openFind();
            }
          }}
          onDragOver={(e) => {
            e.preventDefault();
            e.dataTransfer!.dropEffect = 'move';
            ws.setDragOverPath('.');
          }}
          onDragLeave={(e) => {
            if (e.currentTarget === e.target && ws.dragOverPath() === '.') ws.setDragOverPath(null);
          }}
          onDrop={(e) => {
            e.preventDefault();
            ws.setDragOverPath(null);
            const fromPath = e.dataTransfer?.getData('text/plain');
            if (fromPath) void ws.moveEntry(fromPath, '.');
          }}
        >
          {/* Search bar (shown when search is open) */}
          <Show when={treeSearchOpen()}>
            <div class="workspace-tree-search">
              <div class="workspace-tree-search-bar">
                <Search size={14} class="workspace-tree-search-icon" />
                <input
                  type="text"
                  class="workspace-tree-search-input"
                  placeholder={treeSearchMode() === 'semantic' ? 'Describe what you\'re looking for...' : 'Search files by keyword...'}
                  value={treeSearchQuery()}
                  onInput={(e) => handleTreeSearchInput(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Escape') closeTreeSearch();
                  }}
                  ref={(el) => { treeSearchInputRef = el; }}
                />
                <Show when={treeSearchQuery()}>
                  <button class="workspace-tree-search-clear" onClick={() => handleTreeSearchInput('')} title="Clear">
                    <X size={12} />
                  </button>
                </Show>
              </div>
              <div class="workspace-tree-search-mode">
                <button
                  class={`workspace-search-mode-btn ${treeSearchMode() === 'keyword' ? 'active' : ''}`}
                  onClick={() => {
                    setTreeSearchMode('keyword');
                    if (treeSearchQuery().trim()) void executeTreeSearch(treeSearchQuery());
                  }}
                  title="Full-text keyword search"
                >
                  <Search size={12} /> Keyword
                </button>
                <button
                  class={`workspace-search-mode-btn ${treeSearchMode() === 'semantic' ? 'active' : ''}`}
                  onClick={() => {
                    setTreeSearchMode('semantic');
                    if (treeSearchQuery().trim()) void executeTreeSearch(treeSearchQuery());
                  }}
                  title="AI-powered semantic search"
                >
                  <Sparkles size={12} /> Semantic
                </button>
              </div>
            </div>
          </Show>

          {/* Search results (replaces file tree when searching) */}
          <Show when={treeSearchOpen() && treeSearchQuery().trim()}>
            <div class="workspace-search-results">
              <Show when={treeSearchLoading()}>
                <div class="workspace-search-status">
                  <span class="spinner" style="width:14px;height:14px;" /> Searching…
                </div>
              </Show>
              <Show when={!treeSearchLoading() && treeSearchResults().length === 0 && treeSearchQuery().trim()}>
                <p class="empty-copy">No results found</p>
              </Show>
              <For each={treeSearchResults()}>
                {(result) => {
                  const fileName = () => result.path.split('/').pop() || result.path;
                  const dirPath = () => {
                    const parts = result.path.split('/');
                    return parts.length > 1 ? parts.slice(0, -1).join('/') : '';
                  };
                  return (
                    <div
                      class={`workspace-search-result-item ${ws.selectedEntryPath() === result.path ? 'selected' : ''}`}
                      onClick={() => handleSearchResultClick(result)}
                      title={result.path}
                    >
                      <div class="workspace-search-result-path">
                        <FileText size={13} />
                        <span class="workspace-search-result-name">{fileName()}</span>
                        <Show when={dirPath()}>
                          <span class="workspace-search-result-dir">{dirPath()}</span>
                        </Show>
                      </div>
                      <Show when={result.snippet}>
                        <div class="workspace-search-result-snippet">{result.snippet}</div>
                      </Show>
                      <Show when={result.distance != null}>
                        <span class="workspace-search-result-score" title="Similarity score">
                          {(1 - result.distance!).toFixed(2)}
                        </span>
                      </Show>
                    </div>
                  );
                }}
              </For>
              <Show when={!treeSearchLoading() && treeSearchResults().length > 0}>
                <button class="workspace-search-back-btn" onClick={closeTreeSearch}>
                  <ArrowLeft size={12} /> Back to file tree
                </button>
              </Show>
            </div>
          </Show>

          {/* File tree (hidden when search results are showing) */}
          <Show when={!treeSearchOpen() || !treeSearchQuery().trim()}>
          <Show when={ws.newFolderParent() !== null}>
            <div class="new-folder-input" style={`padding-left: ${ws.newFolderParent() === '.' ? 8 : 24}px`}>
              <span class="tree-icon">📁</span>
              <input
                type="text"
                placeholder="Folder name"
                value={ws.newFolderName()}
                onInput={(e) => ws.setNewFolderName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void ws.createNewFolder();
                  if (e.key === 'Escape') { ws.setNewFolderParent(null); ws.setNewFolderName(''); }
                }}
                ref={(el) => setTimeout(() => el.focus(), 0)}
              />
              <button class="icon-btn" onClick={() => void ws.createNewFolder()} title="Create">✓</button>
              <button class="icon-btn" onClick={() => { ws.setNewFolderParent(null); ws.setNewFolderName(''); }} title="Cancel">✕</button>
            </div>
          </Show>
          <Show when={ws.newFileParent() !== null}>
            <div class="new-folder-input" style={`padding-left: ${ws.newFileParent() === '.' ? 8 : 24}px`}>
              <span class="tree-icon"><FileText size={14} /></span>
              <input
                type="text"
                placeholder="File name"
                value={ws.newFileName()}
                onInput={(e) => ws.setNewFileName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void ws.createNewFile();
                  if (e.key === 'Escape') { ws.setNewFileParent(null); ws.setNewFileName(''); }
                }}
                ref={(el) => setTimeout(() => el.focus(), 0)}
              />
              <button class="icon-btn" onClick={() => void ws.createNewFile()} title="Create">✓</button>
              <button class="icon-btn" onClick={() => { ws.setNewFileParent(null); ws.setNewFileName(''); }} title="Cancel">✕</button>
            </div>
          </Show>
          <Show
            when={!ws.workspaceLoading()}
            fallback={<p class="empty-copy">Loading...</p>}
          >
            <Show
              when={ws.workspaceFiles().length > 0}
              fallback={<p class="empty-copy">No files in workspace</p>}
            >
              <VirtualTreeList files={ws.workspaceFiles()} workspace={ws} />
            </Show>
          </Show>
          </Show>
        </div>

        <div class="workspace-splitter" onPointerDown={onSplitterPointerDown} />

        {/* Context menu overlay */}
        <Portal>
          <Show when={ws.contextMenu()}>
            {(menu) => (
              <div class="context-menu-backdrop" onClick={() => ws.setContextMenu(null)}>
                <div
                  class="context-menu"
                  style={`left:${menu().x}px;top:${menu().y}px;`}
                  onClick={(e) => e.stopPropagation()}
                >
                  <Show when={!menu().entry.is_dir && !menu().entry._isRoot}>
                    <button class="context-menu-item" onClick={() => { ws.setAuditTarget(menu().entry); ws.setAuditResult(null); ws.setAuditModel(''); ws.setContextMenu(null); }}>
                      <Search size={14} /> Security Audit...
                    </button>
                  </Show>
                  <Show when={!menu().entry._isRoot}>
                    <button class="context-menu-item" onClick={() => { void ws.copyToClipboard([menu().entry.path]); ws.setContextMenu(null); }}>
                      <ClipboardList size={14} /> Copy
                    </button>
                  </Show>
                  <Show when={menu().entry.is_dir}>
                    <button class="context-menu-item" onClick={() => { const dir = menu().entry._isRoot ? '.' : menu().entry.path; ws.setContextMenu(null); void ws.pasteFromClipboard(dir); }}>
                      <ClipboardList size={14} /> Paste
                    </button>
                  </Show>
                  <Show when={menu().entry.is_dir}>
                    <div class="context-menu-separator" />
                    <button class="context-menu-item" onClick={() => { ws.setNewFolderParent(menu().entry._isRoot ? '.' : menu().entry.path); ws.setNewFolderName(''); ws.setContextMenu(null); }}>
                      <FolderPlus size={14} /> New Folder…
                    </button>
                    <button class="context-menu-item" onClick={() => { ws.setNewFileParent(menu().entry._isRoot ? '.' : menu().entry.path); ws.setNewFileName(''); ws.setContextMenu(null); }}>
                      <FileText size={14} /> New File…
                    </button>
                  </Show>
                  <Show when={!menu().entry._isRoot}>
                    <div class="context-menu-separator" />
                    <div class="context-menu-label">Classification</div>
                    {['PUBLIC', 'INTERNAL', 'CONFIDENTIAL', 'RESTRICTED'].map((cls) => (
                      <button
                        class={`context-menu-item ${menu().entry.effective_classification === cls && menu().entry.has_classification_override ? 'active' : ''}`}
                        onClick={() => ws.setClassification(menu().entry.path, cls)}
                      >
                        <span class={ws.classificationColors[cls]}>●</span> {cls.charAt(0) + cls.slice(1).toLowerCase()}
                      </button>
                    ))}
                    <Show when={menu().entry.has_classification_override}>
                      <div class="context-menu-separator" />
                      <button class="context-menu-item" onClick={() => ws.clearClassification(menu().entry.path)}>
                        ↩ Clear Override
                      </button>
                    </Show>
                  </Show>
                  <Show when={!menu().entry._isRoot}>
                    <div class="context-menu-separator" />
                    <Show when={!menu().entry.is_dir}>
                      <button class="context-menu-item" onClick={() => {
                        const path = menu().entry.path;
                        ws.setContextMenu(null);
                        void ws.reindexFile(path);
                      }}>
                        <RefreshCw size={14} /> Reindex
                      </button>
                    </Show>
                    <button class="context-menu-item danger" onClick={() => {
                      const path = menu().entry.path;
                      ws.setContextMenu(null);
                      void ws.deleteEntry(path);
                    }}>
                      <Trash2 size={14} /> Delete
                    </button>
                  </Show>
                </div>
              </div>
            )}
          </Show>
        </Portal>

        {/* File audit dialog */}
        <Dialog
          open={!!ws.auditTarget()}
          onOpenChange={(open) => { if (!open && !ws.auditRunning()) { ws.setAuditTarget(null); ws.setAuditResult(null); } }}
        >
          <DialogContent
            class="max-w-[600px] w-[90vw] max-h-[80vh] overflow-y-auto"
            onInteractOutside={(e) => { if (ws.auditRunning()) e.preventDefault(); }}
            onFocusOutside={(e) => e.preventDefault()}
            onEscapeKeyDown={(e) => { if (ws.auditRunning()) e.preventDefault(); }}
          >
          <Show when={ws.auditTarget()}>
            <DialogHeader>
              <DialogTitle class="flex items-center gap-2">
                <Search size={24} />
                Security Audit — {ws.auditTarget()!.name}
              </DialogTitle>
            </DialogHeader>
            <p style="margin:0 0 16px;font-size:0.85em;color:hsl(var(--muted-foreground));">
              Path: <code>{ws.auditTarget()!.path}</code>
            </p>

            <Show when={!ws.auditResult()}>
              <div style="margin-bottom:16px;">
                <label style="font-size:0.85em;font-weight:500;display:block;margin-bottom:4px;">
                  Select model for security audit:
                </label>
                <select
                  ref={(el) => {
                    createEffect(() => { el.value = ws.auditModel(); });
                  }}
                  onInput={(e) => ws.setAuditModel(e.currentTarget.value)}
                  style="width:100%;"
                  disabled={ws.auditRunning()}
                >
                  <option value="">— Select a model —</option>
                  <For each={props.availableModels()}>
                    {(model) => <option value={model.id}>{model.label}</option>}
                  </For>
                </select>
              </div>

              <Show when={ws.auditRunning()}>
                <div style="display:flex;align-items:center;gap:8px;margin-bottom:12px;">
                  <span class="spinner" />
                  <span>{ws.auditStatus()}</span>
                </div>
              </Show>

              <div style="display:flex;gap:8px;justify-content:flex-end;">
                <Button variant="outline" onClick={() => { ws.setAuditTarget(null); ws.setAuditResult(null); }} disabled={ws.auditRunning()}>Cancel</Button>
                <Button onClick={() => void ws.runFileAudit()} disabled={!ws.auditModel() || ws.auditRunning()}>
                  <Search size={14} /> Start Audit
                </Button>
              </div>
            </Show>

            <Show when={ws.auditResult()}>
              <div style="margin-bottom:16px;">
                <h4 style="margin:0 0 8px;">Audit Results</h4>
                <p style="margin:0 0 12px;font-size:0.85em;color:hsl(var(--muted-foreground));">
                  {ws.auditResult()!.summary}
                </p>
                <Show when={ws.auditResult()!.risks?.length > 0}>
                  <div style="display:flex;flex-direction:column;gap:8px;">
                    <For each={ws.auditResult()!.risks}>
                      {(risk: any) => (
                        <div style={`padding:10px 12px;border-radius:6px;border-left:3px solid ${
                          risk.severity === 'critical' ? 'hsl(var(--destructive))' :
                          risk.severity === 'high' ? '#fb923c' :
                          risk.severity === 'medium' ? '#fbbf24' : '#34d399'
                        };background:hsl(var(--card));`}>
                          <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:4px;">
                            <strong style="font-size:0.9em;">{risk.id}</strong>
                            <div style="display:flex;gap:6px;align-items:center;">
                              <span class="badge" style={`color:${
                                risk.severity === 'critical' ? 'hsl(var(--destructive))' :
                                risk.severity === 'high' ? '#fb923c' :
                                risk.severity === 'medium' ? '#fbbf24' : '#34d399'
                              }`}>
                                {risk.severity.toUpperCase()}
                              </span>
                              <span style="font-size:0.8em;color:hsl(var(--muted-foreground));">
                                {Math.round(risk.probability * 100)}% likely
                              </span>
                            </div>
                          </div>
                          <p style="margin:0;font-size:0.85em;color:hsl(var(--muted-foreground));">{risk.description}</p>
                          <Show when={risk.evidence}>
                            <pre style="margin:6px 0 0;font-size:0.75em;padding:6px;background:hsl(var(--background));border-radius:4px;overflow:auto;max-height:80px;">{risk.evidence}</pre>
                          </Show>
                        </div>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={!ws.auditResult()!.risks?.length}>
                  <div style="padding:12px;text-align:center;color:#34d399;">✅ No risks identified</div>
                </Show>
              </div>
              <div style="display:flex;gap:8px;justify-content:flex-end;">
                <Button variant="outline" onClick={() => { ws.setAuditTarget(null); ws.setAuditResult(null); }}>Close</Button>
              </div>
            </Show>
          </Show>
          </DialogContent>
        </Dialog>

        <div class="workspace-viewer">
          <Show
            when={ws.fileContent()}
            fallback={
              <div class="workspace-viewer-empty">
                <p>Select a file to view its contents</p>
              </div>
            }
          >
            {(content) => (
              <>
                <div class="workspace-viewer-header">
                  <span class="workspace-viewer-filename">{content().path}</span>
                  <span class="pill neutral">{content().mime_type}</span>
                  <span class="pill neutral">{ws.formatFileSize(content().size)}</span>
                  <div class="workspace-viewer-actions ml-auto">
                    <Show when={!content().is_binary}>
                      <Button
                        size="sm"
                        variant={findOpen() ? 'default' : 'outline'}
                        onClick={() => findOpen() ? closeFind() : openFind()}
                        title="Find in file (Ctrl+F)"
                      >
                        <Search size={14} />
                      </Button>
                    </Show>
                  </div>
                  <Show when={!content().is_binary && !content().read_only}>
                    <div class="workspace-viewer-actions">
                      <Button
                        size="sm"
                        variant={editMode() ? 'default' : 'outline'}
                        onClick={() => setEditMode(!editMode())}
                        title={editMode() ? 'Switch to view mode' : 'Switch to edit mode'}
                      >
                        <Show when={editMode()} fallback={<><Pencil size={14} /> Edit</>}>
                          <><Eye size={14} /> View</>
                        </Show>
                      </Button>
                      <Show when={editMode()}>
                        <Button
                          size="sm"
                          data-testid="workspace-save-btn"
                          aria-label="Save file"
                          disabled={ws.fileSaving()}
                          onClick={() => void ws.saveWorkspaceFile()}
                        >
                          <Save size={14} />
                          {ws.fileSaving() ? 'Saving...' : 'Save'}
                        </Button>
                      </Show>
                    </div>
                  </Show>
                </div>
                <Show when={findOpen()}>
                  <FindBar
                    query={findQuery()}
                    onQueryChange={handleFindQueryChange}
                    matchCount={findMatchCount()}
                    currentMatch={findMatchCount() > 0 ? currentFindMatch() + 1 : 0}
                    onNext={findNext}
                    onPrev={findPrev}
                    onClose={closeFind}
                    inputRef={(el) => { findInputRef = el; }}
                  />
                </Show>
                <div class="workspace-viewer-content">
                  <Show when={!content().is_binary}>
                    <Show when={editMode() && !content().read_only}>
                      <HighlightedEditor
                        value={ws.fileEditorContent()}
                        language={currentLanguage()}
                        onInput={(val) => ws.setFileEditorContent(val)}
                        themeFamily={getThemeFamily()}
                      />
                    </Show>
                    <Show when={!editMode()}>
                      <Show
                        when={isMarkdown()}
                        fallback={
                          <CodeViewer
                            code={content().content}
                            language={currentLanguage()}
                            onNavigate={handleNavigate}
                            workspacePaths={workspacePathSet()}
                            currentFilePath={content().path}
                            findQuery={findOpen() ? findQuery() : undefined}
                            currentFindMatch={currentFindMatch()}
                            onFindMatchCount={setFindMatchCount}
                            themeFamily={getThemeFamily()}
                          />
                        }
                      >
                        <MarkdownViewer
                          source={content().content}
                          onNavigate={handleNavigate}
                          workspacePaths={workspacePathSet()}
                          currentFilePath={content().path}
                          themeFamily={getThemeFamily()}
                        />
                      </Show>
                    </Show>
                  </Show>
                  <Show when={content().mime_type.startsWith('image/')}>
                    <img
                      src={`data:${content().mime_type};base64,${content().content}`}
                      alt={content().path}
                      class="workspace-image-preview"
                    />
                  </Show>
                  <Show when={content().mime_type === 'application/pdf'}>
                    <iframe
                      src={`data:application/pdf;base64,${content().content}`}
                      class="workspace-pdf-preview"
                      title={content().path}
                    />
                  </Show>
                  <Show when={content().mime_type === 'model/stl'}>
                    <Suspense fallback={<div class="workspace-viewer-empty"><p>Loading 3D viewer…</p></div>}>
                      <StlViewer content={content().content} filename={content().path} />
                    </Suspense>
                  </Show>
                  <Show when={content().is_binary && !content().mime_type.startsWith('image/') && content().mime_type !== 'application/pdf' && content().mime_type !== 'model/stl'}>
                    <div class="workspace-viewer-empty">
                      <p>Binary file ({content().mime_type}) — preview not available</p>
                      <p class="muted">{ws.formatFileSize(content().size)}</p>
                    </div>
                  </Show>
                </div>
              </>
            )}
          </Show>
        </div>
      </div>
    </div>
  );
};

export default WorkspaceView;
