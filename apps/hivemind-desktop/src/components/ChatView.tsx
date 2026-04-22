import { For, Show, createEffect, createMemo, createSignal, onCleanup, createResource } from 'solid-js';
import type { Accessor, Setter } from 'solid-js';
import { useTimerCleanup } from '~/lib/useTimerCleanup';
import { authFetch } from '~/lib/authFetch';
import SpatialCanvas from '../SpatialCanvas';
import type { ActivityItem } from '../stores/streamingStore';
import type {
  Persona,
  ChatSessionSnapshot,
  ChatRunState,
  MessageAttachment,
  PromptInjectionReview,
  ToolDefinition,
  InstalledSkill,
  WorkflowDefinitionSummary,
  WorkflowInstanceSummary,
  McpToolInfo,
} from '../types';
import { formatTime, renderMarkdown, riskClass, statusClass } from '../utils';
import { CheckCircle, XCircle, Radio, Brain, Lock, Wrench, Tag, Target, MessageCircle, Shield, ClipboardList, MessageSquare, Pause, Square, Play, Hourglass, RefreshCw, Settings, Paperclip, Zap, BarChart3, ChevronDown, ChevronRight, GitBranch, BookOpen, Info, FileText } from 'lucide-solid';
import InlineQuestion, { AnsweredQuestion, type PendingQuestion } from './InlineQuestion';
import SessionConfigDialog from './SessionConfigDialog';
import { highlightYaml } from './YamlHighlight';
import { Dialog, DialogBody, DialogContent, DialogFooter, Button, Badge, Card, CardContent, Separator, Tooltip, TooltipTrigger, TooltipContent } from '~/ui';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import WorkflowLauncher, { extractManualTriggers } from './shared/WorkflowLauncher';
import PromptParameterDialog from './shared/PromptParameterDialog';
import McpAppView from './McpAppView';

export interface ChatWorkflowEvent {
  topic: string;
  payload: any;
  timestamp_ms: number;
}

export interface ChatViewProps {
  session: Accessor<ChatSessionSnapshot | null>;
  activeSessionState: Accessor<ChatRunState | null>;
  queueCount: Accessor<number>;
  showDiagnostics: Accessor<boolean>;
  setShowDiagnostics: Setter<boolean>;
  showMemoriesDialog: Accessor<boolean>;
  setShowMemoriesDialog: Setter<boolean>;
  chatFontPx: () => string;
  expandedMsgIds: Accessor<Set<string>>;
  setExpandedMsgIds: Setter<Set<string>>;
  streamingContent: Accessor<string>;
  isStreaming: Accessor<boolean>;
  activities: Accessor<ActivityItem[]>;
  toolCallHistory: Accessor<Record<string, Array<{
    id: string;
    tool_id: string;
    label: string;
    input?: string;
    output?: string;
    isError: boolean;
    startedAt: number;
    completedAt?: number;
  }>>>;
  pendingReview: Accessor<PromptInjectionReview | null>;
  busyAction: Accessor<string | null>;
  daemonOnline: Accessor<boolean>;
  draft: Accessor<string>;
  setDraft: Setter<string>;
  pendingAttachments: Accessor<MessageAttachment[]>;
  setPendingAttachments: Setter<MessageAttachment[]>;
  personas: Accessor<Persona[]>;
  selectedAgentId: Accessor<string>;
  setSelectedAgentId: Setter<string>;
  tools: Accessor<ToolDefinition[]>;
  installedSkills: Accessor<InstalledSkill[]>;
  selectedDataClass: Accessor<string>;
  setSelectedDataClass: Setter<string>;
  excludedTools: Accessor<string[]>;
  setExcludedTools: Setter<string[]>;
  excludedSkills: Accessor<string[]>;
  setExcludedSkills: Setter<string[]>;
  selectedSessionId: Accessor<string | null>;
  sendMessage: (decision?: any, options?: { skipPreempt?: boolean }) => Promise<void>;
  uploadFiles: () => Promise<void>;

  interrupt: (mode: 'soft' | 'hard') => Promise<void>;
  resume: () => Promise<void>;
  loadSessionPerms: () => Promise<void>;
  setShowSessionPermsDialog: Setter<boolean>;
  setShowSettings: (open: boolean) => void;
  setSettingsTab: Setter<any>;
  loadEditConfig: () => Promise<void>;
  loadToolDefinitions: () => Promise<void>;
  // Entity type — controls which features are visible
  entityType?: 'session' | 'bot';
  // When true, the compose area is hidden (e.g. bot in "done" status)
  readOnly?: boolean;
  // Spatial canvas props
  onSpatialSendMessage: (content: string, position: any) => void;
  // Inline question props
  allQuestions: Accessor<(PendingQuestion & { answer?: string })[]>;
  onQuestionAnswered: (request_id: string, answerText: string) => void;
  // Chat workflow props
  chatWorkflowDefinitions: Accessor<WorkflowDefinitionSummary[]>;
  chatWorkflows: Accessor<{ instanceId: number; instance: any | null; events: ChatWorkflowEvent[] }[]>;
  activeChatWorkflows: Accessor<{ instanceId: number; instance: any; events: ChatWorkflowEvent[] }[]>;
  terminalChatWorkflows: Accessor<{ instanceId: number; instance: any; events: ChatWorkflowEvent[] }[]>;
  onLaunchChatWorkflow: (definition: string, inputs: any, triggerStepId?: string) => Promise<number | null>;
  onPauseChatWorkflow: (instanceId: number) => void;
  onResumeChatWorkflow: (instanceId: number) => void;
  onKillChatWorkflow: (instanceId: number) => void;
  onRespondWorkflowGate: (instanceId: number, stepId: string, response: any) => Promise<void>;
  fetchParsedWorkflow: (name: string) => Promise<{ definition: any } | null>;
  // @-mention workspace file references
  workspaceFiles?: Accessor<any[]>;
  // MCP App integration: map of "serverId::toolName" → McpToolInfo for tools with UI
  mcpAppTools?: Accessor<Map<string, import('../types').McpToolInfo & { server_id: string }>>;
  mcpAppHtmlCache?: Accessor<Map<string, string>>;
  setMcpAppHtmlCache?: Setter<Map<string, string>>;
  daemonUrl?: Accessor<string | undefined>;
}

const ChatView = (props: ChatViewProps) => {
  const { safeTimeout } = useTimerCleanup();
  let messageListRef: HTMLDivElement | undefined;
  let textareaRef: HTMLTextAreaElement | undefined;
  const [userAtBottom, setUserAtBottom] = createSignal(true);
  const [showConfigDialog, setShowConfigDialog] = createSignal(false);
  const [expandedToolMsgs, setExpandedToolMsgs] = createSignal<Set<string>>(new Set());
  // Tool call whose details are shown in the popup overlay.
  const [popupToolCall, setPopupToolCall] = createSignal<{
    id: string; tool_id?: string; label: string; input?: string; output?: string; isError: boolean;
    startedAt: number; completedAt?: number; mcpRaw?: unknown;
  } | null>(null);
  const [expandedNotifications, setExpandedNotifications] = createSignal<Set<string>>(new Set());
  const [isDragging, setIsDragging] = createSignal(false);
  let attachmentIdCounter = 0;

  // ── @-mention workspace file references ──
  const [cursorPos, setCursorPos] = createSignal(0);
  const [atMentionIndex, setAtMentionIndex] = createSignal(0);

  /** Recursively flatten the workspace file tree into a list of file entries (not dirs). */
  const flatWorkspaceFiles = createMemo(() => {
    const tree = props.workspaceFiles?.() ?? [];
    const result: { name: string; path: string }[] = [];
    const walk = (entries: any[]) => {
      for (const entry of entries) {
        if (!entry.is_dir) {
          result.push({ name: entry.name, path: entry.path });
        }
        if (entry.children) walk(entry.children);
      }
    };
    walk(tree);
    return result;
  });

  /**
   * Detect @-mention trigger: look backward from cursor to find `@` preceded by
   * whitespace or start-of-string. Returns filtered file matches or null.
   */
  const atMentionMatches = createMemo(() => {
    const text = props.draft();
    const pos = cursorPos();
    if (!text || pos <= 0) return null;

    // Find the `@` that starts this mention (scan backward from cursor)
    let atIdx = -1;
    for (let i = pos - 1; i >= 0; i--) {
      const ch = text[i];
      if (ch === '@') {
        // `@` must be at start or preceded by whitespace
        if (i === 0 || /\s/.test(text[i - 1])) {
          atIdx = i;
        }
        break;
      }
      // Stop scanning if we hit whitespace or newline — no @ found
      if (/\s/.test(ch)) break;
    }
    if (atIdx < 0) return null;

    const query = text.slice(atIdx + 1, pos).toLowerCase();
    const files = flatWorkspaceFiles();
    if (files.length === 0) return null;

    const filtered = query
      ? files.filter((f) => f.path.toLowerCase().includes(query) || f.name.toLowerCase().includes(query))
      : files;

    return filtered.length > 0 ? { atIdx, query, matches: filtered.slice(0, 15) } : null;
  });

  // Reset selected index when matches change
  createEffect(() => {
    atMentionMatches();
    setAtMentionIndex(0);
  });

  /** Insert a file reference token into the draft, replacing the @query. */
  const insertAtMention = (filePath: string) => {
    const info = atMentionMatches();
    if (!info) return;
    const text = props.draft();
    const before = text.slice(0, info.atIdx);
    const after = text.slice(cursorPos());
    const token = `@[${filePath}] `;
    const newDraft = before + token + after;
    props.setDraft(newDraft);
    const newPos = before.length + token.length;
    setCursorPos(newPos);
    // Restore focus and cursor position after SolidJS re-renders
    queueMicrotask(() => {
      if (textareaRef) {
        textareaRef.focus();
        textareaRef.setSelectionRange(newPos, newPos);
      }
    });
  };

  // ── Message collapse state ──
  const [collapsedMsgIds, setCollapsedMsgIds] = createSignal<Set<string>>(new Set());
  const toggleCollapsed = (id: string) => {
    const s = new Set(collapsedMsgIds());
    if (s.has(id)) s.delete(id); else s.add(id);
    setCollapsedMsgIds(s);
  };

  // ── Chat workflow status bar collapse state ──
  const [workflowBarsExpanded, setWorkflowBarsExpanded] = createSignal(false);

  // ── Chat workflow launcher state ──
  const [showWfLauncher, setShowWfLauncher] = createSignal(false);
  const [wfLaunchValue, setWfLaunchValue] = createSignal<{ definition: string; inputs: any; trigger_step_id?: string } | null>(null);
  const [wfLaunching, setWfLaunching] = createSignal(false);

  // ── Workflow gate response dialog (triggered from workflow status bar) ──
  const [wfGateDialog, setWfGateDialog] = createSignal<{
    instanceId: number; stepId: string; prompt: string;
    choices: string[]; allowFreeform: boolean; requestId: string;
  } | null>(null);
  const [wfGateText, setWfGateText] = createSignal('');

  // Clear the workflow gate dialog when switching sessions
  createEffect(() => {
    props.selectedSessionId();
    setWfGateDialog(null);
    setWfGateText('');
  });

  // ── Prompt template picker state ──
  const [showPromptPicker, setShowPromptPicker] = createSignal(false);
  const [activePromptTemplate, setActivePromptTemplate] = createSignal<{ template: import('../types').PromptTemplate; persona_id: string } | null>(null);

  const currentPersonaPrompts = createMemo(() => {
    const persona = props.personas().find((p) => p.id === props.selectedAgentId()) ?? props.personas().find((p) => p.id === 'system/general');
    return persona?.prompts ?? [];
  });

  // Slash command: detect /prompt or /p at the start of the draft
  const slashMatches = createMemo(() => {
    const text = props.draft().trimStart();
    const match = text.match(/^\/(?:prompt|p)\s*(.*)/i);
    if (!match) return null;
    const filter = match[1]?.toLowerCase() ?? '';
    const prompts = currentPersonaPrompts();
    if (prompts.length === 0) return null;
    const filtered = filter
      ? prompts.filter((p) => p.name.toLowerCase().includes(filter) || p.id.toLowerCase().includes(filter))
      : prompts;
    return filtered.length > 0 ? filtered : null;
  });

  // Slash command: detect /workflow or /wf at the start of the draft
  const slashWorkflowMatches = createMemo(() => {
    const text = props.draft().trimStart();
    const match = text.match(/^\/(?:workflow|wf)\s*(.*)/i);
    if (!match) return null;
    const filter = match[1]?.toLowerCase() ?? '';
    const defs = props.chatWorkflowDefinitions();
    if (defs.length === 0) return null;
    const filtered = filter
      ? defs.filter((d) => d.name.toLowerCase().includes(filter) || (d.description ?? '').toLowerCase().includes(filter))
      : defs;
    return filtered.length > 0 ? filtered : null;
  });

  async function handleLaunchChatWorkflow() {
    const val = wfLaunchValue();
    if (!val?.definition) return;
    setWfLaunching(true);
    try {
      const id = await props.onLaunchChatWorkflow(val.definition, val.inputs ?? {}, val.trigger_step_id);
      if (id) {
        setShowWfLauncher(false);
        setWfLaunchValue(null);
      }
    } finally {
      setWfLaunching(false);
    }
  }

  // Helper: elapsed time for a workflow instance
  function wfElapsedStr(inst: any): string {
    if (!inst) return '';
    const start = inst.created_at_ms;
    const end = inst.completed_at_ms ?? Date.now();
    const secs = Math.floor((end - start) / 1000);
    if (secs < 60) return `${secs}s`;
    const mins = Math.floor(secs / 60);
    const remSecs = secs % 60;
    return `${mins}m ${remSecs}s`;
  }

  const ACCEPTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
  const MAX_IMAGE_BYTES = 10 * 1024 * 1024; // 10 MB

  const activeReaders = new Set<FileReader>();
  onCleanup(() => {
    for (const r of activeReaders) {
      r.onload = null;
      r.abort();
    }
    activeReaders.clear();
  });

  const addImageFiles = (files: FileList | File[]) => {
    for (const file of Array.from(files)) {
      if (!ACCEPTED_IMAGE_TYPES.includes(file.type)) continue;
      if (file.size > MAX_IMAGE_BYTES) continue;
      const reader = new FileReader();
      activeReaders.add(reader);
      reader.onload = () => {
        activeReaders.delete(reader);
        const result = reader.result;
        if (typeof result !== 'string') return;
        const base64 = result.split(',', 2)[1] ?? '';
        const att: MessageAttachment = {
          id: `att-${attachmentIdCounter++}`,
          filename: file.name,
          media_type: file.type,
          data: base64,
        };
        props.setPendingAttachments((prev) => [...prev, att]);
      };
      reader.readAsDataURL(file);
    }
  };

  const removeAttachment = (id: string) => {
    props.setPendingAttachments((prev) => prev.filter((a) => a.id !== id));
  };

  const handleDragOver = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(true);
  };

  const handleDragLeave = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);
  };

  const handleDrop = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);
    if (e.dataTransfer?.files?.length) {
      addImageFiles(e.dataTransfer.files);
    }
  };

  const handlePaste = (e: ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (const item of Array.from(items)) {
      if (item.kind === 'file' && ACCEPTED_IMAGE_TYPES.includes(item.type)) {
        const file = item.getAsFile();
        if (file) imageFiles.push(file);
      }
    }
    if (imageFiles.length > 0) {
      e.preventDefault();
      addImageFiles(imageFiles);
    }
  };

  const currentModel = createMemo(() => {
    const modelAct = props.activities().find(a => a.kind === 'model' && !a.done);
    if (modelAct) return modelAct.label;
    const last = props.activities().filter(a => a.kind === 'model').pop();
    return last?.label ?? null;
  });

  const activeActivity = createMemo(() => {
    return props.activities().filter(a => a.kind !== 'model' && !a.done).pop() ?? null;
  });

  const currentPersona = createMemo(() =>
    props.personas().find((p) => p.id === props.selectedAgentId())
  );

  const checkIfAtBottom = () => {
    if (!messageListRef) return;
    const threshold = 60;
    const { scrollTop, scrollHeight, clientHeight } = messageListRef;
    setUserAtBottom(scrollHeight - scrollTop - clientHeight < threshold);
  };

  const scrollToBottom = () => {
    if (messageListRef && userAtBottom()) {
      messageListRef.scrollTop = messageListRef.scrollHeight;
    }
  };

  createEffect(() => {
    const s = props.session();
    const _msgs = s?.messages?.length;
    const _stream = props.streamingContent();
    const _activities = props.activities();
    const _state = s?.state;
    const _questions = props.allQuestions().length;
    queueMicrotask(scrollToBottom);
  });

  const activeSession = () => props.session()!;

  // Cache previous timeline items by key so SolidJS <For> sees stable
  // references and doesn't recreate components (which would destroy
  // in-progress form state like half-typed question answers).
  let _prevTimelineByKey = new Map<string, any>();

  const chatTimeline = createMemo(() => {
    const items: Array<
      | { kind: 'message'; message: any; ts: number }
      | { kind: 'question'; message: any; ts: number }
      | { kind: 'answered-question'; message: any; ts: number }
      | { kind: 'answered'; question: PendingQuestion & { answer?: string }; ts: number }
      | { kind: 'workflow-result'; wf: { instanceId: number; instance: any; events: ChatWorkflowEvent[] }; ts: number }
    > = [];
    for (const m of (activeSession().messages ?? [])) {
      // Skip notification messages from workflows — the workflow bubble renders instead
      if (m.role === 'notification' && m.provider_id?.startsWith('workflow:')) continue;
      // Also skip legacy system messages from the old workflow-result injection path
      if (m.role === 'system' && m.provider_id?.startsWith('workflow:')) continue;
      // Question messages (from interaction gates) render as interactive elements
      if (m.interaction_request_id && m.interaction_kind === 'question') {
        if (m.interaction_answer) {
          items.push({ kind: 'answered-question', message: m, ts: m.created_at_ms });
        } else {
          items.push({ kind: 'question', message: m, ts: m.created_at_ms });
        }
        continue;
      }
      items.push({ kind: 'message', message: m, ts: m.created_at_ms });
    }
    for (const q of props.allQuestions().filter((q) => q.answer && q.session_id === props.selectedSessionId())) {
      // Only show legacy answered questions that don't have a corresponding message
      const hasMessage = (activeSession().messages ?? []).some(
        (m: any) => m.interaction_request_id === q.request_id
      );
      if (!hasMessage) {
        items.push({ kind: 'answered', question: q, ts: q.timestamp });
      }
    }
    for (const wf of props.terminalChatWorkflows()) {
      // Only show workflows belonging to the current session
      if (wf.instance?.parent_session_id !== props.selectedSessionId()) continue;
      const ts = wf.instance?.completed_at_ms ?? wf.instance?.updated_at_ms ?? 0;
      items.push({ kind: 'workflow-result', wf, ts });
    }
    items.sort((a, b) => a.ts - b.ts);

    // Reuse previous object references when the item hasn't materially
    // changed so that <For> keeps existing component instances alive.
    const nextCache = new Map<string, any>();
    const stable = items.map((item) => {
      const key =
        'message' in item && item.message?.id ? `${item.kind}:${item.message.id}` :
        'question' in item && (item as any).question?.request_id ? `q:${(item as any).question.request_id}` :
        'wf' in item ? `wf:${(item as any).wf.instanceId}` :
        `${item.kind}:${item.ts}`;
      const prev = _prevTimelineByKey.get(key);
      // For unanswered questions, reuse the cached item as long as the
      // question hasn't been answered (kind stays 'question').
      if (prev && prev.kind === item.kind && prev.ts === item.ts) {
        nextCache.set(key, prev);
        return prev;
      }
      nextCache.set(key, item);
      return item;
    });
    _prevTimelineByKey = nextCache;
    return stable;
  });

  return (
    <>
    <Show
      when={activeSession().modality === 'spatial'}
      fallback={
        <div class="grid h-full grid-rows-[1fr_auto] overflow-hidden">
          <div class="overflow-y-auto" ref={messageListRef} onScroll={checkIfAtBottom}>
          <Show when={props.showDiagnostics()}>
            <section class="flex gap-2 rounded-lg border border-border bg-muted/30 p-2">
              <div class="flex-1 rounded bg-card p-2 text-xs">
                <span class="text-muted-foreground">Stage</span>
                <strong class="ml-1 text-foreground">{activeSession().active_stage ?? 'idle'}</strong>
              </div>
              <div class="flex-1 rounded bg-card p-2 text-xs">
                <span class="text-muted-foreground">Intent</span>
                <strong class="ml-1 text-foreground">{activeSession().active_intent ?? 'Waiting for work'}</strong>
              </div>
              <div class="flex-[2] rounded bg-card p-2 text-xs">
                <span class="text-muted-foreground">Thinking</span>
                <strong class="ml-1 text-foreground">{activeSession().active_thinking ?? 'No active reasoning trace.'}</strong>
              </div>
            </section>
          </Show>

          <Dialog
            open={props.showMemoriesDialog() && activeSession().recalled_memories.length > 0}
            onOpenChange={(open) => { if (!open) props.setShowMemoriesDialog(false); }}
          >
            <DialogContent class="max-w-xl min-w-[400px] max-h-[70vh] flex flex-col overflow-hidden rounded-xl bg-background p-6">
              <header class="mb-3 flex items-center justify-between gap-2">
                <h3 class="m-0 text-foreground"><Brain size={14} /> Recalled Memories</h3>
                <span class="inline-flex items-center rounded-full bg-muted px-2 py-0.5 text-xs text-muted-foreground">{activeSession().recalled_memories.length} items</span>
              </header>
              <DialogBody class="space-y-2">
                <For each={activeSession().recalled_memories}>
                  {(memory) => (
                    <article class="rounded-lg border border-border bg-card p-3">
                      <header>
                        <strong>{memory.name}</strong>
                        <span class="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-xs font-medium">{memory.data_class}</span>
                      </header>
                      <p>{memory.content ?? 'No stored content.'}</p>
                    </article>
                  )}
                </For>
              </DialogBody>
              <DialogFooter class="mt-3">
                <Button variant="outline" onClick={() => props.setShowMemoriesDialog(false)}>Close</Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          <section class="flex flex-col gap-3 px-4" style={`font-size:${props.chatFontPx()}`}>
            <Show
              when={activeSession().messages.length > 0 || props.terminalChatWorkflows().length > 0}
              fallback={<p class="text-center text-muted-foreground py-8">No messages yet. Send the first command below.</p>}
            >
              {/* Unified chronological stream: messages + answered questions + workflow results interleaved by timestamp */}
              <For each={chatTimeline()}>
                {(item) => {
                  if (item.kind === 'question') {
                    // Unanswered question message — render as interactive InlineQuestion
                    const m = item.message;
                    const meta = m.interaction_meta ?? {};
                    const q: PendingQuestion = {
                      request_id: m.interaction_request_id!,
                      text: m.content,
                      choices: meta.choices ?? [],
                      allow_freeform: meta.allow_freeform !== false,
                      multi_select: meta.multi_select === true,
                      session_id: props.selectedSessionId() ?? undefined,
                      agent_id: meta.agent_id,
                      agent_name: meta.agent_name,
                      timestamp: m.created_at_ms,
                      message: meta.message ?? undefined,
                      workflow_instance_id: meta.workflow_instance_id ?? undefined,
                      workflow_step_id: meta.workflow_step_id ?? undefined,
                    };
                    return (
                      <InlineQuestion
                        question={q}
                        session_id={props.selectedSessionId()!}
                        onAnswered={props.onQuestionAnswered}
                      />
                    );
                  }
                  if (item.kind === 'answered-question') {
                    // Answered question message — render as AnsweredQuestion
                    const m = item.message;
                    const meta = m.interaction_meta ?? {};
                    const q: PendingQuestion = {
                      request_id: m.interaction_request_id!,
                      text: m.content,
                      choices: meta.choices ?? [],
                      allow_freeform: meta.allow_freeform !== false,
                      timestamp: m.created_at_ms,
                      agent_name: meta.agent_name,
                    };
                    return <AnsweredQuestion question={q} answer={m.interaction_answer!} />;
                  }
                  if (item.kind === 'answered') {
                    return <AnsweredQuestion question={item.question} answer={item.question.answer!} />;
                  }
                  if (item.kind === 'workflow-result') {
                    const wf = item.wf;
                    const inst = () => wf.instance;
                    const status = () => inst()?.status ?? 'completed';
                    const isCompleted = () => status() === 'completed';
                    const isFailed = () => status() === 'failed';
                    const icon = () => isCompleted() ? <CheckCircle size={14} /> : isFailed() ? <XCircle size={14} /> : <Square size={14} />;
                    const statusLabel = () => isCompleted() ? 'Completed' : isFailed() ? 'Failed' : 'Killed';
                    const resultMsg = () => inst()?.resolved_result_message;
                    return (
                      <article class={`rounded-lg border border-l-[3px] p-4 ${isCompleted() ? 'border-emerald-500/30 border-l-emerald-500 bg-emerald-400/5' : isFailed() ? 'border-red-500/30 border-l-red-500 bg-red-400/5' : 'border-border border-l-muted-foreground bg-muted/30'}`}>
                        <div class="flex items-center gap-2">
                          <span class={isCompleted() ? 'text-emerald-400' : isFailed() ? 'text-red-400' : 'text-muted-foreground'}>{icon()}</span>
                          <span class="text-foreground">
                            <GitBranch size={14} class="inline align-middle mr-1" />
                            <strong>{inst()?.definition_name ?? 'Workflow'}</strong>: {statusLabel()}
                          </span>
                          <Show when={inst()?.step_count}>
                            <span class="ml-2 text-xs text-muted-foreground">
                              {inst()!.steps_completed ?? 0}/{inst()!.step_count} steps
                            </span>
                          </Show>
                          <span class="ml-2 text-xs text-muted-foreground">
                            {wfElapsedStr(inst())}
                          </span>
                        </div>
                        <Show when={resultMsg()}>
                          <div class="prose max-w-none pl-7 pt-1.5 text-foreground markdown-body text-[inherit]" innerHTML={renderMarkdown(resultMsg()!)} />
                        </Show>
                        <Show when={isFailed() && inst()?.error}>
                          <p class="mt-1 pl-7 text-xs text-destructive">{inst()!.error}</p>
                        </Show>
                      </article>
                    );
                  }
                  const message = item.message;
                  const hasDiagnostics = () => !!(message.provider_id || message.data_class || message.intent || message.thinking || message.scan_summary || message.classification_reason);
                  const msgKey = () => message.id ?? `${message.created_at_ms}-${message.role}`;
                  const isExpanded = () => props.expandedMsgIds().has(msgKey());
                  const toggleExpanded = () => {
                    const s = new Set(props.expandedMsgIds());
                    if (s.has(msgKey())) s.delete(msgKey()); else s.add(msgKey());
                    props.setExpandedMsgIds(s);
                  };
                  const isAgentMsg = () => message.provider_id?.startsWith('agent:');
                  const roleColors: Record<string, { border: string; bg: string; badge: string }> = {
                    user:      { border: 'border-sky-500/30 border-l-sky-500',             bg: '',                                badge: 'bg-sky-700 text-white' },
                    assistant: { border: 'border-emerald-500/30 border-l-emerald-500',     bg: '',                                badge: 'bg-emerald-700 text-white' },
                    system:    { border: 'border-red-500/30 border-l-red-500',             bg: '',                                badge: 'bg-red-600 text-white' },
                    tool:      { border: 'border-border border-l-muted-foreground',        bg: '',                                badge: 'bg-zinc-600 text-white' },
                  };
                  const agentStyle = { border: 'border-orange-500/30 border-l-orange-500', bg: 'bg-orange-400/5', badge: 'bg-orange-600 text-white' };
                  const roleStyle = () => isAgentMsg() ? agentStyle : (roleColors[message.role] ?? { border: 'border-border border-l-border', bg: '', badge: 'bg-zinc-600 text-white' });
                  const cardClass = () => {
                    const s = roleStyle();
                    return `rounded-lg border border-l-[3px] ${s.border} ${s.bg} bg-card/80 p-4 text-foreground`;
                  };
                  const badgeClass = () => `inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium ${roleStyle().badge}`;
                  return (
                    <article class={cardClass()}>
                      <header class="mb-2 flex items-center gap-2 text-xs">
                        <span class={badgeClass()}>
                          {isAgentMsg() ? (message.model || message.provider_id?.replace('agent:', '') || 'agent') : message.role}
                        </span>
                        <span class="text-xs text-muted-foreground">{formatTime(message.created_at_ms)}</span>
                        <span class="ml-auto flex items-center gap-0.5">
                        <Show when={hasDiagnostics()}>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="size-8"
                          title={isExpanded() ? 'Hide diagnostics' : 'Show diagnostics'}
                          aria-label={isExpanded() ? 'Hide diagnostics' : 'Show diagnostics'}
                          onClick={toggleExpanded}
                        >
                          <Info size={12} />
                        </Button>
                        </Show>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="size-8"
                          title="Copy message"
                          aria-label="Copy message"
                          onClick={() => {
                            navigator.clipboard.writeText(message.content);
                            const btn = document.querySelector(`[data-copy-id="${message.id}"]`);
                            if (btn) { btn.textContent = '✓'; safeTimeout(() => { btn.textContent = '⧉'; }, 1200); }
                          }}
                          data-copy-id={message.id}
                        >⧉</Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="size-8 ml-0.5"
                          title={collapsedMsgIds().has(message.id) ? 'Expand message' : 'Collapse message'}
                          aria-label={collapsedMsgIds().has(message.id) ? 'Expand message' : 'Collapse message'}
                          onClick={() => toggleCollapsed(message.id)}
                        >
                          {collapsedMsgIds().has(message.id) ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                        </Button>
                        </span>
                      </header>

                      <Show when={!collapsedMsgIds().has(message.id)}>
                      <Show
                        when={message.role === 'system' && message.content.startsWith('[MCP Notification]')}
                        fallback={
                      <Show
                        when={message.role === 'system' && message.content.startsWith('[Scheduler]')}
                        fallback={
                      <Show
                        when={message.content.includes('denied by policy')}
                        fallback={
                          <Show
                            when={message.role === 'assistant' || message.provider_id?.startsWith('agent:')}
                            fallback={
                              <div>
                                <p class="text-foreground">{message.content}</p>
                                <Show when={message.attachments?.length > 0}>
                                  <div class="mt-2 flex flex-wrap gap-2">
                                    <For each={message.attachments}>
                                      {(att) => (
                                        <img
                                          class="max-h-40 rounded border border-border"
                                          src={`data:${att.media_type};base64,${att.data}`}
                                          alt={att.filename ?? 'attachment'}
                                        />
                                      )}
                                    </For>
                                  </div>
                                </Show>
                              </div>
                            }
                          >
                            <div class="prose max-w-none text-foreground markdown-body text-[inherit]" innerHTML={renderMarkdown(message.content)} />
                          </Show>
                        }
                      >
                        <div class="border-l-[3px] border-l-destructive px-3 py-2 bg-red-500/5 rounded">
                          <p class="m-0 mb-1 text-destructive font-semibold"><Lock size={14} /> Tool Access Denied</p>
                          <p class="m-0 text-muted-foreground">{message.content}</p>
                          <Button variant="ghost" size="sm" onClick={() => { props.setShowSettings(true); props.setSettingsTab('tools'); void props.loadEditConfig(); void props.loadToolDefinitions(); }} class="mt-2 text-xs px-2 py-1 rounded border border-border bg-transparent text-muted-foreground cursor-pointer">
                            View Tools Settings →
                          </Button>
                        </div>
                      </Show>
                        }
                      >
                        {(() => {
                          // Parse: [Scheduler] ✅ Task **name** (id): status\nError: ...
                          const isSuccess = message.content.includes('✅');
                          const taskMatch = message.content.match(/Task \*\*([^*]+)\*\* \(([^)]+)\): (\w+)/);
                          const taskName = taskMatch ? taskMatch[1] : 'unknown';
                          const taskId = taskMatch ? taskMatch[2] : '';
                          const status = taskMatch ? taskMatch[3] : 'unknown';
                          const errorMatch = message.content.match(/Error: (.+)/);
                          const errorMsg = errorMatch ? errorMatch[1] : null;
                          const icon = isSuccess ? <CheckCircle size={14} /> : <XCircle size={14} />;
                          const borderClass = isSuccess ? 'border-emerald-500/30 border-l-emerald-500 bg-emerald-400/5' : 'border-red-500/30 border-l-red-500 bg-red-500/5';
                          return (
                            <div class={`rounded-md p-3 border border-l-[3px] ${borderClass}`}>
                              <div class="flex items-center gap-2">
                                <span>{icon}</span>
                                <span class="text-foreground">
                                  Scheduled task <strong>{taskName}</strong>: {status}
                                </span>
                              </div>
                              <Show when={errorMsg}>
                                <p class="mt-1 ml-7 text-destructive">{errorMsg}</p>
                              </Show>
                              <Show when={taskId}>
                                <p class="mt-0.5 ml-7 text-[0.75em] text-muted-foreground">Task ID: {taskId}</p>
                              </Show>
                            </div>
                          );
                        })()}
                      </Show>
                        }
                      >
                        {(() => {
                          const serverMatch = message.content.match(/Server '([^']+)'/);
                          const serverName = serverMatch ? serverMatch[1] : 'unknown';
                          const kindMatch = message.content.match(/Server '[^']+': ([^.]+)/);
                          const kindLabel = kindMatch ? kindMatch[1] : 'notification';
                          const isNotifExpanded = () => expandedNotifications().has(message.id);
                          const toggleNotif = () => {
                            const s = new Set(expandedNotifications());
                            if (s.has(message.id)) s.delete(message.id); else s.add(message.id);
                            setExpandedNotifications(s);
                          };
                          return (
                            <div class="rounded-md border border-border bg-muted/30 p-3">
                              <div
                                class="flex cursor-pointer items-center gap-2"
                                role="button"
                                tabIndex={0}
                                onClick={toggleNotif}
                                onKeyDown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleNotif(); } }}
                              >
                                <span class="text-muted-foreground"><Radio size={14} /></span>
                                <span class="flex-1 text-foreground">
                                  Notification received from <strong>{serverName}</strong>: {kindLabel}
                                </span>
                                <span class="text-xs text-muted-foreground">{isNotifExpanded() ? '▾' : '▸'}</span>
                              </div>
                              <Show when={isNotifExpanded()}>
                                <pre class="mt-2 overflow-auto rounded bg-muted p-2 text-xs">{message.content}</pre>
                              </Show>
                            </div>
                          );
                        })()}
                      </Show>

                      </Show>{/* end collapse wrapper */}

                      {/* Footer toggles: tool calls */}
                      <Show when={message.role === 'assistant' && (props.toolCallHistory()[message.id] ?? []).length > 0}>
                        <div class="mt-1 flex items-center gap-1 border-t border-border/50 pt-1">
                          {/* Tool call history */}
                          {(() => {
                              const calls = () => props.toolCallHistory()[message.id] ?? [];
                              const isToolExpanded = () => expandedToolMsgs().has(message.id);
                              const toggleTools = () => {
                                const s = new Set(expandedToolMsgs());
                                if (s.has(message.id)) s.delete(message.id); else s.add(message.id);
                                setExpandedToolMsgs(s);
                              };
                              return (
                                <Button variant="ghost" size="sm" onClick={toggleTools}>
                                  <span>{isToolExpanded() ? '▾' : '▸'}</span>
                                  <span><Wrench size={14} /> {calls().length} tool call{calls().length !== 1 ? 's' : ''}</span>
                                </Button>
                              );
                          })()}
                        </div>

                        {/* Expanded tool call list */}
                        <Show when={message.role === 'assistant' && expandedToolMsgs().has(message.id) && (props.toolCallHistory()[message.id] ?? []).length > 0}>
                          <ul class="mt-0.5 space-y-0.5 pl-2">
                            <For each={props.toolCallHistory()[message.id] ?? []}>
                              {(tc) => (
                                <li class={`flex cursor-pointer items-center gap-2 rounded px-2 py-0.5 text-xs hover:bg-muted/50 ${tc.isError ? 'text-destructive' : 'text-muted-foreground'}`}>
                                  <div
                                    class="flex flex-1 items-center gap-2"
                                    role="button"
                                    tabIndex={0}
                                    onClick={() => setPopupToolCall(tc)}
                                    onKeyDown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setPopupToolCall(tc); } }}
                                  >
                                    <span>{tc.isError ? '✗' : '✓'}</span>
                                    <span class="flex-1">{tc.label}</span>
                                    <Show when={tc.completedAt && tc.startedAt}>
                                      <Badge variant="secondary" class="text-[10px]">{Math.max(1, Math.round(((tc.completedAt ?? 0) - tc.startedAt) / 1000))}s</Badge>
                                    </Show>
                                  </div>
                                </li>
                              )}
                            </For>
                          </ul>
                        </Show>

                        {/* Inline MCP App views — auto-rendered for tools with UI resources */}
                        <Show when={props.mcpAppTools && props.daemonUrl}>
                          <For each={(props.toolCallHistory()[message.id] ?? []).filter(tc => {
                            const match = tc.tool_id.match(/^mcp\.(.+?)\.(.+)$/);
                            if (!match) return false;
                            return props.mcpAppTools!().has(`${match[1]}::${match[2]}`);
                          })}>
                            {(tc) => {
                              const match = tc.tool_id.match(/^mcp\.(.+?)\.(.+)$/)!;
                              const serverId = match[1];
                              const toolName = match[2];
                              const mcpTool = () => props.mcpAppTools!().get(`${serverId}::${toolName}`);
                              const [appHtml, setAppHtml] = createSignal<string | null>(null);

                              createEffect(() => {
                                const uri = mcpTool()?.ui_meta?.resource_uri;
                                const daemonUrl = props.daemonUrl?.();
                                if (!uri || !daemonUrl) return;
                                // Check app-level cache first (survives session switches)
                                const cacheKey = `${serverId}::${uri}`;
                                const cached = props.mcpAppHtmlCache?.()?.get(cacheKey);
                                if (cached) { setAppHtml(cached); return; }
                                void (async () => {
                                  try {
                                    const resp = await authFetch(`${daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(serverId)}/fetch-ui-resource`, {
                                      method: 'POST',
                                      headers: { 'Content-Type': 'application/json' },
                                      body: JSON.stringify({ uri }),
                                    });
                                    if (resp.ok) {
                                      const resource = await resp.json() as { html: string; uri: string };
                                      setAppHtml(resource.html);
                                      // Populate cache for future navigations
                                      if (props.setMcpAppHtmlCache) {
                                        props.setMcpAppHtmlCache(prev => { const m = new Map(prev); m.set(cacheKey, resource.html); return m; });
                                      }
                                    }
                                  } catch (e) {
                                    console.warn('[MCP App] Failed to fetch UI resource:', e);
                                  }
                                })();
                              });

                              return (
                                <Show when={appHtml()}>
                                  {(html) => (
                                    <McpAppView
                                      html={html()}
                                      serverId={serverId}
                                      toolName={toolName}
                                      toolInput={tc.input}
                                      toolOutput={tc.output}
                                      toolIsError={tc.isError}
                                      toolResultRaw={tc.mcpRaw}
                                      toolInputSchema={mcpTool()?.input_schema as Record<string, unknown> | undefined}
                                      toolDescription={mcpTool()?.description}
                                      toolVisibility={mcpTool()?.ui_meta?.visibility ?? undefined}
                                      sessionId={props.selectedSessionId() ?? ''}
                                      daemonUrl={props.daemonUrl?.() ?? ''}
                                      uiMeta={mcpTool()?.ui_meta}
                                      theme="dark"
                                      displayMode="inline"
                                      onPopout={() => {
                                        setPopupToolCall({
                                          tool_id: tc.tool_id,
                                          label: tc.label,
                                          input: tc.input,
                                          output: tc.output,
                                          isError: tc.isError,
                                          startedAt: tc.startedAt,
                                          completedAt: tc.completedAt,
                                          mcpRaw: tc.mcpRaw,
                                        });
                                      }}
                                    />
                                  )}
                                </Show>
                              );
                            }}
                          </For>
                        </Show>
                      </Show>

                      {/* Expanded diagnostics */}
                      <Show when={isExpanded()}>
                        <div class="mt-1 space-y-0.5 rounded border border-border/50 bg-muted/30 px-2 py-1 text-xs">
                          <Show when={message.provider_id}>
                            <span class="inline-flex items-center gap-1 text-xs text-muted-foreground"><Tag size={14} /> {message.provider_id}<Show when={message.model}> / {message.model}</Show></span>
                          </Show>
                          <Show when={message.data_class}>
                            <span class="inline-flex items-center gap-1 text-xs text-muted-foreground"><Lock size={14} /> {message.data_class}</span>
                          </Show>
                          <Show when={message.intent}>
                            <span class="inline-flex items-center gap-1 text-xs text-muted-foreground"><Target size={14} /> {message.intent}</span>
                          </Show>
                          <Show when={message.thinking}>
                            <span class="inline-flex items-center gap-1 text-xs text-muted-foreground"><MessageCircle size={14} /> {message.thinking}</span>
                          </Show>
                          <Show when={message.scan_summary}>
                            {(value) => (
                              <span class={`inline-flex items-center gap-1 text-xs text-muted-foreground ${riskClass(value().verdict)}`}>
                                <Shield size={14} /> {value().verdict} ({Math.round(value().confidence * 100)}%)
                              </span>
                            )}
                          </Show>
                          <Show when={message.classification_reason}>
                            <span class="inline-flex items-center gap-1 text-xs text-muted-foreground"><ClipboardList size={14} /> {message.classification_reason}</span>
                          </Show>
                        </div>
                      </Show>
                    </article>
                  );
                }}
              </For>

              {/* Model indicator moved below composer */}
              <Show when={props.activeSessionState() === 'running' && !props.streamingContent() && props.activities().filter(a => !a.done).length === 0}>
                <div class="chat-spinner">
                  <span class="dot"></span>
                  <span class="dot"></span>
                  <span class="dot"></span>
                </div>
              </Show>
              {/* Activity feed moved below composer */}
              <Show when={props.isStreaming() && props.streamingContent()}>
                <article class="rounded-lg border border-emerald-500/30 border-l-[3px] border-l-emerald-500 bg-card/80 p-4 text-foreground">
                  <header class="mb-2">
                    <span class="inline-flex items-center gap-1 rounded-full bg-emerald-600 px-2 py-0.5 text-xs font-medium text-white">streaming</span>
                  </header>
                  <div class="prose max-w-none text-foreground markdown-body text-[inherit]" innerHTML={renderMarkdown(props.streamingContent())} />
                </article>
              </Show>

              {/* Pending questions from legacy path (no corresponding message) — fallback */}
              <Show when={props.selectedSessionId()}>
                <For each={props.allQuestions().filter((q) => {
                  if (q.answer) return false;
                  if (q.session_id !== props.selectedSessionId()) return false;
                  // Skip questions that already have a message in the timeline
                  const msgs = activeSession().messages ?? [];
                  return !msgs.some((m: any) => m.interaction_request_id === q.request_id);
                })}>
                  {(q) => (
                    <InlineQuestion
                      question={q}
                      session_id={props.selectedSessionId()!}
                      onAnswered={props.onQuestionAnswered}
                    />
                  )}
                </For>
              </Show>
            </Show>
          </section>

          <Show when={props.pendingReview()}>
            {(review) => (
              <section class="grid gap-3">
                <header class="flex items-center justify-between gap-2 mb-1">
                  <h3>Prompt injection review required</h3>
                  <span class={`pill ${riskClass(review().verdict)}`}>{review().verdict}</span>
                </header>
                <div class="rounded-lg border border-border bg-card p-3">
                  <p>{review().preview}</p>
                  <dl class="mt-2 space-y-2 text-xs text-muted-foreground">
                    <div>
                      <dt>Recommendation</dt>
                      <dd>{review().recommendation}</dd>
                    </div>
                    <div>
                      <dt>Confidence</dt>
                      <dd>{Math.round(review().confidence * 100)}%</dd>
                    </div>
                    <Show when={review().threat_type}>
                      {(value) => (
                        <div>
                          <dt>Threat type</dt>
                          <dd>{value()}</dd>
                        </div>
                      )}
                    </Show>
                  </dl>
                  <Show when={review().proposed_redaction}>
                    {(value) => (
                      <div class="memory-search-results">
                        <strong>Proposed redaction</strong>
                        <p>{value()}</p>
                      </div>
                    )}
                  </Show>
                  <div class="flex items-center justify-between gap-1 px-1 py-1">
                    <p class="muted">
                      Review the scan before allowing sensitive content onto a public path.
                    </p>
                    <div class="button-row">
                      <Button variant="destructive" disabled={props.busyAction() !== null} onClick={() => void props.sendMessage('block')}>
                        Block
                      </Button>
                      <Button
                        variant="outline"
                        disabled={props.busyAction() !== null || !review().proposed_redaction}
                        onClick={() => void props.sendMessage('redact')}
                      >
                        Send redacted
                      </Button>
                      <Button
                        disabled={props.busyAction() !== null}
                        onClick={() => void props.sendMessage('allow')}
                      >
                        Allow once
                      </Button>
                    </div>
                  </div>
                </div>
              </section>
            )}
          </Show>
          </div>{/* end scrollable area */}

          <section class="flex flex-col gap-0 border-t border-border bg-background p-2">
            <div class="flex items-center gap-2 px-2 py-1 text-xs text-muted-foreground">
              <Show when={currentModel()}>
                <span class="text-xs text-muted-foreground"><Brain size={14} /> {currentModel()}</span>
              </Show>
              <Show when={activeActivity()}>
                {(act) => (
                  <span class="activity-indicator">
                    <span class="activity-icon spinning">
                      {act().kind === 'tool' ? <Wrench size={14} /> :
                       act().kind === 'skill' ? <Zap size={14} /> :
                       act().kind === 'inference' ? <MessageCircle size={14} /> :
                       act().kind === 'feedback' ? <MessageSquare size={14} /> : <Settings size={14} />}
                    </span>
                    {act().label}
                  </span>
                )}
              </Show>
            </div>

            {/* ── Chat Workflow Status Widgets (collapsible when multiple) ── */}
            {(() => {
              const filteredWorkflows = () => props.activeChatWorkflows().filter(w => !w.instance?.parent_session_id || w.instance.parent_session_id === props.selectedSessionId());
              const wfCount = () => filteredWorkflows().length;

              const renderWorkflowBar = (wf: { instanceId: number; instance: any; events: ChatWorkflowEvent[] }) => {
                const inst = () => wf.instance;
                const iid = wf.instanceId;
                const status = () => inst()?.status ?? 'pending';
                const isRunning = () => ['running', 'pending'].includes(status());
                const isPaused = () => status() === 'paused';
                const isWaiting = () => ['waiting_on_input', 'waiting_on_event'].includes(status());

                const statusTextClass = () => {
                  switch (status()) {
                    case 'running': return 'text-blue-400';
                    case 'paused': return 'text-orange-400';
                    case 'waiting_on_input': case 'waiting_on_event': return 'text-yellow-300';
                    default: return 'text-foreground';
                  }
                };
                const statusBorderClass = () => {
                  switch (status()) {
                    case 'running': return 'border-b-blue-400';
                    case 'paused': return 'border-b-orange-400';
                    case 'waiting_on_input': case 'waiting_on_event': return 'border-b-yellow-300';
                    default: return 'border-b-foreground';
                  }
                };

                const statusLabel = () => {
                  switch (status()) {
                    case 'running': return '⟳ Running';
                    case 'pending': return '⟳ Starting';
                    case 'paused': return '⏸ Paused';
                    case 'waiting_on_input': return '💬 Waiting for input';
                    case 'waiting_on_event': return '⏳ Waiting for event';
                    default: return status();
                  }
                };

                return (
                  <div class={`flex flex-col px-2.5 py-1.5 gap-1 bg-background border-b-2 ${statusBorderClass()} text-sm`}>
                    <div class="flex items-center gap-2">
                      <span class={`${statusTextClass()} font-semibold ${(isRunning() || isWaiting()) ? 'animate-pulse' : ''}`}>
                        {statusLabel()}
                      </span>
                      <span class="text-muted-foreground text-sm">
                        <GitBranch size={14} /> {inst()?.definition_name ?? 'Workflow'}
                      </span>
                      <span class="text-muted-foreground text-sm">
                        {wfElapsedStr(inst())}
                      </span>
                      <Show when={inst()?.step_count}>
                        <span class="text-muted-foreground text-sm">
                          {inst()!.steps_completed ?? 0}/{inst()!.step_count} steps
                        </span>
                      </Show>
                      <Show when={isWaiting()}>
                        <button
                          class="inline-flex items-center gap-1 text-yellow-300 animate-pulse cursor-pointer hover:text-yellow-100 transition-colors bg-transparent border-none p-0 text-xs font-medium"
                          title="Click to respond to workflow gate"
                          onClick={(e) => {
                            e.stopPropagation();
                            // Find the latest pending question for this workflow instance
                            const allQ = props.allQuestions();
                            const q = allQ
                              .filter((q) => !q.answer && q.workflow_instance_id === iid)
                              .at(-1);
                            if (q) {
                              setWfGateDialog({
                                instanceId: iid,
                                stepId: q.workflow_step_id ?? '',
                                prompt: q.text,
                                choices: q.choices,
                                allowFreeform: q.allow_freeform,
                                requestId: q.request_id,
                              });
                              setWfGateText('');
                            } else {
                              // Fallback: find the gate from step_states on the workflow instance
                              const stepStates = inst()?.step_states as Record<string, any> | undefined;
                              if (stepStates) {
                                const waitingStep = Object.entries(stepStates)
                                  .filter(([, s]) => s.status === 'waiting_on_input')
                                  .at(-1);
                                if (waitingStep) {
                                  const stepState = waitingStep[1] as any;
                                  const rawChoices = stepState.interaction_choices;
                                  const choices = Array.isArray(rawChoices) ? rawChoices : (() => { try { return JSON.parse(rawChoices ?? '[]'); } catch { return []; } })();
                                  setWfGateDialog({
                                    instanceId: iid,
                                    stepId: waitingStep[0],
                                    prompt: stepState.interaction_prompt ?? 'Waiting for your response',
                                    choices,
                                    allowFreeform: stepState.interaction_allow_freeform !== false,
                                    requestId: stepState.interaction_request_id ?? `wf:${iid}:${waitingStep[0]}`,
                                  });
                                  setWfGateText('');
                                }
                              }
                            }
                          }}
                        >
                          <MessageSquare size={14} /> Respond
                        </button>
                      </Show>
                      <div class="ml-auto flex gap-1">
                        <Show when={isRunning()}>
                          <Button variant="ghost" size="icon" class="size-8 text-sm" onClick={() => props.onPauseChatWorkflow(iid)} title="Pause workflow" aria-label="Pause workflow"><Pause size={14} /></Button>
                          <Button variant="ghost" size="icon" class="size-8 text-sm" onClick={() => props.onKillChatWorkflow(iid)} title="Kill workflow" aria-label="Kill workflow"><Square size={14} /></Button>
                        </Show>
                        <Show when={isPaused()}>
                          <Button variant="ghost" size="icon" class="size-8 text-sm" onClick={() => props.onResumeChatWorkflow(iid)} title="Resume workflow" aria-label="Resume workflow"><Play size={14} /></Button>
                          <Button variant="ghost" size="icon" class="size-8 text-sm" onClick={() => props.onKillChatWorkflow(iid)} title="Kill workflow" aria-label="Kill workflow"><Square size={14} /></Button>
                        </Show>
                        <Show when={isWaiting()}>
                          <Button variant="ghost" size="icon" class="size-8 text-sm" onClick={() => props.onKillChatWorkflow(iid)} title="Kill workflow" aria-label="Kill workflow"><Square size={14} /></Button>
                        </Show>
                      </div>
                    </div>
                  </div>
                );
              };

              return (
                <>
                  {/* Summary bar shown when multiple workflows are active */}
                  <Show when={wfCount() > 1}>
                    <div
                      class="flex items-center gap-2 px-2.5 py-1.5 bg-background border-b-2 border-b-blue-400 text-sm cursor-pointer select-none"
                      onClick={() => setWorkflowBarsExpanded(prev => !prev)}
                    >
                      <span class="text-blue-400 font-semibold animate-pulse">⟳</span>
                      <span class="text-muted-foreground font-medium">{wfCount()} workflows running</span>
                      <span class="text-muted-foreground ml-auto">
                        <Show when={workflowBarsExpanded()} fallback={<ChevronRight size={14} />}>
                          <ChevronDown size={14} />
                        </Show>
                      </span>
                    </div>
                  </Show>

                  {/* Individual bars: always shown for single workflow, toggled for multiple */}
                  <Show when={wfCount() === 1 || workflowBarsExpanded()}>
                    <For each={filteredWorkflows()}>
                      {(wf) => renderWorkflowBar(wf)}
                    </For>
                  </Show>
                </>
              );
            })()}

            {/* ── Workflow gate response dialog (opened from status bar) ── */}
            <Show when={wfGateDialog()}>
              {(gate) => {
                const submitGateResponse = async (choice?: string) => {
                  const g = gate();
                  const response: { selected?: string; text?: string } = {};
                  if (choice != null) response.selected = choice;
                  if (wfGateText().trim()) response.text = wfGateText().trim();
                  try {
                    await props.onRespondWorkflowGate(g.instanceId, g.stepId, response);
                    props.onQuestionAnswered(g.requestId, choice ?? wfGateText().trim());
                    setWfGateDialog(null);
                    setWfGateText('');
                  } catch (err) {
                    console.error('Failed to respond to workflow gate:', err);
                  }
                };
                return (
                  <div class="mx-2 mb-2 rounded-lg border border-yellow-500/40 bg-yellow-900/10 p-3 max-h-[50vh] overflow-y-auto">
                    <div class="flex items-start gap-2 mb-2">
                      <MessageSquare size={16} class="text-yellow-300 mt-0.5 flex-shrink-0" />
                      <div class="flex-1 min-w-0 overflow-hidden">
                        <div class="font-medium text-yellow-200 text-xs mb-1">Workflow Feedback</div>
                        <div class="prose max-w-none text-foreground markdown-body text-[inherit] overflow-x-auto" innerHTML={renderMarkdown(gate().prompt)} />
                      </div>
                      <button class="text-muted-foreground hover:text-foreground" onClick={() => setWfGateDialog(null)}>✕</button>
                    </div>
                    <Show when={gate().choices.length > 0}>
                      <div class="flex flex-wrap gap-1.5 mb-2">
                        <For each={gate().choices}>
                          {(choice) => (
                            <Button variant="outline" size="sm" class="text-xs" onClick={() => void submitGateResponse(choice)}>{choice}</Button>
                          )}
                        </For>
                      </div>
                    </Show>
                    <Show when={gate().allowFreeform || gate().choices.length === 0}>
                      <div class="flex gap-2">
                        <textarea
                          class="flex-1 min-h-[40px] max-h-[120px] rounded border border-input bg-background px-2 py-1.5 text-sm resize-y"
                          placeholder="Type your response…"
                          value={wfGateText()}
                          onInput={(e) => setWfGateText(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && wfGateText().trim()) { e.preventDefault(); void submitGateResponse(); } }}
                        />
                        <Button size="sm" disabled={!wfGateText().trim()} onClick={() => void submitGateResponse()}>Send</Button>
                      </div>
                    </Show>
                  </div>
                );
              }}
            </Show>

            <Show when={!props.readOnly}>
            <div
              class={`relative flex items-end gap-2 rounded-lg border border-input bg-background p-2 ${isDragging() ? 'drag-over' : ''}`}
              onDragOver={handleDragOver}
              onDragLeave={handleDragLeave}
              onDrop={handleDrop}
            >
              <Show when={props.pendingAttachments().length > 0}>
                <div class="attachment-preview-strip">
                  <For each={props.pendingAttachments()}>
                    {(att) => (
                      <div class="attachment-thumb">
                        <img
                          src={`data:${att.media_type};base64,${att.data}`}
                          alt={att.filename ?? 'image'}
                        />
                        <Button variant="ghost" size="icon" class="size-5" onClick={() => removeAttachment(att.id)} title="Remove" aria-label="Remove attachment">×</Button>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              {/* Slash command autocomplete */}
              <Show when={slashMatches()}>
                {(matches) => (
                  <div class="absolute bottom-full left-0 z-50 mb-1 max-h-60 w-full overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
                    <For each={matches()}>
                      {(tpl) => (
                        <div
                          class="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                          onMouseDown={(e) => {
                            e.preventDefault();
                            props.setDraft('');
                            if (!tpl.input_schema?.properties || Object.keys(tpl.input_schema.properties).length === 0) {
                              props.setDraft(tpl.template);
                            } else {
                              setActivePromptTemplate({ template: tpl, persona_id: props.selectedAgentId() });
                            }
                          }}
                        >
                          <span class="font-medium">{tpl.name || tpl.id}</span>
                          <Show when={tpl.description}>
                            <span class="text-xs text-muted-foreground">{tpl.description}</span>
                          </Show>
                        </div>
                      )}
                    </For>
                  </div>
                )}
              </Show>
              {/* /workflow or /wf slash command autocomplete */}
              <Show when={slashWorkflowMatches()}>
                {(matches) => (
                  <div class="absolute bottom-full left-0 z-50 mb-1 max-h-60 w-full overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
                    <div class="px-2 py-1 text-xs text-muted-foreground border-b border-border mb-1">Chat workflows</div>
                    <For each={matches()}>
                      {(def) => (
                        <div
                          class="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                          onMouseDown={(e) => {
                            e.preventDefault();
                            props.setDraft('');
                            // Pre-fill the launcher dialog with this workflow
                            setWfLaunchValue({ definition: def.name, inputs: {} });
                            setShowWfLauncher(true);
                          }}
                        >
                          <GitBranch size={14} class="shrink-0 text-muted-foreground" />
                          <span class="font-medium">{def.name}</span>
                          <Show when={def.description}>
                            <span class="text-xs text-muted-foreground truncate">{def.description}</span>
                          </Show>
                        </div>
                      )}
                    </For>
                  </div>
                )}
              </Show>
              {/* @-mention file autocomplete */}
              <Show when={atMentionMatches()}>
                {(info) => (
                  <div class="absolute bottom-full left-0 z-50 mb-1 max-h-60 w-full overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
                    <div class="px-2 py-1 text-xs text-muted-foreground border-b border-border mb-1">Workspace files</div>
                    <For each={info().matches}>
                      {(file, idx) => (
                        <div
                          class={`flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground ${idx() === atMentionIndex() ? 'bg-accent text-accent-foreground' : ''}`}
                          onMouseDown={(e) => {
                            e.preventDefault();
                            insertAtMention(file.path);
                          }}
                        >
                          <FileText size={14} class="shrink-0 text-muted-foreground" />
                          <span class="truncate font-mono text-xs">{file.path}</span>
                        </div>
                      )}
                    </For>
                  </div>
                )}
              </Show>
              <textarea
                ref={textareaRef}
                data-testid="composer-textarea"
                aria-label="Message input"
                value={props.draft()}
                placeholder={
                  props.daemonOnline()
                    ? 'Ask HiveMind something, or type @ to reference workspace files…'
                    : 'Start the daemon to enable chat.'
                }
                disabled={!props.daemonOnline() || props.busyAction() === 'start'}
                onInput={(event) => {
                  props.setDraft(event.currentTarget.value);
                  setCursorPos(event.currentTarget.selectionStart ?? 0);
                }}
                onClick={(event) => setCursorPos(event.currentTarget.selectionStart ?? 0)}
                onSelect={(event) => setCursorPos((event.currentTarget as HTMLTextAreaElement).selectionStart ?? 0)}
                onKeyDown={(event) => {
                  const mentions = atMentionMatches();
                  if (mentions) {
                    if (event.key === 'ArrowDown') {
                      event.preventDefault();
                      setAtMentionIndex((prev) => Math.min(prev + 1, mentions.matches.length - 1));
                      return;
                    }
                    if (event.key === 'ArrowUp') {
                      event.preventDefault();
                      setAtMentionIndex((prev) => Math.max(prev - 1, 0));
                      return;
                    }
                    if (event.key === 'Enter' || event.key === 'Tab') {
                      event.preventDefault();
                      const selected = mentions.matches[atMentionIndex()];
                      if (selected) insertAtMention(selected.path);
                      return;
                    }
                    if (event.key === 'Escape') {
                      event.preventDefault();
                      // Clear the @ query to dismiss the popup
                      setCursorPos(0);
                      return;
                    }
                  }
                  if (event.key === 'Enter' && !event.shiftKey) {
                    event.preventDefault();
                    void props.sendMessage();
                  }
                }}
                onPaste={handlePaste}
              />
            </div>
            <div class="flex items-center justify-between gap-1 px-1 py-1">
              <div class="flex items-center gap-0.5">
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="chat-settings-btn"
                  aria-label="Chat settings"
                  onClick={() => setShowConfigDialog(true)}
                  title={`Configure session — ${currentPersona()?.name || 'General Agent'}`}
                >
                  <Settings size={14} />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="upload-btn"
                  aria-label="Upload file"
                  disabled={!props.daemonOnline() || props.busyAction() !== null || !props.selectedSessionId()}
                  onClick={() => void props.uploadFiles()}
                  title="Add files to workspace"
                >
                  <Paperclip size={14} />
                </Button>

                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  disabled={!props.daemonOnline() || !props.selectedSessionId()}
                  onClick={() => { setShowWfLauncher(true); }}
                  title="Launch a chat workflow"
                  aria-label="Launch a chat workflow"
                >
                  <GitBranch size={14} />
                </Button>
                <Show when={currentPersonaPrompts().length > 0}>
                  <Popover open={showPromptPicker()} onOpenChange={setShowPromptPicker}>
                    <PopoverTrigger
                      as={(triggerProps: any) => (
                        <Button
                          variant="ghost"
                          size="icon"
                          class="size-8"
                          data-testid="prompt-picker-btn"
                          disabled={!props.daemonOnline()}
                          title="Prompt templates"
                          aria-label="Prompt templates"
                          {...triggerProps}
                        >
                          <BookOpen size={14} />
                        </Button>
                      )}
                    />
                    <PopoverContent class="max-h-60 min-w-[240px] max-w-[360px] overflow-y-auto p-1">
                      <For each={currentPersonaPrompts()}>
                        {(tpl) => (
                          <div
                            class="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm hover:bg-accent hover:text-accent-foreground"
                            role="button"
                            tabIndex={0}
                            onClick={() => {
                              setShowPromptPicker(false);
                              if (!tpl.input_schema?.properties || Object.keys(tpl.input_schema.properties).length === 0) {
                                props.setDraft(tpl.template);
                              } else {
                                setActivePromptTemplate({ template: tpl, persona_id: props.selectedAgentId() });
                              }
                            }}
                            onKeyDown={(e: KeyboardEvent) => {
                              if (e.key === 'Enter' || e.key === ' ') {
                                e.preventDefault();
                                setShowPromptPicker(false);
                                if (!tpl.input_schema?.properties || Object.keys(tpl.input_schema.properties).length === 0) {
                                  props.setDraft(tpl.template);
                                } else {
                                  setActivePromptTemplate({ template: tpl, persona_id: props.selectedAgentId() });
                                }
                              }
                            }}
                          >
                            <span class="font-medium">{tpl.name || tpl.id}</span>
                            <Show when={tpl.description}>
                              <span class="text-xs text-muted-foreground">{tpl.description}</span>
                            </Show>
                          </div>
                        )}
                      </For>
                    </PopoverContent>
                  </Popover>
                </Show>
                <Show when={props.activeSessionState() === 'running'}>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="interrupt-btn"
                  aria-label="Interrupt"
                  disabled={props.busyAction() !== null}
                  onClick={() => void props.interrupt('soft')}
                  title="Pause"
                >
                  <Pause size={14} />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="interrupt-hard-btn"
                  aria-label="Stop"
                  disabled={props.busyAction() !== null}
                  onClick={() => void props.interrupt('hard')}
                  title="Stop"
                >
                  <Square size={14} />
                </Button>
                </Show>
                <Show when={props.activeSessionState() === 'paused' || props.activeSessionState() === 'interrupted'}>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="resume-btn"
                  aria-label="Resume"
                  disabled={props.busyAction() !== null}
                  onClick={() => void props.resume()}
                  title="Resume"
                >
                  <Play size={14} />
                </Button>
                </Show>
                <Show when={props.session()?.recalled_memories?.length}>
                  {(count) => (
                    <Button
                      variant="ghost"
                      size="icon"
                      class="size-8"
                      data-testid="memories-btn"
                      aria-label="Recalled memories"
                      onClick={() => props.setShowMemoriesDialog(true)}
                      title={`${count()} recalled ${count() === 1 ? 'memory' : 'memories'}`}
                    >
                      <Brain size={14} />
                    </Button>
                  )}
                </Show>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-8"
                  data-testid="session-perms-btn"
                  aria-label="Session permissions"
                  onClick={() => { void props.loadSessionPerms(); props.setShowSessionPermsDialog(true); }}
                  title="Session permissions"
                >
                  <Lock size={14} />
                </Button>
                <Button
                  variant={props.showDiagnostics() ? 'secondary' : 'ghost'}
                  size="icon"
                  class="size-8"
                  data-testid="diagnostics-toggle"
                  aria-label="Toggle diagnostics"
                  onClick={() => props.setShowDiagnostics(!props.showDiagnostics())}
                  title="Toggle session activity strip"
                >
                  <BarChart3 size={14} />
                </Button>
              </div>
              <Show
                when={props.activeSessionState() === 'running'}
                fallback={
                  <Button
                    data-testid="send-btn"
                    aria-label="Send message"
                    disabled={!props.daemonOnline() || (!props.draft().trim() && props.pendingAttachments().length === 0) || props.busyAction() === 'send'}
                    onClick={() => void props.sendMessage()}
                  >
                    {props.busyAction() === 'send' ? 'Queueing…' : 'Send'}
                  </Button>
                }
              >
                <Button
                  data-testid="send-btn"
                  aria-label="Send message (interrupts current turn)"
                  disabled={!props.daemonOnline() || (!props.draft().trim() && props.pendingAttachments().length === 0) || props.busyAction() === 'send'}
                  onClick={() => void props.sendMessage()}
                >
                  {props.busyAction() === 'send' ? 'Sending…' : 'Send'}
                </Button>
                <Button
                  data-testid="queue-btn"
                  aria-label="Queue message after current turn"
                  disabled={!props.daemonOnline() || (!props.draft().trim() && props.pendingAttachments().length === 0) || props.busyAction() === 'send'}
                  onClick={() => void props.sendMessage(undefined, { skipPreempt: true })}
                  style={{ "margin-left": "4px" }}
                >
                  Queue
                </Button>
              </Show>
            </div>
            </Show>
            <Show when={props.readOnly}>
              <div class="px-3 py-2 text-center text-xs text-muted-foreground border-t border-border">
                This bot has finished — chat is read-only.
              </div>
            </Show>
          </section>
        </div>
      }
    >
      {/* Spatial canvas as chat tab content */}
      <div class="relative flex-1 min-h-0">
        <div class="absolute inset-0 flex flex-col">
          <SpatialCanvas
            session_id={props.selectedSessionId()!}
            onSendMessage={props.onSpatialSendMessage}
            messages={activeSession().messages}
            streamingContent={props.streamingContent()}
            activeSessionState={props.activeSessionState}
            daemonOnline={props.daemonOnline}
            isStreaming={props.isStreaming}
            activities={props.activities}
            busyAction={props.busyAction}
            toolCallHistory={props.toolCallHistory}
            allQuestions={props.allQuestions}
            onQuestionAnswered={props.onQuestionAnswered}
            pendingReview={props.pendingReview}
            sendDecision={props.sendMessage}
            interrupt={props.interrupt}
            resume={props.resume}
            onShowConfig={() => setShowConfigDialog(true)}
            onShowPermissions={() => { props.loadSessionPerms(); props.setShowSessionPermsDialog(true); }}
            onShowSettings={(tab) => { if (tab) props.setSettingsTab(tab); props.setShowSettings(true); }}
            onShowMemories={() => props.setShowMemoriesDialog(true)}
            onUploadFiles={props.uploadFiles}
            onShowWorkflowLauncher={() => setShowWfLauncher(true)}
            onShowToolCall={(tc) => setPopupToolCall(tc)}
            personas={props.personas}
            selectedAgentId={props.selectedAgentId}
            chatFontPx={props.chatFontPx}
            draft={props.draft}
            setDraft={props.setDraft}
            pendingAttachments={props.pendingAttachments}
            setPendingAttachments={props.setPendingAttachments}
            activeChatWorkflows={props.activeChatWorkflows}
            terminalChatWorkflows={props.terminalChatWorkflows}
            onPauseChatWorkflow={props.onPauseChatWorkflow}
            onResumeChatWorkflow={props.onResumeChatWorkflow}
            onKillChatWorkflow={props.onKillChatWorkflow}
          />
        </div>
      </div>
    </Show>

    <SessionConfigDialog
      open={showConfigDialog()}
      session_id={props.selectedSessionId}
      personas={props.personas}
      tools={props.tools}
      installedSkills={props.installedSkills}
      selectedAgentId={props.selectedAgentId}
      setSelectedAgentId={props.setSelectedAgentId}
      selectedDataClass={props.selectedDataClass}
      setSelectedDataClass={props.setSelectedDataClass}
      excludedTools={props.excludedTools}
      setExcludedTools={props.setExcludedTools}
      excludedSkills={props.excludedSkills}
      setExcludedSkills={props.setExcludedSkills}
      onClose={() => setShowConfigDialog(false)}
    />

    {/* Tool call detail popup */}
    <Dialog
      open={!!popupToolCall()}
      onOpenChange={(open) => { if (!open) setPopupToolCall(null); }}
    >
      <DialogContent class="max-w-4xl max-h-[85vh] flex flex-col" style="overflow-x:hidden;overflow-y:auto;">
      <Show when={popupToolCall()}>
        {(tc) => {
            // Parse MCP tool ID: "mcp.{serverId}.{toolName}"
            const mcpMatch = () => tc().tool_id.match(/^mcp\.(.+?)\.(.+)$/);
            const serverId = () => mcpMatch()?.[1];
            const toolName = () => mcpMatch()?.[2];

            // Check if this tool has an MCP App UI
            const mcpToolEntry = () => {
              if (!props.mcpAppTools || !serverId() || !toolName()) return undefined;
              return props.mcpAppTools().get(`${serverId()}::${toolName()}`);
            };
            const toolUiMeta = () => mcpToolEntry()?.ui_meta;
            const uiResourceUri = () => toolUiMeta()?.resource_uri;

            // Fetch the UI resource HTML when the popup opens
            const [appHtml, setAppHtml] = createSignal<string | null>(null);
            createEffect(() => {
              const uri = uiResourceUri();
              const sid = serverId();
              const daemonUrl = props.daemonUrl?.();
              if (!uri || !sid || !daemonUrl) return;
              // Check app-level cache first
              const cacheKey = `${sid}::${uri}`;
              const cached = props.mcpAppHtmlCache?.()?.get(cacheKey);
              if (cached) { setAppHtml(cached); return; }
              void (async () => {
                try {
                  const resp = await authFetch(`${daemonUrl}/api/v1/mcp/servers/${encodeURIComponent(sid)}/fetch-ui-resource`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ uri }),
                  });
                  if (resp.ok) {
                    const resource = await resp.json() as { html: string };
                    setAppHtml(resource.html);
                    if (props.setMcpAppHtmlCache) {
                      props.setMcpAppHtmlCache(prev => { const m = new Map(prev); m.set(cacheKey, resource.html); return m; });
                    }
                  }
                } catch (e) {
                  console.warn('[MCP App] Failed to fetch UI resource:', e);
                }
              })();
            });

            return (<>
            <header class="mb-3 flex items-center gap-2">
              <span class={`text-lg ${tc().isError ? 'text-destructive' : 'text-green-400'}`}>{tc().isError ? '✗' : '✓'}</span>
              <h3 class="flex-1 text-sm font-semibold text-foreground">{tc().label}</h3>
              <Show when={tc().completedAt && tc().startedAt}>
                <Badge variant="secondary">{Math.max(1, Math.round(((tc().completedAt ?? 0) - tc().startedAt) / 1000))}s</Badge>
              </Show>
              <Button variant="ghost" size="icon" class="size-8" onClick={() => setPopupToolCall(null)} aria-label="Close">✕</Button>
            </header>

            {/* MCP App view (when available) — popout mode with larger container */}
            <Show when={appHtml() && serverId()}>
              {(html) => (
                <McpAppView
                  html={appHtml()!}
                  serverId={serverId()!}
                  toolName={toolName()!}
                  toolInput={tc().input}
                  toolOutput={tc().output}
                  toolIsError={tc().isError}
                  toolResultRaw={tc().mcpRaw}
                  toolInputSchema={mcpToolEntry()?.input_schema as Record<string, unknown> | undefined}
                  toolDescription={mcpToolEntry()?.description}
                  toolVisibility={toolUiMeta()?.visibility ?? undefined}
                  sessionId={props.selectedSessionId() ?? ''}
                  daemonUrl={props.daemonUrl?.() ?? ''}
                  uiMeta={toolUiMeta()}
                  theme="dark"
                  displayMode="fullscreen"
                />
              )}
            </Show>

            {/* Standard tool I/O (shown when no app, or as fallback) */}
            <Show when={!appHtml()}>
              <div class="space-y-3" style="min-width:0;overflow:hidden;">
                <Show when={tc().input}>
                  <div style="min-width:0;">
                    <span class="mb-1 block text-xs font-medium text-muted-foreground">Input</span>
                    <pre class="rounded border border-border bg-muted/30 p-2 text-xs" style="overflow-x:auto;overflow-y:auto;max-height:35vh;white-space:pre;word-break:normal;" innerHTML={highlightYaml(tc().input!)} />
                  </div>
                </Show>
                <Show when={tc().output}>
                  <div style="min-width:0;">
                    <span class="mb-1 block text-xs font-medium text-muted-foreground">Output</span>
                    <pre class="rounded border border-border bg-muted/30 p-2 text-xs" style="overflow-x:auto;overflow-y:auto;max-height:35vh;white-space:pre;word-break:normal;" innerHTML={highlightYaml(
                      tc().output!.length > 4000 ? tc().output!.slice(0, 4000) + '\n… (truncated)' : tc().output!
                    )} />
                  </div>
                </Show>
              </div>
            </Show>
        </>);}}
      </Show>
      </DialogContent>
    </Dialog>

    {/* ── Chat Workflow Launcher Dialog ── */}
    <Dialog
      open={showWfLauncher()}
      onOpenChange={(open) => { if (!open) setShowWfLauncher(false); }}
    >
      <DialogContent class="min-w-[400px] max-w-[640px] w-[90vw] max-h-[80vh] flex flex-col overflow-hidden p-4">
        <h3 class="m-0 mb-3 text-base text-foreground">
          <GitBranch size={14} /> Launch Chat Workflow
        </h3>
        <DialogBody>
          <WorkflowLauncher
            definitions={props.chatWorkflowDefinitions().map(d => ({ name: d.name, version: d.version, description: d.description }))}
            fetchParsedDefinition={props.fetchParsedWorkflow}
            value={wfLaunchValue()}
            onChange={setWfLaunchValue}
          />
        </DialogBody>
        <div class="mt-3 flex shrink-0 justify-end gap-2">
          <Button variant="outline" size="sm" onClick={() => setShowWfLauncher(false)}>Cancel</Button>
          <Button
            disabled={!wfLaunchValue()?.definition || wfLaunching()}
            onClick={() => void handleLaunchChatWorkflow()}
            class="bg-blue-400 text-background font-semibold disabled:opacity-50"
          >
            {wfLaunching() ? 'Launching…' : 'Launch'}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
    {/* ── Prompt parameter dialog ── */}
    <Show when={activePromptTemplate()}>
      {(tpl) => (
        <PromptParameterDialog
          template={tpl().template}
          persona_id={tpl().persona_id}
          onSubmit={(rendered, _params) => {
            props.setDraft(rendered);
            setActivePromptTemplate(null);
          }}
          onCancel={() => setActivePromptTemplate(null)}
        />
      )}
    </Show>
    </>
  );
};

export default ChatView;
