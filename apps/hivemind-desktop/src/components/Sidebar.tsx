import { createEffect, createMemo, createSignal, For, on, Show, type Accessor, type Setter } from 'solid-js';
import { open } from '@tauri-apps/plugin-dialog';
import { Plus, Settings, Search, Bot, Clock, GitBranch, MessageSquare, Compass, Home, FolderOpen, Trash2, Pencil, ChevronDown, LoaderCircle, Package } from 'lucide-solid';
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, Button,
  Switch, SwitchControl, SwitchThumb, SwitchLabel,
  SidebarContent, SidebarFooter, SidebarGroup, SidebarGroupContent, SidebarUiHeader,
  SidebarMenu, SidebarMenuButton, SidebarMenuItem, SidebarMenuAction, SidebarMenuBadge,
  SidebarTrigger, SidebarSeparator,
  ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuSeparator,
} from '~/ui';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import type { ChatSessionSummary, Persona, SessionModality } from '../types';
import type { BotStore } from '../stores/botStore';
import type { WorkflowStore } from '../stores/workflowStore';
import type { InteractionStore } from '../stores/interactionStore';
import SidebarBotList from './SidebarBotList';
import SidebarWorkflowList from './SidebarWorkflowList';
import { formatTime } from '../utils';
import { buildNamespaceTree, type NamespaceNode } from '../lib/workflowGrouping';

export interface SidebarProps {
  sessions: Accessor<ChatSessionSummary[]>;
  selectedSessionId: Accessor<string | null>;
  daemonOnline: Accessor<boolean>;
  busyAction: Accessor<string | null>;
  inspectorOpen: Accessor<boolean>;
  setInspectorOpen: Setter<boolean>;
  showNewSessionDialog: Accessor<boolean>;
  setShowNewSessionDialog: Setter<boolean>;
  createSession: (modality: SessionModality, workspace_path?: string, persona_id?: string) => Promise<void>;
  personas: Accessor<Persona[]>;
  selectSession: (session_id: string) => Promise<void>;
  deleteSession: (session_id: string, scrubKb: boolean) => Promise<void>;
  renameSession: (session_id: string, title: string) => Promise<void>;
  onOpenSettings: () => void;
  onReorderSessions: (fromIndex: number, toIndex: number) => void;
  activeScreen: Accessor<'session' | 'bots' | 'scheduler' | 'workflows' | 'settings' | 'agent-kits'>;
  setActiveScreen: Setter<'session' | 'bots' | 'scheduler' | 'workflows' | 'settings' | 'agent-kits'>;
  botStore: BotStore;
  workflowStore: WorkflowStore;
  interactionStore: InteractionStore;
  onSelectBot?: (bot_id: string) => void;
}

const Sidebar = (props: SidebarProps) => {
  const [deleteTarget, setDeleteTarget] = createSignal<{ id: string; title: string } | null>(null);
  const [scrubKb, setScrubKb] = createSignal(false);
  const [isDeleting, setIsDeleting] = createSignal(false);
  const [editingSessionId, setEditingSessionId] = createSignal<string | null>(null);
  const [editingTitle, setEditingTitle] = createSignal('');
  const [renameError, setRenameError] = createSignal<string | null>(null);

  // New-session wizard state
  const [newSessionStep, setNewSessionStep] = createSignal<'modality' | 'workspace'>('modality');
  const [pendingModality, setPendingModality] = createSignal<SessionModality>('linear');
  const [pendingPersonaId, setPendingPersonaId] = createSignal<string | undefined>(undefined);

  const activePersonas = () => props.personas().filter(p => !p.archived);

  // Persona picker popover state
  const [personaPickerOpen, setPersonaPickerOpen] = createSignal(false);
  const [personaSearch, setPersonaSearch] = createSignal('');
  const [collapsedPersonaNs, setCollapsedPersonaNs] = createSignal<Set<string>>(new Set());

  const filteredPersonas = createMemo(() => {
    const q = personaSearch().toLowerCase();
    const list = activePersonas();
    if (!q) return list;
    return list.filter(p =>
      p.name.toLowerCase().includes(q) ||
      p.id.toLowerCase().includes(q) ||
      (p.description || '').toLowerCase().includes(q)
    );
  });

  const filteredPersonaTree = createMemo(() =>
    buildNamespaceTree(filteredPersonas(), (p) => p.id, (p) => p.name)
  );

  const togglePersonaNs = (ns: string) => {
    setCollapsedPersonaNs(prev => {
      const next = new Set(prev);
      next.has(ns) ? next.delete(ns) : next.add(ns);
      return next;
    });
  };

  const countPersonaItems = (node: NamespaceNode<Persona>): number =>
    node.items.length + node.children.reduce((sum, c) => sum + countPersonaItems(c), 0);

  function selectPersona(persona: Persona) {
    setPendingPersonaId(persona.id);
    setPersonaPickerOpen(false);
    setPersonaSearch('');
    props.setShowNewSessionDialog(true);
  }

  function renderPersonaNsNode(node: NamespaceNode<Persona>, depth: number): import('solid-js').JSX.Element {
    const paddingLeft = `${10 + depth * 12}px`;
    const itemPaddingLeft = `${20 + depth * 12}px`;
    return (
      <div>
        <div
          onMouseDown={(e) => { e.preventDefault(); togglePersonaNs(node.fullPath); }}
          style={`padding:5px 10px 5px ${paddingLeft};cursor:pointer;font-size:0.78em;font-weight:600;color:hsl(var(--muted-foreground));display:flex;align-items:center;gap:6px;user-select:none;text-transform:uppercase;letter-spacing:0.04em;border-bottom:1px solid hsl(var(--border) / 0.3);`}
        >
          <span style={`display:inline-block;transition:transform 0.15s;transform:${collapsedPersonaNs().has(node.fullPath) ? 'rotate(-90deg)' : 'rotate(0deg)'};font-size:0.9em;`}>▾</span>
          {node.segment}
          <span style="font-size:0.85em;opacity:0.7;">({countPersonaItems(node)})</span>
        </div>
        <Show when={!collapsedPersonaNs().has(node.fullPath)}>
          <For each={node.items}>
            {(persona) => (
              <div
                data-testid={`persona-option-${persona.id}`}
                onMouseDown={(e) => { e.preventDefault(); selectPersona(persona); }}
                style={`padding:6px 10px 6px ${itemPaddingLeft};cursor:pointer;font-size:0.82em;color:hsl(var(--foreground));`}
                class="tool-dropdown-item"
              >
                <div style="font-weight:500;">{persona.name}</div>
                <Show when={persona.description}>
                  <div style="font-size:0.85em;color:hsl(var(--muted-foreground));overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{persona.description}</div>
                </Show>
              </div>
            )}
          </For>
          <For each={node.children}>
            {(child) => renderPersonaNsNode(child, depth + 1)}
          </For>
        </Show>
      </div>
    );
  }

  const [createSessionError, setCreateSessionError] = createSignal<string | null>(null);

  const resetNewSessionDialog = () => {
    setNewSessionStep('modality');
    setPendingModality('linear');
    setPendingPersonaId(undefined);
    setCreateSessionError(null);
    props.setShowNewSessionDialog(false);
  };

  const handleModalityPick = (modality: SessionModality) => {
    setPendingModality(modality);
    setNewSessionStep('workspace');
  };

  const handleWorkspacePick = async (mode: 'default' | 'choose') => {
    if (props.busyAction() !== null) return;
    setCreateSessionError(null);
    const persona_id = pendingPersonaId();
    try {
      if (mode === 'choose') {
        const folder = await open({ directory: true, multiple: false, title: 'Choose workspace folder' });
        if (!folder) return;
        await props.createSession(pendingModality(), folder as string, persona_id);
      } else {
        await props.createSession(pendingModality(), undefined, persona_id);
      }
      resetNewSessionDialog();
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setCreateSessionError(msg);
    }
  };

  const commitRename = async () => {
    const id = editingSessionId();
    const title = editingTitle().trim();
    if (id && title) {
      try {
        await props.renameSession(id, title);
        setRenameError(null);
        setEditingSessionId(null);
      } catch (e: any) {
        setRenameError(e?.message ?? 'Rename failed');
      }
    } else {
      setEditingSessionId(null);
    }
  };

  const cancelRename = () => {
    setEditingSessionId(null);
    setRenameError(null);
  };

  // Unread tracking
  const [unreadIds, setUnreadIds] = createSignal<Set<string>>(new Set());
  const lastSeenMs = new Map<string, number>();

  // Drag-to-reorder state
  const [dragFromIndex, setDragFromIndex] = createSignal<number | null>(null);
  const [dragOverIndex, setDragOverIndex] = createSignal<number | null>(null);

  createEffect(on(props.sessions, (sessions) => {
    const selectedId = props.selectedSessionId();
    const currentIds = new Set(sessions.map(s => s.id));
    // Prune entries for deleted sessions
    for (const key of lastSeenMs.keys()) {
      if (!currentIds.has(key)) lastSeenMs.delete(key);
    }
    setUnreadIds((prev) => {
      const next = new Set(prev);
      for (const s of sessions) {
        if (s.id === selectedId) {
          lastSeenMs.set(s.id, s.updated_at_ms);
          next.delete(s.id);
        } else {
          const lastSeen = lastSeenMs.get(s.id);
          if (lastSeen === undefined) {
            lastSeenMs.set(s.id, s.updated_at_ms);
          } else if (s.updated_at_ms > lastSeen) {
            next.add(s.id);
          }
        }
      }
      return next;
    });
  }));

  createEffect(on(props.selectedSessionId, (selectedId) => {
    if (!selectedId) return;
    const s = props.sessions().find((x) => x.id === selectedId);
    if (s) lastSeenMs.set(s.id, s.updated_at_ms);
    setUnreadIds((prev) => {
      if (!prev.has(selectedId)) return prev;
      const next = new Set(prev);
      next.delete(selectedId);
      return next;
    });
  }));

  return (
    <>
      <SidebarUiHeader class="p-3">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-2">
            <SidebarTrigger data-testid="sidebar-expand" />
            <span class="text-sm font-semibold text-sidebar-foreground">HiveMind OS</span>
          </div>
          <div class="flex items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon"
              class="size-7"
              disabled={!props.daemonOnline() || props.busyAction() !== null}
              data-testid="new-session-btn"
              onClick={() => props.setShowNewSessionDialog(true)}
              title="New session"
              aria-label="New session"
            >
              <Plus size={16} />
            </Button>
            <Popover
              open={personaPickerOpen()}
              onOpenChange={(o) => { setPersonaPickerOpen(o); if (!o) setPersonaSearch(''); }}
              placement="bottom-end"
              gutter={2}
            >
              <PopoverTrigger
                as={Button}
                variant="ghost"
                size="icon"
                class="size-7"
                disabled={!props.daemonOnline() || props.busyAction() !== null || activePersonas().length === 0}
                data-testid="new-session-persona-btn"
                title="New session with persona"
                aria-label="New session with persona"
              >
                <ChevronDown size={12} />
              </PopoverTrigger>
              <PopoverContent class="w-auto p-0" style={{
                'z-index': '10000',
                'min-width': '260px',
                'max-height': '320px',
                'overflow-y': 'auto',
                background: 'hsl(var(--card))',
                border: '1px solid hsl(var(--border))',
                'border-radius': '6px',
                'box-shadow': '0 6px 16px hsl(0 0% 0% / 0.5)',
              }}>
                <div style="padding:4px;border-bottom:1px solid hsl(var(--border));">
                  <input
                    ref={(el) => requestAnimationFrame(() => el?.focus())}
                    type="text"
                    value={personaSearch()}
                    onInput={(e) => setPersonaSearch(e.currentTarget.value)}
                    onKeyDown={(e) => { if (e.key === 'Escape') { setPersonaPickerOpen(false); setPersonaSearch(''); } }}
                    placeholder="Search personas…"
                    style="width:100%;padding:4px 8px;border-radius:4px;border:none;background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;box-sizing:border-box;"
                  />
                </div>
                <For each={filteredPersonaTree()}>
                  {(node) => renderPersonaNsNode(node, 0)}
                </For>
                <Show when={filteredPersonas().length === 0}>
                  <div style="padding:8px 10px;font-size:0.8em;color:hsl(var(--muted-foreground));text-align:center;">No personas found</div>
                </Show>
              </PopoverContent>
            </Popover>
            <Button
              variant="ghost"
              size="icon"
              class="size-7"
              onClick={() => props.setInspectorOpen(!props.inspectorOpen())}
              title="Inspector"
              aria-label="Inspector"
            >
              <Search size={16} />
            </Button>
          </div>
        </div>
      </SidebarUiHeader>

      <SidebarContent>
        {/* Navigation */}
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'session'}
                  onClick={() => props.setActiveScreen('session')}
                  size="sm"
                >
                  <MessageSquare size={16} />
                  <span>Sessions</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'bots'}
                  onClick={() => props.setActiveScreen(props.activeScreen() === 'bots' ? 'session' : 'bots')}
                  size="sm"
                >
                  <Bot size={16} />
                  <span>Bots</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'scheduler'}
                  onClick={() => props.setActiveScreen(props.activeScreen() === 'scheduler' ? 'session' : 'scheduler')}
                  size="sm"
                >
                  <Clock size={16} />
                  <span>Scheduler</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'workflows'}
                  onClick={() => props.setActiveScreen(props.activeScreen() === 'workflows' ? 'session' : 'workflows')}
                  size="sm"
                >
                  <GitBranch size={16} />
                  <span>Workflows</span>
                </SidebarMenuButton>
                <SidebarMenuAction
                  showOnHover
                  aria-label="Manage workflow definitions"
                  data-testid="wf-definitions-toggle"
                  onClick={(e: MouseEvent) => {
                    e.stopPropagation();
                    props.setActiveScreen('workflows');
                    props.workflowStore.setViewDefinitions(true);
                  }}
                >
                  <Settings size={14} />
                </SidebarMenuAction>
              </SidebarMenuItem>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'agent-kits'}
                  onClick={() => props.setActiveScreen(props.activeScreen() === 'agent-kits' ? 'session' : 'agent-kits')}
                  size="sm"
                >
                  <Package size={16} />
                  <span>Agent Kits</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
              <SidebarMenuItem>
                <SidebarMenuButton
                  isActive={props.activeScreen() === 'settings'}
                  onClick={() => { props.setActiveScreen('settings'); props.onOpenSettings(); }}
                  size="sm"
                  data-testid="sidebar-settings-btn"
                >
                  <Settings size={16} />
                  <span>Settings</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>

        <SidebarSeparator />

      <Dialog
        open={props.showNewSessionDialog()}
        onOpenChange={(open) => { if (!open) resetNewSessionDialog(); }}
      >
        <DialogContent class="min-w-[360px] max-w-[440px]" data-testid="new-session-dialog">
          <DialogHeader>
            <DialogTitle>
              {pendingPersonaId()
                ? `New Session — ${activePersonas().find(p => p.id === pendingPersonaId())?.name ?? pendingPersonaId()}`
                : 'New Session'}
            </DialogTitle>
          </DialogHeader>

          <Show when={newSessionStep() === 'modality'}>
            <div class="flex gap-3">
              <button
                data-testid="modality-classic"
                aria-label="Classic Chat"
                onClick={() => handleModalityPick('linear')}
                class="flex-1 cursor-pointer rounded-lg border border-input bg-secondary p-5 text-center text-foreground hover:border-primary"
              >
                <div class="mb-2 text-2xl"><MessageSquare size={24} /></div>
                <div class="font-semibold">Classic Chat</div>
                <div class="mt-1 text-xs text-muted-foreground">Linear conversation</div>
              </button>
              <button
                data-testid="modality-spatial"
                aria-label="Spatial Canvas"
                onClick={() => handleModalityPick('spatial')}
                class="relative flex-1 cursor-pointer rounded-lg border border-input bg-secondary p-5 text-center text-foreground hover:border-primary"
              >
                <span class="absolute top-2 right-2 rounded-full bg-amber-500/15 px-2 py-0.5 text-[0.65rem] font-semibold text-amber-400 ring-1 ring-amber-500/30">
                  Experimental
                </span>
                <div class="mb-2 text-2xl"><Compass size={24} /></div>
                <div class="font-semibold">Spatial Canvas</div>
                <div class="mt-1 text-xs text-muted-foreground">2D reasoning canvas</div>
              </button>
            </div>
            <Button variant="secondary" class="mt-3 w-full" onClick={() => resetNewSessionDialog()}>Cancel</Button>
          </Show>

          <Show when={newSessionStep() === 'workspace'}>
            <p class="mb-3 text-sm text-muted-foreground">Where should this session's workspace live?</p>
            <div class="flex gap-3">
              <button
                data-testid="workspace-default"
                aria-label="Default workspace"
                disabled={props.busyAction() !== null}
                onClick={() => void handleWorkspacePick('default')}
                class="flex-1 cursor-pointer rounded-lg border border-input bg-secondary p-5 text-center text-foreground hover:border-primary disabled:pointer-events-none disabled:opacity-50"
              >
                <div class="mb-2 text-2xl"><Home size={24} /></div>
                <div class="font-semibold">Default</div>
                <div class="mt-1 text-xs text-muted-foreground">Under HiveMind home</div>
              </button>
              <button
                data-testid="workspace-existing"
                aria-label="Choose existing folder"
                disabled={props.busyAction() !== null}
                onClick={() => void handleWorkspacePick('choose')}
                class="flex-1 cursor-pointer rounded-lg border border-input bg-secondary p-5 text-center text-foreground hover:border-primary disabled:pointer-events-none disabled:opacity-50"
              >
                <div class="mb-2 text-2xl"><FolderOpen size={24} /></div>
                <div class="font-semibold">Existing Folder</div>
                <div class="mt-1 text-xs text-muted-foreground">Choose a directory</div>
              </button>
            </div>
            <Show when={createSessionError()}>
              <p class="text-sm text-destructive mt-2">{createSessionError()}</p>
            </Show>
            <Button variant="secondary" class="mt-3 w-full" onClick={() => setNewSessionStep('modality')}>Back</Button>
          </Show>
        </DialogContent>
      </Dialog>

        {/* Bottom panel — context-sensitive */}
        <Show when={props.activeScreen() === 'bots'}>
          <SidebarBotList store={props.botStore} onSelectBot={props.onSelectBot} />
        </Show>

        <Show when={props.activeScreen() === 'workflows'}>
          <SidebarWorkflowList
            store={props.workflowStore}
            interactionStore={props.interactionStore}
          />
        </Show>

        <Show when={props.activeScreen() === 'session' || props.activeScreen() === 'scheduler'}>
        {/* Sessions */}
        <SidebarGroup>
          <SidebarGroupContent>
            <Show when={props.daemonOnline()} fallback={<p class="px-2 text-xs text-muted-foreground">Start the daemon to create or resume sessions.</p>}>
              <Show when={props.sessions().length > 0} fallback={<p class="px-2 text-xs text-muted-foreground">No sessions yet.</p>}>
                <SidebarMenu data-testid="session-list">
                  <For each={props.sessions()}>
                    {(item, index) => {
                      let clickTimer: ReturnType<typeof setTimeout> | undefined;
                      return (
                      <ContextMenu>
                        <ContextMenuTrigger
                          as={SidebarMenuItem}
                          data-testid={`session-item-${item.id}`}
                          draggable={true}
                          onDragStart={(e: DragEvent) => {
                            setDragFromIndex(index());
                            if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
                          }}
                          onDragOver={(e: DragEvent) => {
                            e.preventDefault();
                            if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
                            setDragOverIndex(index());
                          }}
                          onDragLeave={() => setDragOverIndex(null)}
                          onDrop={(e: DragEvent) => {
                            e.preventDefault();
                            const from = dragFromIndex();
                            const to = index();
                            if (from !== null && from !== to) {
                              props.onReorderSessions(from, to);
                            }
                            setDragFromIndex(null);
                            setDragOverIndex(null);
                          }}
                          onDragEnd={() => {
                            setDragFromIndex(null);
                            setDragOverIndex(null);
                          }}
                          class={dragOverIndex() === index() && dragFromIndex() !== index() ? 'ring-1 ring-sidebar-ring rounded-md' : ''}
                        >
                          <SidebarMenuButton
                            isActive={item.id === props.selectedSessionId()}
                            onClick={() => {
                              if (editingSessionId() !== null || props.busyAction() !== null) return;
                              clearTimeout(clickTimer);
                              clickTimer = setTimeout(() => {
                                if (editingSessionId() === null) void props.selectSession(item.id);
                              }, 250);
                            }}
                            onDblClick={(e: MouseEvent) => {
                              clearTimeout(clickTimer);
                              e.preventDefault();
                              e.stopPropagation();
                              setEditingSessionId(item.id);
                              setEditingTitle(item.title);
                            }}
                            onKeyDown={(e: KeyboardEvent) => {
                              if (e.key === 'F2' && editingSessionId() === null) {
                                e.preventDefault();
                                setEditingSessionId(item.id);
                                setEditingTitle(item.title);
                              }
                            }}
                            class="gap-1.5"
                            size="sm"
                          >
                            <span
                              class="inline-block size-2 shrink-0 rounded-full"
                              classList={{
                                'bg-green-400': item.state === 'running',
                                'bg-yellow-400': item.state === 'paused',
                                'bg-red-400': item.state === 'interrupted',
                                'bg-slate-600': item.state === 'idle',
                              }}
                              title={item.state}
                            />
                            <Show when={item.modality === 'spatial'}>
                              <Compass size={14} class="shrink-0" />
                            </Show>
                            <Show when={editingSessionId() === item.id} fallback={
                              <span class="truncate" classList={{ 'font-semibold': unreadIds().has(item.id) }}>
                                {item.title}
                              </span>
                            }>
                              <input
                                class={`w-full bg-transparent text-sm outline-none border-b ${renameError() ? 'border-red-400' : 'border-sidebar-ring'}`}
                                value={editingTitle()}
                                onInput={(e) => { setEditingTitle(e.currentTarget.value); setRenameError(null); }}
                                onBlur={() => void commitRename()}
                                onKeyDown={(e: KeyboardEvent) => {
                                  if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
                                  if (e.key === 'Escape') cancelRename();
                                }}
                                onClick={(e: MouseEvent) => e.stopPropagation()}
                                ref={(el) => setTimeout(() => { el.focus(); el.select(); }, 0)}
                                title={renameError() ?? undefined}
                              />
                            </Show>
                          </SidebarMenuButton>
                          <SidebarMenuAction
                            showOnHover
                            data-testid="delete-session-btn"
                            aria-label="Delete session"
                            title="Delete session"
                            onClick={(e: MouseEvent) => {
                              e.stopPropagation();
                              setDeleteTarget({ id: item.id, title: item.title });
                              setScrubKb(false);
                            }}
                          >
                            <Trash2 size={14} />
                          </SidebarMenuAction>
                          <Show when={unreadIds().has(item.id) || (item.queued_count ?? 0) > 0 || (() => { const c = props.interactionStore.badgeCountForEntity(`session/${item.id}`); return c.questions + c.approvals + c.gates > 0; })()}>
                            <SidebarMenuBadge>
                              <Show when={unreadIds().has(item.id)}>
                                <span class="mr-1 inline-block size-1.5 rounded-full bg-blue-400" />
                              </Show>
                              <Show when={(item.queued_count ?? 0) > 0}>
                                <span class="text-[10px]">{item.queued_count}</span>
                              </Show>
                              {(() => { const c = props.interactionStore.badgeCountForEntity(`session/${item.id}`); return (
                                <>
                                  <Show when={c.questions > 0}>
                                    <span class="ml-0.5 text-[10px] text-blue-400" title="Pending questions">❓{c.questions}</span>
                                  </Show>
                                  <Show when={c.approvals > 0}>
                                    <span class="ml-0.5 text-[10px] text-amber-400" title="Pending approvals">🔒{c.approvals}</span>
                                  </Show>
                                </>
                              ); })()}
                            </SidebarMenuBadge>
                          </Show>
                        </ContextMenuTrigger>
                        <ContextMenuContent>
                          <ContextMenuItem onSelect={() => {
                            setEditingSessionId(item.id);
                            setEditingTitle(item.title);
                          }}>
                            <Pencil size={14} />
                            Rename
                          </ContextMenuItem>
                          <ContextMenuSeparator />
                          <ContextMenuItem class="text-destructive focus:text-destructive" onSelect={() => {
                            setDeleteTarget({ id: item.id, title: item.title });
                            setScrubKb(false);
                          }}>
                            <Trash2 size={14} />
                            Delete
                          </ContextMenuItem>
                        </ContextMenuContent>
                      </ContextMenu>
                      );
                    }}
                  </For>
                </SidebarMenu>
              </Show>
            </Show>
          </SidebarGroupContent>
        </SidebarGroup>
        </Show>
      </SidebarContent>

      <SidebarFooter />

      {/* Delete confirmation dialog */}
      <Dialog
        open={!!deleteTarget()}
        onOpenChange={(open) => { if (!open && !isDeleting()) setDeleteTarget(null); }}
      >
        <DialogContent class="w-[400px]" data-testid="delete-session-dialog">
          <Show when={deleteTarget()}>
            {(target) => (
              <>
                <DialogHeader>
                  <DialogTitle>Delete session?</DialogTitle>
                </DialogHeader>
                <p class="mb-4 text-sm text-muted-foreground">
                  <strong>{target().title}</strong>
                </p>
                <Switch checked={scrubKb()} onChange={(checked) => setScrubKb(checked)} class="flex items-center gap-2" disabled={isDeleting()}>
                  <SwitchControl><SwitchThumb /></SwitchControl>
                  <SwitchLabel>Also remove from knowledge base</SwitchLabel>
                </Switch>
                <DialogFooter class="mt-4">
                  <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={isDeleting()}>Cancel</Button>
                  <Button
                    variant="destructive"
                    disabled={isDeleting()}
                    onClick={async () => {
                      const t = target();
                      setIsDeleting(true);
                      try {
                        await props.deleteSession(t.id, scrubKb());
                      } finally {
                        setIsDeleting(false);
                        setDeleteTarget(null);
                      }
                    }}
                  >
                    <Show when={isDeleting()} fallback="Delete">
                      <LoaderCircle size={14} class="animate-spin" style="margin-right:6px;" />
                      Deleting…
                    </Show>
                  </Button>
                </DialogFooter>
              </>
            )}
          </Show>
        </DialogContent>
      </Dialog>
    </>
  );
};

export default Sidebar;
