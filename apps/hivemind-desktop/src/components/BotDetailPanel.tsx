import { For, Show, createEffect, createMemo, createSignal, on } from 'solid-js';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Tabs, TabsList, TabsTrigger } from '~/ui/tabs';
import { Button } from '~/ui/button';
import { invoke } from '@tauri-apps/api/core';
import { MessageSquare, ClipboardList, RefreshCw, Folder, FileText } from 'lucide-solid';
import EventLogList, { type SupervisorEvent } from './EventLogList';

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

interface BotDetailPanelProps {
  agent_id: string;
  agent_name: string;
  agentAvatar: string;
  agentStatus: string;
  events: SupervisorEvent[];
  totalCount: number;
  eventsLoading: boolean;
  onClose: () => void;
  onApprove: (request_id: string, approved: boolean) => void;
  onLoadMore?: () => void;
}

type DetailTab = 'chat' | 'events' | 'workspace';

let pendingIdCounter = 0;

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

export default function BotDetailPanel(props: BotDetailPanelProps) {
  const [activeTab, setActiveTab] = createSignal<DetailTab>('chat');
  const [messageInput, setMessageInput] = createSignal('');
  const [sending, setSending] = createSignal(false);
  const [pendingMessages, setPendingMessages] = createSignal<ChatMessage[]>([]);

  // Workspace state
  const [wsFiles, setWsFiles] = createSignal<WorkspaceEntry[]>([]);
  const [wsLoading, setWsLoading] = createSignal(false);
  const [selectedFile, setSelectedFile] = createSignal<WorkspaceFileContent | null>(null);
  const [fileLoading, setFileLoading] = createSignal(false);

  // Pure derivation: messages from events (no side effects)
  const eventMessages = createMemo(() => buildChatFromEvents(props.events));

  // Reconcile: remove pending messages whose text appears in event messages
  // Use createEffect (not inside memo) to avoid reactive loops
  createEffect(on(eventMessages, (evMsgs) => {
    const evUserTexts = new Set(evMsgs.filter(m => m.role === 'user').map(m => m.content));
    setPendingMessages(prev => prev.filter(pm => !evUserTexts.has(pm.content)));
  }));

  // Combined message list: event messages + remaining pending
  const chatMessages = createMemo(() => [
    ...eventMessages(),
    ...pendingMessages(),
  ]);

  // Agent working state: use the authoritative status from the stage
  const working = createMemo(() => props.agentStatus === 'active');
  const isDone = createMemo(() => props.agentStatus === 'done' || props.agentStatus === 'error');

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
      await invoke('message_bot', { agent_id: props.agent_id, content: text });
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
      const files = await invoke<WorkspaceEntry[]>('bot_workspace_list_files', { bot_id: props.agent_id });
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
      const file = await invoke<WorkspaceFileContent>('bot_workspace_read_file', { bot_id: props.agent_id, path });
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

  return (
    <Dialog open={true} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent class="max-w-[700px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e: Event) => e.preventDefault()}>
        {/* Header */}
        <div class="flex items-center gap-3 px-6 pt-6 pb-2">
          <span class="text-2xl">{props.agentAvatar || '🤖'}</span>
          <div class="flex-1 min-w-0">
            <div class="text-sm font-semibold text-foreground truncate">{props.agent_name}</div>
            <div class="text-xs text-muted-foreground truncate">{props.agent_id}</div>
          </div>
          <button class="text-muted-foreground hover:text-foreground text-lg" onClick={() => props.onClose()}>✕</button>
        </div>

        {/* Tab bar */}
        <Tabs value={activeTab()} onChange={(v) => setActiveTab(v as DetailTab)} class="px-4">
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
            <Show when={!isDone()} fallback={
              <div class="p-3 border-t border-border text-center text-xs text-muted-foreground">
                This bot has finished — chat is read-only.
              </div>
            }>
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
          </Show>

          {/* Events tab */}
          <Show when={activeTab() === 'events'}>
            <div class="flex-1 overflow-y-auto p-3">
              <EventLogList
                events={props.events}
                totalCount={props.totalCount}
                loading={props.eventsLoading}
                hasMore={props.events.length < props.totalCount}
                onLoadMore={props.onLoadMore}
                onApprove={(reqId, approved) => props.onApprove(reqId, approved)}
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
                        <pre class="text-xs bg-secondary border border-border rounded-md p-2.5 overflow-x-auto whitespace-pre-wrap break-all max-h-[60vh]">
                          {file().content}
                        </pre>
                      </Show>
                    </div>
                  )}
                </Show>
              </div>
            </div>
          </Show>
        </div>
      </DialogContent>
    </Dialog>
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
