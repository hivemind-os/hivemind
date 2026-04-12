import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from 'solid-js';
import { Tabs, TabsList, TabsTrigger } from '~/ui/tabs';
import { Button } from '~/ui/button';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { MessageSquare, ClipboardList, RefreshCw, Folder, FileText } from 'lucide-solid';
import EventLogList, { type SupervisorEvent } from './EventLogList';
import CodeViewer from './CodeViewer';
import MarkdownViewer from './MarkdownViewer';
import { languageFromPath, extensionFromPath } from '../lib/languageMap';
import { buildWorkspacePathSet } from '../lib/importLinks';
import type { BotStore } from '../stores/botStore';
import { getThemeFamily } from '../stores/themeStore';

interface ChatMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  pending?: boolean;
}

interface WorkspaceEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size?: number;
  children?: WorkspaceEntry[];
}

interface WorkspaceFileContent {
  path: string;
  content: string;
  is_binary: boolean;
  mime_type: string;
  size: number;
}

export interface BotDetailViewProps {
  bot_id: string;
  botStore: BotStore;
}

type DetailTab = 'chat' | 'events' | 'workspace';

/** Build chat messages from supervisor events, stable and pure. */
function buildChatFromEvents(events: SupervisorEvent[]): ChatMessage[] {
  const messages: ChatMessage[] = [];
  let msgIdx = 0;
  for (const ev of events) {
    if (ev.type === 'agent_task_assigned' && ev.task) {
      messages.push({ id: `ev-${msgIdx++}`, role: 'user', content: ev.task });
    }
    if (ev.type === 'agent_output' && ev.event?.type === 'completed' && ev.event.result) {
      messages.push({ id: `ev-${msgIdx++}`, role: 'assistant', content: ev.event.result });
    }
  }
  return messages;
}

export default function BotDetailView(props: BotDetailViewProps) {
  let pendingIdCounter = 0;
  const [activeTab, setActiveTab] = createSignal<DetailTab>('chat');
  const [messageInput, setMessageInput] = createSignal('');
  const [sending, setSending] = createSignal(false);
  const [pendingMessages, setPendingMessages] = createSignal<ChatMessage[]>([]);

  // Events state (self-fetched)
  const [events, setEvents] = createSignal<SupervisorEvent[]>([]);
  const [eventsLoading, setEventsLoading] = createSignal(false);

  // Workspace state
  const [wsFiles, setWsFiles] = createSignal<WorkspaceEntry[]>([]);
  const [wsLoading, setWsLoading] = createSignal(false);
  const [selectedFile, setSelectedFile] = createSignal<WorkspaceFileContent | null>(null);
  const [fileLoading, setFileLoading] = createSignal(false);

  // Bot summary from store
  const botSummary = createMemo(() =>
    props.botStore.bots().find(b => b.config.id === props.bot_id)
  );

  // Fetch events
  const fetchEvents = async () => {
    setEventsLoading(true);
    try {
      const result = await invoke<SupervisorEvent[]>('get_bot_events', { agent_id: props.bot_id, limit: 100 });
      setEvents(result ?? []);
    } catch (err) {
      console.error('[BotDetailView] Failed to fetch events:', err);
    } finally {
      setEventsLoading(false);
    }
  };

  // Fetch snapshot on mount / bot_id change, then subscribe to push events
  createEffect(on(() => props.bot_id, () => {
    // Reset state from previous bot
    setPendingMessages([]);
    setWsFiles([]);
    setSelectedFile(null);
    setActiveTab('chat');

    let eventUnlisten: UnlistenFn | null = null;
    let refreshDebounce: ReturnType<typeof setTimeout> | undefined;
    let disposed = false;

    // Initial snapshot
    void fetchEvents();

    // Ensure bot SSE stream is running, then listen for push events
    void (async () => {
      try {
        await invoke('ensure_bot_stream');
      } catch (e) {
        console.warn('[BotDetailView] Failed to ensure bot stream:', e);
      }

      if (disposed) return;

      eventUnlisten = await listen<{ session_id: string; event: any }>(
        'stage:event',
        (ev) => {
          if (ev.payload.session_id !== '__service__') return;
          if (refreshDebounce) clearTimeout(refreshDebounce);
          refreshDebounce = setTimeout(() => void fetchEvents(), 200);
        },
      );

      if (disposed) { eventUnlisten(); eventUnlisten = null; }
    })();

    onCleanup(() => {
      disposed = true;
      if (eventUnlisten) { eventUnlisten(); eventUnlisten = null; }
      if (refreshDebounce) clearTimeout(refreshDebounce);
    });
  }));

  // Pure derivation: messages from events
  const eventMessages = createMemo(() => buildChatFromEvents(events()));

  // Reconcile: remove pending messages that have appeared in event messages
  createEffect(on(eventMessages, (evMsgs) => {
    const evUserIds = new Set(evMsgs.filter(m => m.role === 'user').map(m => m.id));
    setPendingMessages(prev => prev.filter(pm => !evUserIds.has(pm.id)));
  }));

  // Combined message list: event messages + remaining pending
  const chatMessages = createMemo(() => [
    ...eventMessages(),
    ...pendingMessages(),
  ]);

  // Agent working state
  const working = createMemo(() => botSummary()?.status === 'active');

  // Auto-scroll chat
  let chatEndRef: HTMLDivElement | undefined;
  createEffect(() => {
    chatMessages(); // track
    setTimeout(() => chatEndRef?.scrollIntoView({ behavior: 'smooth' }), 50);
  });

  const sendMessage = async () => {
    const text = messageInput().trim();
    if (!text || sending()) return;
    setSending(true);
    setMessageInput('');
    const pendingMsg: ChatMessage = {
      id: `pending-${++pendingIdCounter}`,
      role: 'user',
      content: text,
      pending: true,
    };
    setPendingMessages(prev => [...prev, pendingMsg]);
    try {
      await invoke('message_bot', { agent_id: props.bot_id, content: text });
    } catch (err) {
      console.error('Failed to send message:', err);
      setPendingMessages(prev => prev.filter(m => m.id !== pendingMsg.id));
    } finally {
      setSending(false);
    }
  };

  const loadWorkspace = async () => {
    setWsLoading(true);
    try {
      const files = await invoke<WorkspaceEntry[]>('bot_workspace_list_files', { bot_id: props.bot_id });
      setWsFiles(files);
    } catch {
      setWsFiles([]);
    } finally {
      setWsLoading(false);
    }
  };

  const openFile = async (path: string) => {
    setFileLoading(true);
    try {
      const file = await invoke<WorkspaceFileContent>('bot_workspace_read_file', { bot_id: props.bot_id, path });
      setSelectedFile(file);
    } catch (err) {
      console.error('Failed to read file:', err);
    } finally {
      setFileLoading(false);
    }
  };

  // Load workspace on tab switch
  createEffect(() => {
    if (activeTab() === 'workspace' && wsFiles().length === 0) {
      void loadWorkspace();
    }
  });

  const handleApprove = async (request_id: string, approved: boolean) => {
    try {
      await invoke('bot_interaction', { agent_id: props.bot_id, response: { request_id, approved } });
      void fetchEvents();
    } catch (err) {
      console.error('Failed to respond to approval:', err);
    }
  };

  return (
    <div class="flex h-full flex-col overflow-hidden">
      {/* Header bar */}
      <div class="flex items-center justify-between border-b border-input px-4 py-2">
        <div class="flex items-center gap-3">
          <span class="text-xl">{botSummary()?.config.avatar || '🤖'}</span>
          <div>
            <div class="text-sm font-semibold text-foreground">{botSummary()?.config.friendly_name}</div>
            <div class="text-xs text-muted-foreground">{props.bot_id} • {botSummary()?.status}</div>
          </div>
        </div>
      </div>

      {/* Tab bar */}
      <Tabs value={activeTab()} onChange={(v: string) => setActiveTab(v as DetailTab)} class="px-4 pt-2">
        <TabsList class="bg-muted/50 w-full">
          <TabsTrigger value="chat" class="flex-1 gap-1"><MessageSquare size={14} /> Chat</TabsTrigger>
          <TabsTrigger value="events" class="flex-1 gap-1"><ClipboardList size={14} /> Events</TabsTrigger>
          <TabsTrigger value="workspace" class="flex-1 gap-1"><Folder size={14} /> Workspace</TabsTrigger>
        </TabsList>
      </Tabs>

      {/* Tab content */}
      <div class="flex-1 overflow-hidden flex flex-col min-h-0">

        {/* Chat tab */}
        <Show when={activeTab() === 'chat'}>
          <div class="flex-1 overflow-y-auto p-3 flex flex-col gap-2">
            <Show
              when={chatMessages().length > 0}
              fallback={<p class="text-muted-foreground text-center py-6">No messages yet. Send a message to start a conversation.</p>}
            >
              <For each={chatMessages()}>
                {(msg) => (
                  <div
                    class="py-2 px-3 rounded-[10px] max-w-[85%] text-sm leading-relaxed whitespace-pre-wrap break-words"
                    classList={{
                      'self-end bg-accent text-accent-foreground rounded-br-sm': msg.role === 'user',
                      'self-start bg-secondary text-foreground rounded-bl-sm': msg.role !== 'user',
                    }}
                  >
                    {msg.content}
                  </div>
                )}
              </For>
              <Show when={working()}>
                <div class="self-start py-2 px-4 rounded-[10px] rounded-bl-sm bg-secondary text-muted-foreground text-sm flex items-center gap-1.5">
                  <span class="inline-flex gap-[3px]">
                    <span style="animation:dot-blink 1.4s infinite both;animation-delay:0s;">●</span>
                    <span style="animation:dot-blink 1.4s infinite both;animation-delay:0.2s;">●</span>
                    <span style="animation:dot-blink 1.4s infinite both;animation-delay:0.4s;">●</span>
                  </span>
                  <span>Working…</span>
                </div>
              </Show>
            </Show>
            <div ref={(el) => { chatEndRef = el; }} />
          </div>

          {/* Message input */}
          <div class="flex gap-1.5 p-3 border-t border-border">
            <input
              type="text"
              placeholder="Send a message..."
              value={messageInput()}
              onInput={(e) => setMessageInput(e.currentTarget.value)}
              onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void sendMessage(); } }}
              class="flex-1 bg-secondary text-foreground border border-border rounded-md px-2.5 py-1.5 text-sm"
            />
            <Button
              size="sm"
              disabled={!messageInput().trim() || sending()}
              onClick={() => void sendMessage()}
            >
              {sending() ? '...' : 'Send'}
            </Button>
          </div>
        </Show>

        {/* Events tab */}
        <Show when={activeTab() === 'events'}>
          <div class="flex-1 overflow-y-auto p-3">
            <EventLogList
              events={events()}
              totalCount={events().length}
              loading={eventsLoading()}
              hasMore={false}
              onApprove={(reqId, approved) => handleApprove(reqId, approved)}
            />
          </div>
        </Show>

        {/* Workspace tab */}
        <Show when={activeTab() === 'workspace'}>
          <div class="flex-1 overflow-y-auto p-3 flex gap-3 min-h-0">
            {/* File tree */}
            <div class="w-[220px] shrink-0 overflow-y-auto border-r border-border pr-2.5">
              <div class="flex justify-between items-center mb-2">
                <span class="text-xs font-semibold">Files</span>
                <button
                  onClick={() => void loadWorkspace()}
                  class="bg-transparent border-none cursor-pointer text-muted-foreground text-xs"
                  title="Refresh"
                ><RefreshCw size={14} /></button>
              </div>
              <Show when={!wsLoading()} fallback={<p class="text-xs text-muted-foreground">Loading…</p>}>
                <Show when={wsFiles().length > 0} fallback={<p class="text-xs text-muted-foreground">Workspace is empty.</p>}>
                  <FileTree entries={wsFiles()} onSelect={(path) => void openFile(path)} selectedPath={selectedFile()?.path} depth={0} />
                </Show>
              </Show>
            </div>

            {/* File content */}
            <div class="flex-1 overflow-y-auto min-w-0">
              <Show when={selectedFile()} fallback={
                <p class="text-muted-foreground text-center py-6 text-sm">
                  Select a file to view its contents.
                </p>
              }>
                {(file) => (
                  <div>
                    <div class="text-xs font-semibold mb-1.5 text-muted-foreground">
                      {file().path}
                      <span class="ml-2 font-normal text-muted-foreground">
                        {formatFileSize(file().size)}
                      </span>
                    </div>
                    <Show when={fileLoading()}>
                      <p class="text-xs text-muted-foreground">Loading…</p>
                    </Show>
                    <Show when={!file().is_binary} fallback={
                      <p class="text-xs text-muted-foreground">Binary file — preview not available.</p>
                    }>
                      {(() => {
                        const ext = extensionFromPath(file().path);
                        const isMd = ext === 'md' || ext === 'mdx';
                        const lang = languageFromPath(file().path);
                        const wsPathSet = buildWorkspacePathSet(wsFiles());
                        const navigate = (path: string) => void openFile(path);
                        return (
                          <div class="max-h-[60vh] overflow-auto rounded-md border border-border">
                            <Show when={isMd} fallback={
                              <CodeViewer
                                code={file().content}
                                language={lang}
                                onNavigate={navigate}
                                workspacePaths={wsPathSet}
                                currentFilePath={file().path}
                                themeFamily={getThemeFamily()}
                              />
                            }>
                              <MarkdownViewer
                                source={file().content}
                                onNavigate={navigate}
                                workspacePaths={wsPathSet}
                                currentFilePath={file().path}
                                themeFamily={getThemeFamily()}
                              />
                            </Show>
                          </div>
                        );
                      })()}
                    </Show>
                  </div>
                )}
              </Show>
            </div>
          </div>
        </Show>
      </div>
    </div>
  );
}

function FileTree(props: { entries: WorkspaceEntry[]; onSelect: (path: string) => void; selectedPath?: string; depth: number }) {
  return (
    <div style={`padding-left:${props.depth * 12}px;`}>
      <For each={props.entries}>
        {(entry) => (
          <div>
            <div
              onClick={() => !entry.is_dir && props.onSelect(entry.path)}
              class="py-0.5 px-1 text-xs rounded-sm whitespace-nowrap overflow-hidden text-ellipsis"
              classList={{
                'cursor-pointer text-foreground': !entry.is_dir,
                'cursor-default text-muted-foreground': entry.is_dir,
                'bg-accent/15': props.selectedPath === entry.path,
              }}
              title={entry.path}
            >
              {entry.is_dir ? <Folder size={14} /> : <FileText size={14} />}{' '}{entry.name}
            </div>
            <Show when={entry.is_dir && entry.children}>
              <FileTree entries={entry.children!} onSelect={props.onSelect} selectedPath={props.selectedPath} depth={props.depth + 1} />
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
