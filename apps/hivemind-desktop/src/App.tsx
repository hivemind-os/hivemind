import { For, Show, Switch, Match, ErrorBoundary, batch, createEffect, createMemo, createSignal, lazy, onCleanup, onMount } from 'solid-js';
import { Portal } from 'solid-js/web';
import { ShieldCheck, TriangleAlert, RefreshCw, Rocket, FolderOpen, Link, ClipboardList, MessageSquare, Compass, Layers, GitBranch, Activity, Terminal, Settings, Plug } from 'lucide-solid';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { authFetch } from '~/lib/authFetch';
import InspectorModal from './components/InspectorModal';
import FlightDeck from './components/FlightDeck';
import AgentStage from './components/AgentStage';
import SessionPermsDialog from './components/SessionPermsDialog';
import SettingsModal from './components/SettingsModal';
import SetupWizard from './components/setup/SetupWizard';
import Sidebar from './components/Sidebar';
import { SidebarProvider, UiSidebar, SidebarInset } from '~/ui';
import ToolApprovalDialog from './components/ToolApprovalDialog';
import AgentApprovalToast, { type PendingApproval } from './components/AgentApprovalToast';
import { UpdateDialog } from './components/UpdateDialog';
import type { Update } from '@tauri-apps/plugin-updater';
import type { PendingQuestion } from './components/InlineQuestion';
import ErrorBanner from './components/ErrorBanner';
const SessionWorkflows = lazy(() => import('./components/SessionWorkflows'));
const SessionEvents = lazy(() => import('./components/SessionEvents'));
const SessionProcesses = lazy(() => import('./components/SessionProcesses'));
import ChatView from './components/ChatView';
import WorkspaceView from './components/WorkspaceView';
import { warmUpHighlighter } from './lib/shikiHighlighter';
import BotsPage from './components/BotsPage';
import BotConfigPanel from './components/BotConfigPanel';
import SessionMcpPanel from './components/SessionMcpPanel';
import SchedulerPage from './components/SchedulerPage';
import WorkflowsPage from './components/WorkflowsPage';
import WorkflowDetailPanel from './components/WorkflowDetailPanel';
import AgentKitsPage from './components/AgentKitsPage';
import { createWorkflowStore } from './stores/workflowStore';
import { createBotStore } from './stores/botStore';
import { createInteractionStore } from './stores/interactionStore';
import { createAgentKitStore } from './stores/agentKitStore';
import { createSessionOrdering } from './stores/sessionOrdering';
import { createConfigStore, isNoTokenError, isLicenseError, extractRepoFromError, openExternal, scrollToHfToken } from './stores/configStore';
import type {
  Persona,
  AppContext,
  CapabilityOption,
  ChatMemoryItem,
  ChatRunState,
  ChatSessionSnapshot,
  ChatSessionSummary,
  DaemonStatus,
  DataClass,
  DownloadProgress,
  HiveMindConfigData,
  HardwareInfo,
  HardwareSummary,
  HubFileInfo,
  HubModelInfo,
  HubRepoFilesResult,
  HubSearchResult,
  InferenceParams,
  InstalledModel,
  KgNode,
  KgNodeWithEdges,
  KgStats,
  LocalModelSummary,
  McpNotificationEvent,
  McpPromptInfo,
  McpResourceInfo,
  McpServerLog,
  McpServerSnapshot,
  McpToolInfo,
  MessageAttachment,
  ModelRouterSnapshot,
  PromptInjectionReview,
  RiskScanRecord,
  RuntimeResourceUsage,
  ScanDecision,
  SendMessageResponse,
  SessionModality,
  ToolDefinition,
  InstalledSkill,
  InteractionKind,
  UserInteractionResponse,
} from './types';
import { parseModelMeta, groupInstallableFiles } from './types';
import type { InstallableItem } from './types';
import {
  dataClassBadge,
  fileToBase64,
  formatBytes,
  formatPayload,
  formatTime,
  isTauriInternalError,
  mcpStatusClass,
  renderMarkdown,
  riskClass,
  statusClass,
  workspaceNameFromPath,
} from './utils';

const App = () => {
  const [daemonStatus, setDaemonStatus] = createSignal<DaemonStatus | null>(null);
  const [configYaml, setConfigYaml] = createSignal<string>('');
  const [context, setContext] = createSignal<AppContext | null>(null);
  const [sessions, setSessions] = createSignal<ChatSessionSummary[]>([]);
  const [userStatus, setUserStatus] = createSignal<string>('active');

  const {
    sessionOrder, setSessionOrder,
    updateSessions, orderedSessions,
    displayedSessions, reorderSessions,
  } = createSessionOrdering({ sessions, setSessions });
  const [session, setSession] = createSignal<ChatSessionSnapshot | null>(null);
  const [selectedSessionId, setSelectedSessionId] = createSignal<string | null>(null);
  const [personas, setPersonas] = createSignal<Persona[]>([]);
  const [selectedAgentId, setSelectedAgentId] = createSignal<string>('system/general');
  const [sessionMemory, setSessionMemory] = createSignal<ChatMemoryItem[]>([]);
  const [memoryQuery, setMemoryQuery] = createSignal<string>('');
  const [memoryResults, setMemoryResults] = createSignal<ChatMemoryItem[]>([]);
  const [riskScans, setRiskScans] = createSignal<RiskScanRecord[]>([]);
  const [modelRouter, setModelRouter] = createSignal<ModelRouterSnapshot | null>(null);
  const [mcpServers, setMcpServers] = createSignal<McpServerSnapshot[]>([]);
  const [selectedMcpServerId, setSelectedMcpServerId] = createSignal<string | null>(null);
  const [mcpTools, setMcpTools] = createSignal<McpToolInfo[]>([]);
  const [mcpResources, setMcpResources] = createSignal<McpResourceInfo[]>([]);
  const [mcpPrompts, setMcpPrompts] = createSignal<McpPromptInfo[]>([]);
  const [mcpNotifications, setMcpNotifications] = createSignal<McpNotificationEvent[]>([]);
  const [mcpServerLogs, setMcpServerLogs] = createSignal<McpServerLog[]>([]);
  const [mcpLogsServerId, setMcpLogsServerId] = createSignal<string | null>(null);
  const [tools, setTools] = createSignal<ToolDefinition[]>([]);
  const [channels, setChannels] = createSignal<{ id: string; name: string; provider: string; hasComms: boolean }[]>([]);
  const [eventTopics, setEventTopics] = createSignal<{ topic: string; description: string; payload_keys?: string[] }[]>([]);
  const [installedSkills, setInstalledSkills] = createSignal<InstalledSkill[]>([]);
  const [excludedTools, setExcludedTools] = createSignal<string[]>([]);
  const [excludedSkills, setExcludedSkills] = createSignal<string[]>([]);
  const [pendingReview, setPendingReview] = createSignal<PromptInjectionReview | null>(null);
  const [pendingReviewContent, setPendingReviewContent] = createSignal<string | null>(null);
  const [pendingCanvasPosition, setPendingCanvasPosition] = createSignal<[number, number] | null>(null);
  const [draft, setDraft] = createSignal<string>('');
  const [pendingAttachments, setPendingAttachments] = createSignal<MessageAttachment[]>([]);
  const [busyAction, setBusyAction] = createSignal<string | null>(null);
  const [initializing, setInitializing] = createSignal(true);
  const [errorMessage, setErrorMessage] = createSignal<string | null>(null);
  const [lastUpdated, setLastUpdated] = createSignal<string>('never');
  const [showNewSessionDialog, setShowNewSessionDialog] = createSignal(false);
  const [inspectorOpen, setInspectorOpen] = createSignal(false);
  const [flightDeckOpen, setFlightDeckOpen] = createSignal(false);
  const [flightDeckNeedsAttention, setFlightDeckNeedsAttention] = createSignal(false);
  const [externalApproval, setExternalApproval] = createSignal<PendingApproval | null>(null);
  const [externalQuestion, setExternalQuestion] = createSignal<PendingQuestion | null>(null);
  const [showQueuedPopup, setShowQueuedPopup] = createSignal(false);

  const [localModels, setLocalModels] = createSignal<InstalledModel[]>([]);
  const [hubSearchResults, setHubSearchResults] = createSignal<HubModelInfo[]>([]);
  const [hubSearchQuery, setHubSearchQuery] = createSignal('');
  const [hubSearchLoading, setHubSearchLoading] = createSignal(false);
  const [hubSearchError, setHubSearchError] = createSignal<string | null>(null);
  const [hardwareInfo, setHardwareInfo] = createSignal<HardwareInfo | null>(null);
  const [resourceUsage, setResourceUsage] = createSignal<RuntimeResourceUsage | null>(null);
  const [storageBytes, setStorageBytes] = createSignal<number>(0);
  const [localModelView, setLocalModelView] = createSignal<'library' | 'search' | 'hardware'>('library');
  const [installTargetRepo, setInstallTargetRepo] = createSignal<HubModelInfo | null>(null);
  const [installRepoFiles, setInstallRepoFiles] = createSignal<HubFileInfo[]>([]);
  const [installableItems, setInstallableItems] = createSignal<InstallableItem[]>([]);
  const [installFilesLoading, setInstallFilesLoading] = createSignal(false);
  const [installInProgress, setInstallInProgress] = createSignal(false);
  const [activeDownloads, setActiveDownloads] = createSignal<DownloadProgress[]>([]);
  const [expandedModel, setExpandedModel] = createSignal<string | null>(null);

  const [kgStats, setKgStats] = createSignal<KgStats | null>(null);
  const [kgNodes, setKgNodes] = createSignal<KgNode[]>([]);
  const [kgSelectedNode, setKgSelectedNode] = createSignal<KgNodeWithEdges | null>(null);
  const [kgSearchQuery, setKgSearchQuery] = createSignal('');
  const [kgSearchResults, setKgSearchResults] = createSignal<KgNode[]>([]);
  const [kgNodeTypeFilter, setKgNodeTypeFilter] = createSignal('');
  const [kgView, setKgView] = createSignal<'browse' | 'search' | 'create'>('browse');
  const [kgNewNodeType, setKgNewNodeType] = createSignal('');
  const [kgNewNodeName, setKgNewNodeName] = createSignal('');
  const [kgNewNodeContent, setKgNewNodeContent] = createSignal('');
  const [kgNewNodeDataClass, setKgNewNodeDataClass] = createSignal<DataClass>('INTERNAL');
  const [kgNewEdgeTargetId, setKgNewEdgeTargetId] = createSignal('');
  const [kgNewEdgeType, setKgNewEdgeType] = createSignal('');

  const [showDiagnostics, setShowDiagnostics] = createSignal(false);
  const [settingsTab, setSettingsTab] = createSignal<'general-appearance' | 'general-daemon' | 'general-recording' | 'providers' | 'security' | 'mcp' | 'local-models' | 'scheduler' | 'downloads' | 'tools' | 'personas' | 'compaction' | 'channels' | 'comm-audit' | 'afk' | 'python' | 'node' | 'web-search'>('general-appearance');
  const [showSetupWizard, setShowSetupWizard] = createSignal(false);
  const [showMemoriesDialog, setShowMemoriesDialog] = createSignal(false);
  const [showSessionPermsDialog, setShowSessionPermsDialog] = createSignal(false);
  const [showUpdateDialog, setShowUpdateDialog] = createSignal(false);
  const [pendingUpdate, setPendingUpdate] = createSignal<Update | null>(null);
  const [updateCheckState, setUpdateCheckState] = createSignal<'idle' | 'checking' | 'up-to-date' | 'update-available' | 'error' | 'unavailable'>('idle');
  const [updateCheckError, setUpdateCheckError] = createSignal<string | null>(null);
  let updateCheckRequestId = 0;
  const [sessionPerms, setSessionPerms] = createSignal<{ rules: { tool_pattern: string; scope: string; decision: string }[] }>({ rules: [] });
  const [sidebarOpen, setSidebarOpen] = createSignal(true);
  const [activeTab, setActiveTab] = createSignal<'chat' | 'workspace' | 'stage' | 'workflows' | 'events' | 'processes' | 'config' | 'mcp'>('chat');
  const [activeScreen, setActiveScreen] = createSignal<'session' | 'bots' | 'scheduler' | 'workflows' | 'settings' | 'agent-kits'>('session');
  const [sessionEntityType, setSessionEntityType] = createSignal<'session' | 'bot'>('session');
  const workflowStore = createWorkflowStore();
  const botStore = createBotStore();
  const interactionStore = createInteractionStore();
  const agentKitStore = createAgentKitStore();

  // ── Chat Workflow State (multi-workflow) ──
  interface ChatWorkflowTracker {
    instanceId: number;
    instance: any | null;
    events: any[];
  }
  const [chatWorkflows, setChatWorkflows] = createSignal<ChatWorkflowTracker[]>([]);
  const activeChatWorkflows = createMemo(() =>
    chatWorkflows().filter(w => !w.instance || !['completed', 'failed', 'killed'].includes(w.instance.status))
  );
  const terminalChatWorkflows = createMemo(() =>
    chatWorkflows().filter(w => w.instance && ['completed', 'failed', 'killed'].includes(w.instance.status))
  );

  // Load chat workflow definitions when chat screen is active or session changes
  createEffect(() => {
    const _sid = selectedSessionId(); // re-run when session changes (daemon is ready)
    if (activeScreen() === 'session' || activeScreen() === 'bots') {
      void workflowStore.loadChatDefinitions();
    }
  });

  // Load workflow definitions when the scheduler screen becomes active
  createEffect(() => {
    if (activeScreen() === 'scheduler') {
      void workflowStore.loadDefinitions();
    }
  });

  // Subscribe/unsubscribe bot store when bots screen is active
  createEffect(() => {
    if (activeScreen() === 'bots') {
      void botStore.refresh();
      void botStore.subscribeBotEvents();
    } else {
      botStore.unsubscribeBotEvents();
    }
  });

  // Always keep workflow SSE subscribed for real-time badge updates.
  // Refresh instance data when the workflows screen becomes active.
  createEffect(() => {
    void workflowStore.subscribeEvents();
  });
  createEffect(() => {
    if (activeScreen() === 'workflows') {
      void workflowStore.refresh();
    } else {
      workflowStore.setSidebarSelectedInstanceId(null);
    }
  });

  // Reset sidebar selections when switching screens
  createEffect(() => {
    const screen = activeScreen();
    if (screen !== 'bots') {
      botStore.selectBot(null);
    }
    if (screen !== 'workflows') {
      workflowStore.setSidebarSelectedInstanceId(null);
    }
  });

  // Ensure the workflow event SSE stream is running whenever the daemon is
  // reachable.  This is a global singleton — safe to call repeatedly.
  createEffect(() => {
    const _epoch = daemonEpoch();
    (async () => {
      try { await invoke('workflow_subscribe_events'); } catch { /* already subscribed or daemon offline */ }
    })();
  });

  // Track chat workflow events — subscribe to workflow:event Tauri events
  // and route events to the matching tracked workflow.
  // Scoped to selectedSessionId + daemonEpoch (stable) to avoid listener churn.
  createEffect(() => {
    const sid = selectedSessionId();
    const _epoch = daemonEpoch(); // re-subscribe on daemon reconnect
    if (!sid) return;
    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    (async () => {
      unlisten = await listen<any>('workflow:event', (e) => {
        if (disposed) return;
        const payload = e.payload;
        const evtInstanceId = payload?.payload?.instance_id;
        if (evtInstanceId == null) return;

        // Only process events for tracked workflows (read inside callback — not tracked)
        setChatWorkflows(prev => {
          const idx = prev.findIndex(w => w.instanceId === evtInstanceId);
          if (idx < 0) return prev;
          const updated = [...prev];
          updated[idx] = {
            ...updated[idx],
            events: [...updated[idx].events, {
              topic: payload.topic,
              payload: payload.payload,
              timestamp_ms: payload.timestamp_ms ?? Date.now(),
            }],
          };
          return updated;
        });

        const topic = payload.topic as string;
        if (topic.startsWith('workflow.instance.') || topic.startsWith('workflow.step.') || topic.startsWith('workflow.interaction.')) {
          void refreshChatWorkflowInstance(evtInstanceId);
        }
      });
      if (disposed) { unlisten(); }
    })();

    onCleanup(() => { disposed = true; unlisten?.(); });
  });

  // Forward agent events from workflow child agents to the chat thread.
  createEffect(() => {
    const sid = selectedSessionId();
    const _epoch = daemonEpoch(); // re-subscribe on daemon reconnect
    if (!sid) return;

    let stageUnlisten: UnlistenFn | null = null;

    (async () => {
      // Attach the listener BEFORE starting the SSE subscription so the
      // initial snapshot (emitted immediately on connect) is never missed.
      stageUnlisten = await listen<{ session_id: string; event: any }>(
        'stage:event',
        (ev) => {
          if (ev.payload.session_id !== sid) return;
          const event = ev.payload.event;
          if (!event?.type) return;

          // Sub-agent events (e.g. from workflow-spawned agents) flow through
          // stage:event but NOT through the chat:event SSE stream.  Handle
          // tool approvals and questions here so they aren't silently dropped.
          if (event.type === 'agent_output') {
            const inner = event.event;
            if (!inner) return;
            if (inner.type === 'user_interaction_required') {
              completeActivity('inference');
              pushActivity({ id: 'feedback', kind: 'feedback', label: 'Waiting for your input' });
              if (inner.request_id) {
                setPendingToolApproval({
                  request_id: inner.request_id,
                  tool_id: inner.tool_id,
                  input: inner.input,
                  reason: inner.reason,
                });
              }
            } else if (inner.type === 'question_asked') {
              if (inner.request_id) {
                addPendingQuestion({
                  request_id: inner.request_id,
                  text: inner.text ?? '',
                  choices: inner.choices ?? [],
                  allow_freeform: inner.allow_freeform !== false,
                  multi_select: inner.multi_select === true,
                  agent_id: event.agent_id,
                  agent_name: inner.agent_id,
                  session_id: sid,
                  message: inner.message,
                });
              }
            }
          }
        },
      );

      try {
        await invoke('agent_stage_subscribe', { session_id: sid });
      } catch { /* may already be subscribed */ }
    })();

    onCleanup(() => { stageUnlisten?.(); });
  });

  // Listen for externally-resolved approvals/questions (e.g. answered via AFK email).
  createEffect(() => {
    const _epoch = daemonEpoch();
    let unlisten: UnlistenFn | null = null;
    let disposed = false;

    (async () => {
      unlisten = await listen<any>('approval:event', (e) => {
        if (disposed) return;
        const event = e.payload;
        if (event?.type === 'resolved' && event.request_id) {
          // Dismiss the tool-approval modal if it matches.
          const current = pendingToolApproval();
          if (current && current.request_id === event.request_id) {
            setPendingToolApproval(null);
            completeActivity('feedback');
          }
          // Mark any matching question as answered externally.
          const q = allQuestions().find((q) => q.request_id === event.request_id && !q.answer);
          if (q) {
            markQuestionAnswered(event.request_id, '(answered externally)');
          }
        }
      });
      if (disposed) { unlisten(); }
    })();

    onCleanup(() => { disposed = true; unlisten?.(); });
  });

  // The interaction store is still used for sidebar badges and the triage
  // panel (FlightDeck / AgentStage).  Questions are now rendered from session
  // messages in the chat thread, so we no longer bridge interaction-store
  // snapshots into allQuestions here.

  async function refreshChatWorkflowInstance(instanceId: number) {
    try {
      const full = await invoke<any>('workflow_get_instance', { instance_id: instanceId });
      if (full) {
        const summary = {
          id: full.id,
          definition_name: full.definition?.name ?? '',
          definition_version: full.definition?.version ?? '',
          status: full.status,
          parent_session_id: full.parent_session_id,
          parent_agent_id: full.parent_agent_id,
          created_at_ms: full.created_at_ms,
          updated_at_ms: full.updated_at_ms,
          completed_at_ms: full.completed_at_ms,
          error: full.error,
          resolved_result_message: full.resolved_result_message,
          step_count: Object.keys(full.step_states ?? {}).length,
          steps_completed: Object.values(full.step_states ?? {}).filter((s: any) => s.status === 'completed').length,
          steps_failed: Object.values(full.step_states ?? {}).filter((s: any) => s.status === 'failed').length,
          steps_running: Object.values(full.step_states ?? {}).filter((s: any) => s.status === 'running').length,
          has_pending_interaction: Object.values(full.step_states ?? {}).some((s: any) => s.status === 'waiting_on_input'),
          step_states: full.step_states ?? {},
        };
        setChatWorkflows(prev => prev.map(w =>
          w.instanceId === instanceId ? { ...w, instance: summary } : w
        ));
      }
    } catch { /* ignore */ }
  }

  // Chat workflow action handlers
  async function launchChatWorkflow(definition: string, inputs: any, triggerStepId?: string): Promise<number | null> {
    const sid = selectedSessionId();
    if (!sid) return null;
    const wsPath = session()?.workspace_path;
    const perms = sessionPerms()?.rules?.map((r: any) => ({
      tool_id: r.tool_pattern ?? '*',
      resource: r.scope ?? '*',
      approval: r.decision ?? 'ask',
    })) ?? [];
    const instanceId = await workflowStore.launchChatWorkflow(
      definition, inputs, sid, wsPath ?? '', perms, triggerStepId,
    );
    if (instanceId != null) {
      setChatWorkflows(prev => [...prev, { instanceId, instance: null, events: [] }]);
      // Don't rehydrate immediately — the instance may not be persisted yet.
      // The event listener + poll will pick up the full data shortly.
      void refreshChatWorkflowInstance(instanceId);
    }
    return instanceId;
  }

  function pauseChatWorkflow(instanceId: number) {
    workflowStore.pauseInstance(instanceId).catch(e => setErrorMessage(String(e)));
  }
  function resumeChatWorkflow(instanceId: number) {
    workflowStore.resumeInstance(instanceId).catch(e => setErrorMessage(String(e)));
  }
  function killChatWorkflow(instanceId: number) {
    workflowStore.killInstance(instanceId).catch(e => setErrorMessage(String(e)));
  }
  async function respondWorkflowGate(instanceId: number, stepId: string, response: any) {
    await workflowStore.respondToGate(instanceId, stepId, response);
  }

  // Rehydrate active workflows for a given session from the DB.
  // Called explicitly — not a reactive effect — to avoid async race conditions.
  let rehydrateSeq = 0;
  async function rehydrateChatWorkflows(sid: string) {
    const seq = ++rehydrateSeq;
    try {
      const [activeResult, terminalResult] = await Promise.all([
        invoke<{ items: any[]; total: number }>('workflow_list_instances', {
          session_id: sid,
          status: 'running,paused,pending,waiting_on_input,waiting_on_event',
          limit: 10,
        }),
        invoke<{ items: any[]; total: number }>('workflow_list_instances', {
          session_id: sid,
          status: 'completed,failed,killed',
          limit: 10,
        }),
      ]);
      if (seq !== rehydrateSeq) return; // discard stale response
      const activeItems = activeResult?.items ?? [];
      const terminalItems = terminalResult?.items ?? [];
      const allItems = [...activeItems, ...terminalItems];
      const seen = new Set<string>();
      const unique = allItems.filter(item => { if (seen.has(item.id)) return false; seen.add(item.id); return true; });
      // IMPORTANT: do NOT call other setters inside setChatWorkflows callback —
      // nested signal writes inside an updater break SolidJS notification.
      setChatWorkflows(prev => {
        const dbIds = new Set(unique.map((item: any) => item.id));
        // Only keep un-hydrated placeholders (instance: null) — these are
        // always for the current session (just launched, not yet persisted).
        // Discard fully-hydrated entries from a previous session.
        const localOnly = prev.filter(w => !dbIds.has(w.instanceId) && !w.instance);
        return [
          ...unique.map((item: any) => ({
            instanceId: item.id,
            instance: item,
            events: prev.find(w => w.instanceId === item.id)?.events ?? [],
          })),
          ...localOnly,
        ];
      });
      // Refresh all items to get full detail (including step_states for gate interactions)
      for (const item of unique) {
        void refreshChatWorkflowInstance(item.id);
      }
    } catch { /* ignore */ }
  }

  // Trigger rehydration when session changes or daemon reconnects
  createEffect(() => {
    const sid = selectedSessionId();
    const _epoch = daemonEpoch();
    if (!sid) {
      setChatWorkflows([]);
      return;
    }
    void rehydrateChatWorkflows(sid);
  });

  const [chatFontSize, setChatFontSize] = createSignal<'small' | 'medium' | 'large'>('medium');
  const chatFontPx = () => ({ small: '13px', medium: '14px', large: '16px' }[chatFontSize()]);
  const [expandedMsgIds, setExpandedMsgIds] = createSignal<Set<string>>(new Set());
  const [workspaceFiles, setWorkspaceFiles] = createSignal<any[]>([]);
  const [workspaceLoadedForSession, setWorkspaceLoadedForSession] = createSignal<string | null>(null);
  const [selectedEntryPath, setSelectedEntryPath] = createSignal<string | null>(null);
  const [selectedFilePath, setSelectedFilePath] = createSignal<string | null>(null);
  const [fileContent, setFileContent] = createSignal<any | null>(null);
  const [fileEditorContent, setFileEditorContent] = createSignal('');
  const [fileSaving, setFileSaving] = createSignal(false);
  const [workspaceLoading, setWorkspaceLoading] = createSignal(false);

  // Index status tracking: maps relative file path → 'queued' | 'indexed'
  const [indexStatus, setIndexStatus] = createSignal<Record<string, 'queued' | 'indexed'>>({});

  const [pendingToolApproval, setPendingToolApproval] = createSignal<{
    request_id: string;
    tool_id: string;
    input: string;
    reason: string;
  } | null>(null);

  // Pending questions from agents (core.ask_user tool) — shown inline in chat
  type PendingQuestionEntry = {
    request_id: string;
    text: string;
    choices: string[];
    allow_freeform: boolean;
    multi_select?: boolean;
    session_id?: string;
    agent_id?: string;
    agent_name?: string;
    message?: string;
    is_bot?: boolean;
    workflow_instance_id?: number;
    workflow_step_id?: string;
    timestamp: number;
    answer?: string; // set once answered
  };
  const [allQuestions, setAllQuestions] = createSignal<PendingQuestionEntry[]>([]);
  const pendingQuestions = () => allQuestions().filter((q) => !q.answer);
  const answeredQuestions = () => {
    const m = new Map<string, string>();
    for (const q of allQuestions()) {
      if (q.answer) m.set(q.request_id, q.answer);
    }
    return m;
  };

  const addPendingQuestion = (q: Omit<PendingQuestionEntry, 'timestamp'>) => {
    setAllQuestions((prev) => {
      const existing = prev.find((p) => p.request_id === q.request_id);
      if (existing) {
        // If the previous entry was already answered (e.g. a workflow gate
        // in a loop re-creates the same step_id), replace it so the new
        // unanswered gate appears in the chat thread.
        if (existing.answer) {
          console.debug('[addPendingQuestion] replacing answered question', q.request_id);
          return [...prev.filter((p) => p.request_id !== q.request_id), { ...q, timestamp: Date.now() }];
        }
        // Merge fields from the new entry that improve upon the existing
        // one (e.g. session_id, agent_id arriving from a different SSE
        // path).  Return a NEW array so SolidJS notifies subscribers when
        // a previously-incomplete entry gains its session_id.
        const merged = { ...existing };
        let changed = false;
        if (!merged.session_id && q.session_id) { merged.session_id = q.session_id; changed = true; }
        if (!merged.agent_id && q.agent_id) { merged.agent_id = q.agent_id; changed = true; }
        if (!merged.agent_name && q.agent_name) { merged.agent_name = q.agent_name; changed = true; }
        if (!merged.message && q.message) { merged.message = q.message; changed = true; }
        if (q.workflow_instance_id != null && merged.workflow_instance_id == null) { merged.workflow_instance_id = q.workflow_instance_id; changed = true; }
        if (q.workflow_step_id && !merged.workflow_step_id) { merged.workflow_step_id = q.workflow_step_id; changed = true; }
        if (changed) {
          console.debug('[addPendingQuestion] merging fields into existing question', q.request_id, { session_id: merged.session_id, agent_id: merged.agent_id });
          return prev.map((p) => p.request_id === q.request_id ? merged : p);
        }
        return prev;
      }
      console.debug('[addPendingQuestion] new question', q.request_id, { session_id: q.session_id, agent_id: q.agent_id, text: q.text?.slice(0, 60) });
      return [...prev, { ...q, timestamp: Date.now() }];
    });
  };

  const markQuestionAnswered = (request_id: string, answerText: string) => {
    setAllQuestions((prev) =>
      prev.map((q) => (q.request_id === request_id ? { ...q, answer: answerText } : q)),
    );
    completeActivity('feedback');
  };

  const [selectedDataClass, setSelectedDataClass] = createSignal<string>('PUBLIC');

  const [toolDefinitions, setToolDefinitions] = createSignal<any[]>([]);

  const [streamingContent, setStreamingContent] = createSignal<string>('');
  const [isStreaming, setIsStreaming] = createSignal(false);

  // Activity feed: tracks what the agent is currently doing
  type ActivityItem = {
    id: string;
    kind: 'tool' | 'model' | 'skill' | 'inference' | 'feedback';
    label: string;
    detail?: string;
    startedAt: number;
    done: boolean;
    error?: boolean;
  };
  const [activities, setActivities] = createSignal<ActivityItem[]>([]);

  // Tool call history: accumulates during streaming, committed per assistant message.
  type ToolCallRecord = {
    id: string;
    tool_id: string;
    label: string;
    input?: string;
    output?: string;
    isError: boolean;
    startedAt: number;
    completedAt?: number;
  };
  const [pendingToolCalls, setPendingToolCalls] = createSignal<ToolCallRecord[]>([]);
  const [toolCallHistory, setToolCallHistory] = createSignal<Record<string, ToolCallRecord[]>>({});

  const pushActivity = (item: Omit<ActivityItem, 'startedAt' | 'done'>) => {
    setActivities(prev => [...prev.filter(a => a.id !== item.id), { ...item, startedAt: Date.now(), done: false }]);
  };
  const completeActivity = (id: string, error?: boolean) => {
    setActivities(prev => prev.map(a => a.id === id ? { ...a, done: true, error } : a));
    // Remove completed activities after a short delay
    setTimeout(() => setActivities(prev => prev.filter(a => a.id !== id)), 2000);
  };
  const tryParseJson = (s: string | undefined): any => { try { return s ? JSON.parse(s) : undefined; } catch { return undefined; } };
  const truncate = (s: string | undefined, n: number) => s && s.length > n ? s.slice(0, n) + '…' : s;

  const recordToolCallStart = (activityId: string, tool_id: string, label: string, input?: string) => {
    setPendingToolCalls(prev => [...prev, { id: activityId, tool_id, label, input, isError: false, startedAt: Date.now() }]);
  };
  const recordToolCallResult = (tool_id: string, output?: string, isError?: boolean) => {
    setPendingToolCalls(prev => {
      const idx = prev.findIndex(tc => tc.tool_id === tool_id && !tc.completedAt);
      if (idx < 0) return prev;
      const updated = [...prev];
      updated[idx] = { ...updated[idx], output, isError: isError ?? false, completedAt: Date.now() };
      return updated;
    });
  };
  const commitToolCalls = (messageId: string) => {
    const calls = pendingToolCalls();
    if (calls.length === 0) return;
    setToolCallHistory(prev => ({ ...prev, [messageId]: calls }));
    setPendingToolCalls([]);
  };

  const clearStreamingState = () => {
    batch(() => {
      setIsStreaming(false);
      setStreamingContent('');
      setActivities([]);
      setPendingToolCalls([]);
    });
  };

  const beginStreamingState = () => {
    batch(() => {
      setIsStreaming(true);
      setStreamingContent('');
      setActivities([]);
      setPendingToolCalls([]);
      pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
    });
  };

  let streamSyncEpoch = 0;
  const syncChatStateAfterStream = (session_id: string) => {
    // Capture accumulated tool calls before clearing streaming state — the
    // assistant message ID isn't known until the post-sync session arrives.
    const capturedCalls = pendingToolCalls().slice();
    const epoch = ++streamSyncEpoch;
    clearStreamingState();
    void syncChatState(session_id).then(() => {
      if (capturedCalls.length > 0) {
        const s = session();
        if (s) {
          const lastAssistant = [...s.messages].reverse().find(m => m.role === 'assistant');
          if (lastAssistant) {
            setToolCallHistory(prev => ({ ...prev, [lastAssistant.id]: capturedCalls }));
          }
        }
      }
    }).finally(() => {
      if (epoch === streamSyncEpoch) {
        clearStreamingState();
      }
    });
  };

  createEffect(() => {
    const sid = selectedSessionId();
    console.debug('[session-switch] clearing questions for session change →', sid);
    setActiveTab('chat');
    setSelectedEntryPath(null);
    setSelectedFilePath(null);
    setFileContent(null);
    setFileEditorContent('');
    setWorkspaceFiles([]);
    setWorkspaceLoadedForSession(null);
    setAllQuestions([]);
    setToolCallHistory({});
  });

  createEffect(() => {
    if (activeTab() === 'workspace' && selectedSessionId() && workspaceLoadedForSession() !== selectedSessionId()) {
      void loadWorkspaceFiles();
    }
  });

  // Eagerly load workspace file listing when a session is selected (for @-mention autocomplete)
  createEffect(() => {
    const sid = selectedSessionId();
    if (sid && workspaceLoadedForSession() !== sid) {
      void loadWorkspaceFiles();
    }
  });

  // Subscribe to index-status SSE when session changes
  createEffect(() => {
    const currentSessionId = selectedSessionId();
    const _epoch = daemonEpoch(); // re-subscribe on daemon reconnect
    let indexUnlisten: UnlistenFn | null = null;
    let disposed = false;

    if (!currentSessionId) {
      setIndexStatus({});
      return;
    }

    const setup = async () => {
      // Fetch initial snapshot
      try {
        const files = await invoke<string[]>('workspace_indexed_files', { session_id: currentSessionId });
        if (disposed) return;
        const status: Record<string, 'queued' | 'indexed'> = {};
        for (const f of files) status[f] = 'indexed';
        setIndexStatus(status);
      } catch {
        if (!disposed) setIndexStatus({});
      }

      // Start SSE bridge
      try {
        await invoke('workspace_subscribe_index_status', { session_id: currentSessionId });
      } catch { /* stream may not be available */ }

      if (disposed) return;

      // Listen for Tauri events
      indexUnlisten = await listen<{ session_id: string; event: any }>('index:event', (e) => {
        if (e.payload.session_id !== currentSessionId) return;
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
    };

    void setup();

    onCleanup(() => {
      disposed = true;
      indexUnlisten?.();
    });
  });

  createEffect(() => {
    const sid = selectedSessionId();
    const s = session();
    // Restore the active persona from the session's persona_id.
    if (sid && s && s.persona_id) {
      setSelectedAgentId(s.persona_id);
    } else {
      setSelectedAgentId('system/general');
    }
  });

  const availableModels= createMemo(() => {
    const router = modelRouter();
    if (!router) return [];
    const models: { id: string; label: string }[] = [];
    for (const provider of router.providers) {
      if (!provider.available) continue;
      for (const model of provider.models) {
        models.push({
          id: `${provider.id}:${model}`,
          label: `${model} (${provider.name || provider.id})`,
        });
      }
    }
    return models;
  });

  // Derived lists for bots page
  const modelIdList = createMemo(() => availableModels().map(m => m.id));
  const toolIdList = createMemo(() => tools().map(t => t.name));

  // ── Editable config state (extracted to configStore) ──────────
  const {
    editConfig, setEditConfig,
    savedConfig, setSavedConfig,
    configSaveMsg, setConfigSaveMsg,
    editingProviderIdx, setEditingProviderIdx,
    pendingKeyringDeletes, setPendingKeyringDeletes,
    configDirty,
    configLoadError,
    loadEditConfig,
    loadToolDefinitions,
    saveConfig,
    updateDaemon, updateApi, updateOverridePolicy, updatePromptInjection,
    updateLocalModels, updateCompaction, updateAfk,
    handleSetUserStatus,
    updateProvider, addProvider, removeProvider, moveProvider,
    addModelToProvider, removeModelFromProvider,
  } = createConfigStore({ activeScreen, loadPersonas: () => loadPersonas(), setToolDefinitions, setUserStatus });

  const daemonOnline = createMemo(() => daemonStatus() !== null);

  // Epoch counter that increments on each offline→online transition.
  // Used as a dependency in effects that set up SSE subscriptions or load
  // state from the daemon, so they automatically re-fire on reconnect.
  const [daemonEpoch, setDaemonEpoch] = createSignal(0);
  let prevOnline = false;
  createEffect(() => {
    const online = daemonOnline();
    if (online && !prevOnline) {
      setDaemonEpoch(e => e + 1);
    }
    prevOnline = online;
  });

  // Re-subscribe the interactions SSE when the daemon reconnects so
  // pending questions keep flowing even after a daemon restart.
  createEffect(() => {
    const _epoch = daemonEpoch(); // re-subscribe on daemon reconnect
    interactionStore.resubscribe();
  });
  const activeSessionState = createMemo<ChatRunState | null>(() => session()?.state ?? null);
  const queueCount = createMemo(() => session()?.queued_count ?? 0);
  const queuedMessages = createMemo(() =>
    (session()?.messages ?? []).filter((m) => m.status === 'queued' || m.status === 'processing')
  );

  const clearChatState = () => {
    clearStreamingState();
    setSessions([]);
    setSession(null);
    setSelectedSessionId(null);
    setSessionMemory([]);
    setMemoryResults([]);
    setRiskScans([]);
    setModelRouter(null);
    setMcpServers([]);
    setSelectedMcpServerId(null);
    setMcpTools([]);
    setMcpResources([]);
    setMcpPrompts([]);
    setMcpNotifications([]);
    setTools([]);
    setPendingReview(null);
    setPendingReviewContent(null);
  };

  const refreshStatus = async () => {
    const nextStatus = await invoke<DaemonStatus | null>('daemon_status');
    setDaemonStatus(nextStatus);
    setLastUpdated(new Date().toLocaleTimeString());
    return nextStatus;
  };

  const refreshConfig = async () => {
    setConfigYaml(await invoke<string>('config_show'));
  };

  const refreshContext = async () => {
    setContext(await invoke<AppContext>('app_context'));
  };

  const loadModelRouter = async () => {
    setModelRouter(await invoke<ModelRouterSnapshot>('model_router_snapshot'));
  };

  const loadMcpServers = async () => {
    if (!daemonOnline()) {
      setMcpServers([]);
      return;
    }
    setMcpServers(await invoke<McpServerSnapshot[]>('mcp_list_servers'));
  };

  const loadMcpNotifications = async () => {
    if (!daemonOnline()) {
      setMcpNotifications([]);
      return;
    }
    setMcpNotifications(await invoke<McpNotificationEvent[]>('mcp_list_notifications', { limit: 30 }));
  };

  const loadMcpInventory = async (server_id: string) => {
    if (!daemonOnline()) {
      setMcpTools([]);
      setMcpResources([]);
      setMcpPrompts([]);
      return;
    }

    const [toolsResult, resourcesResult, promptsResult] = await Promise.allSettled([
      invoke<McpToolInfo[]>('mcp_list_tools', { server_id }),
      invoke<McpResourceInfo[]>('mcp_list_resources', { server_id }),
      invoke<McpPromptInfo[]>('mcp_list_prompts', { server_id }),
    ]);
    setMcpTools(toolsResult.status === 'fulfilled' ? toolsResult.value : []);
    setMcpResources(resourcesResult.status === 'fulfilled' ? resourcesResult.value : []);
    setMcpPrompts(promptsResult.status === 'fulfilled' ? promptsResult.value : []);
  };

  const loadTools = async () => {
    if (!daemonOnline()) {
      setTools([]);
      return;
    }
    setTools(await invoke<ToolDefinition[]>('tools_list'));
  };

  const loadChannels = async () => {
    try {
      const data = await invoke<any[]>('list_connectors');
      const arr = Array.isArray(data) ? data : [];
      setChannels(arr.map((c: any) => ({
        id: c.id,
        name: c.name,
        provider: typeof c.provider === 'string' ? c.provider : (c.provider ?? ''),
        hasComms: Array.isArray(c.enabled_services)
          ? c.enabled_services.includes('communication')
          : c.services?.communication != null
            ? (c.services.communication.enabled !== false)
            : false,
      })));
    } catch (e) { console.error('Failed to load channels:', e); }
  };

  const loadEventTopics = async () => {
    try {
      const url = context()?.daemon_url;
      if (!url) return;
      const resp = await authFetch(`${url}/api/v1/workflows/topics`);
      if (resp.ok) {
        const data = await resp.json();
        setEventTopics(Array.isArray(data?.topics) ? data.topics : []);
      }
    } catch (e) { console.error('Failed to load event topics:', e); }
  };

  const loadInstalledSkills = async () => {
    try {
      const persona_id = selectedAgentId();
      setInstalledSkills(await invoke<InstalledSkill[]>('skills_list_installed_for_persona', { persona_id }));
    } catch (e) {
      console.error('Failed to load installed skills:', e);
    }
  };

  const loadPersonas = async () => {
    try {
      const defs = await invoke<Persona[]>('list_personas');
      setPersonas(defs);
      if (selectedAgentId() !== 'system/general' && !defs.some((agent) => agent.id === selectedAgentId())) {
        setSelectedAgentId('system/general');
      }
    } catch (e) {
      console.error('Failed to load agent personas:', e);
    }
  };

  const selectMcpServer = async (server_id: string) => {
    setSelectedMcpServerId(server_id);
    await runAction('mcp-inspect', async () => {
      await loadMcpInventory(server_id);
    });
  };

  const connectMcpServer = async (server_id: string) => {
    await runAction('mcp-connect', async () => {
      await invoke<McpServerSnapshot>('mcp_connect_server', { server_id });
      await loadMcpServers();
      await loadMcpInventory(server_id);
    });
  };

  const disconnectMcpServer = async (server_id: string) => {
    await runAction('mcp-disconnect', async () => {
      await invoke<McpServerSnapshot>('mcp_disconnect_server', { server_id });
      await loadMcpServers();
      if (selectedMcpServerId() === server_id) {
        setMcpTools([]);
        setMcpResources([]);
        setMcpPrompts([]);
      }
    });
  };

  const fetchMcpServerLogs = async (server_id: string) => {
    try {
      const logs = await invoke<McpServerLog[]>('mcp_server_logs', { server_id });
      setMcpServerLogs(logs);
      setMcpLogsServerId(server_id);
    } catch {
      setMcpServerLogs([]);
      setMcpLogsServerId(server_id);
    }
  };

  const loadSessionMemory = async (session_id: string) => {
    setSessionMemory(await invoke<ChatMemoryItem[]>('chat_get_session_memory', { session_id }));
  };

  const loadRiskScans = async (session_id: string) => {
    setRiskScans(await invoke<RiskScanRecord[]>('chat_list_risk_scans', { session_id }));
  };

  const runMemorySearch = async (query = memoryQuery().trim()) => {
    if (!daemonOnline() || !query) {
      setMemoryResults([]);
      return;
    }

    setMemoryResults(await invoke<ChatMemoryItem[]>('memory_search', { query }));
  };

  // ── Knowledge-graph helpers ─────────────────────────────────────────────

  const kgFetch = async <T,>(path: string, init?: RequestInit): Promise<T | null> => {
    const url = context()?.daemon_url;
    if (!url) throw new Error('daemon offline');
    const resp = await authFetch(`${url}${path}`, init);
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `${resp.status}`);
    }
    if (resp.status === 204) return null;
    return resp.json() as Promise<T>;
  };

  const loadKgStats = async () => {
    if (!daemonOnline()) { setKgStats(null); return; }
    setKgStats(await kgFetch<KgStats>('/api/v1/knowledge/stats'));
  };

  const loadKgNodes = async () => {
    if (!daemonOnline()) { setKgNodes([]); return; }
    const typeFilter = kgNodeTypeFilter().trim();
    const params = new URLSearchParams();
    if (typeFilter) params.set('node_type', typeFilter);
    params.set('limit', '50');
    setKgNodes(await kgFetch<KgNode[]>(`/api/v1/knowledge/nodes?${params}`) ?? []);
  };

  const loadKgNode = async (nodeId: number) => {
    if (!daemonOnline()) { setKgSelectedNode(null); return; }
    setKgSelectedNode(await kgFetch<KgNodeWithEdges>(`/api/v1/knowledge/nodes/${nodeId}`));
  };

  const runKgSearch = async () => {
    const q = kgSearchQuery().trim();
    if (!daemonOnline() || !q) { setKgSearchResults([]); return; }
    const params = new URLSearchParams({ q, limit: '20' });
    setKgSearchResults(await kgFetch<KgNode[]>(`/api/v1/knowledge/search?${params}`) ?? []);
  };

  const kgCreateNode = async () => {
    const node_type = kgNewNodeType().trim();
    const name = kgNewNodeName().trim();
    if (!node_type || !name) return;
    await runAction('kg-create-node', async () => {
      await kgFetch<{ id: number }>('/api/v1/knowledge/nodes', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          node_type: node_type,
          name,
          data_class: kgNewNodeDataClass(),
          content: kgNewNodeContent().trim() || null,
        }),
      });
      setKgNewNodeType('');
      setKgNewNodeName('');
      setKgNewNodeContent('');
      setKgNewNodeDataClass('INTERNAL');
      await Promise.all([loadKgNodes(), loadKgStats()]);
    });
  };

  const kgDeleteNode = async (nodeId: number) => {
    await runAction('kg-delete-node', async () => {
      await kgFetch(`/api/v1/knowledge/nodes/${nodeId}`, { method: 'DELETE' });
      if (kgSelectedNode()?.id === nodeId) setKgSelectedNode(null);
      await Promise.all([loadKgNodes(), loadKgStats()]);
    });
  };

  const kgCreateEdge = async (source_id: number) => {
    const targetId = parseInt(kgNewEdgeTargetId().trim(), 10);
    const edgeType = kgNewEdgeType().trim();
    if (isNaN(targetId) || !edgeType) return;
    await runAction('kg-create-edge', async () => {
      await kgFetch<{ id: number }>('/api/v1/knowledge/edges', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ source_id: source_id, target_id: targetId, edge_type: edgeType }),
      });
      setKgNewEdgeTargetId('');
      setKgNewEdgeType('');
      await Promise.all([loadKgNode(source_id), loadKgStats()]);
    });
  };

  const kgDeleteEdge = async (edgeId: number) => {
    await runAction('kg-delete-edge', async () => {
      await kgFetch(`/api/v1/knowledge/edges/${edgeId}`, { method: 'DELETE' });
      const sel = kgSelectedNode();
      if (sel) await loadKgNode(sel.id);
      await loadKgStats();
    });
  };

  const syncChatState = async (desiredSessionId?: string | null) => {
    if (!daemonOnline()) {
      clearChatState();
      return;
    }

    setPendingReview(null);
    setPendingReviewContent(null);

    const list = (await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? [];
    updateSessions(list);

    // For auto-selection purposes, only consider non-bot sessions.
    const interactiveSessions = list.filter(s => !s.bot_id);

    let nextId = desiredSessionId ?? selectedSessionId();
    if (nextId && !list.some((candidate) => candidate.id === nextId)) {
      nextId = null;
    }

    if (!nextId) {
      if (interactiveSessions.length === 0) {
        const created = await invoke<ChatSessionSnapshot>('chat_create_session', { modality: 'linear' });
        setSelectedSessionId(created.id);
        setSession(created);
        updateSessions((await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? []);
        await loadSessionMemory(created.id);
        await loadRiskScans(created.id);
        return;
      }

      nextId = interactiveSessions[0].id;
    }

    const nextSession = await invoke<ChatSessionSnapshot>('chat_get_session', { session_id: nextId });

    // Avoid replacing the session object when nothing changed — a new object
    // reference causes SolidJS to re-render every message, which kills text
    // selection and any in-progress user interaction.
    const prev = session();
    const lastPrev = prev && prev.messages.length > 0 ? prev.messages[prev.messages.length - 1] : null;
    const lastNext = nextSession.messages.length > 0 ? nextSession.messages[nextSession.messages.length - 1] : null;
    const changed =
      !prev ||
      prev.id !== nextSession.id ||
      prev.state !== nextSession.state ||
      prev.messages.length !== nextSession.messages.length ||
      prev.active_stage !== nextSession.active_stage ||
      prev.active_intent !== nextSession.active_intent ||
      prev.active_thinking !== nextSession.active_thinking ||
      prev.updated_at_ms !== nextSession.updated_at_ms ||
      (lastPrev && lastNext && (
        lastPrev.created_at_ms !== lastNext.created_at_ms ||
        lastPrev.updated_at_ms !== lastNext.updated_at_ms
      ));

    batch(() => {
      const prevId = selectedSessionId();
      setSelectedSessionId(nextId);
      if (changed) {
        setSession(nextSession);
      }
      // Only reset tool/skill exclusions when switching to a different session.
      if (nextId !== prevId) {
        setExcludedTools([]);
        setExcludedSkills([]);
      }
      if (nextSession.state !== 'running') {
        clearStreamingState();
      } else if (!isStreaming()) {
        // Re-enter streaming mode so the bubble reappears when returning
        // to a session that is still running.
        beginStreamingState();
      }
    });
    await loadSessionMemory(nextId);
    await loadRiskScans(nextId);

    // Hydrate allQuestions from question messages embedded in the session
    // snapshot so that AgentStage badges and FlightDeck panels stay accurate.
    if (nextSession && nextId) {
      const questionEntries: PendingQuestionEntry[] = [];
      for (const msg of nextSession.messages) {
        if (msg.interaction_request_id && msg.interaction_kind === 'question') {
          const meta = (msg.interaction_meta ?? {}) as Record<string, any>;
          questionEntries.push({
            request_id: msg.interaction_request_id,
            text: msg.content ?? '',
            choices: Array.isArray(meta.choices) ? meta.choices : [],
            allow_freeform: meta.allow_freeform !== false,
            multi_select: meta.multi_select === true,
            session_id: nextId,
            agent_id: typeof meta.agent_id === 'string' ? meta.agent_id : undefined,
            agent_name: typeof meta.agent_name === 'string' ? meta.agent_name : undefined,
            message: typeof meta.message === 'string' ? meta.message : undefined,
            workflow_instance_id: typeof meta.workflow_instance_id === 'number' ? meta.workflow_instance_id : undefined,
            workflow_step_id: typeof meta.workflow_step_id === 'string' ? meta.workflow_step_id : undefined,
            timestamp: msg.created_at_ms ?? Date.now(),
            answer: msg.interaction_answer ?? undefined,
          });
        }
      }
      setAllQuestions(questionEntries);
    }
  };

  // Questions are now delivered as messages through the normal SSE path,
  // so the periodic polling loop for pending questions is no longer needed.

  const refreshAll = async () => {
    const nextStatus = await refreshStatus();
    await Promise.all([refreshConfig(), refreshContext(), loadPersonas()]);

    if (nextStatus) {
      await Promise.all([
        syncChatState(selectedSessionId()),
        loadModelRouter(),
        loadMcpServers(),
        loadMcpNotifications(),
        loadTools(),
        loadChannels(),
        loadEventTopics(),
        loadInstalledSkills(),
        loadLocalModels().catch(() => {}),
      ]);
    } else {
      setModelRouter(null);
      clearChatState();
    }
  };

  const runAction = async (name: string, action: () => Promise<void>) => {
    setBusyAction(name);
    setErrorMessage(null);
    try {
      await action();
    } catch (error) {
      if (!isTauriInternalError(error)) {
        setErrorMessage(String(error));
      }
    } finally {
      setBusyAction(null);
    }
  };

  const createSession = async (modality: SessionModality = 'linear', workspace_path?: string, persona_id?: string) => {
    if (busyAction() !== null) return;
    await runAction('new-session', async () => {
      const created = await invoke<ChatSessionSnapshot>('chat_create_session', { modality, persona_id });
      if (workspace_path) {
        await invoke('chat_link_workspace', { session_id: created.id, path: workspace_path });
      }
      setActiveScreen('session');
      setSessionEntityType('session');
      setSelectedSessionId(created.id);
      setSession(workspace_path
        ? await invoke<ChatSessionSnapshot>('chat_get_session', { session_id: created.id })
        : created);
      updateSessions((await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? []);
      await loadSessionMemory(created.id);
      await loadRiskScans(created.id);
      setShowNewSessionDialog(false);
    });
  };

  const sendMessage = async (decision?: ScanDecision, options?: { skipPreempt?: boolean }) => {
    if (busyAction() !== null) return;
    let content = (decision ? pendingReviewContent() ?? draft() : draft()).trim();
    let attachments = [...pendingAttachments()];
    if ((!content && attachments.length === 0) || !daemonOnline()) {
      return;
    }

    // Detect /queue prefix: strip it and force skip_preempt.
    let skipPreempt = options?.skipPreempt;
    const queueMatch = content.match(/^\/queue\s+([\s\S]*)/i);
    if (queueMatch) {
      content = queueMatch[1].trim();
      skipPreempt = true;
      if (!content && attachments.length === 0) return;
    }

    // Resolve @[path] file references: for regular sessions, attach as
    // MessageAttachments (the backend routes text/* to ContentPart::Text).
    // For bot sessions (which only support plain content), embed inline.
    const atMentionPattern = /@\[([^\]]+)\]/g;
    const mentionMatches = [...content.matchAll(atMentionPattern)];
    if (mentionMatches.length > 0) {
      const isBot = sessionEntityType() === 'bot';
      const sid = selectedSessionId();
      const seen = new Set<string>();
      const fileAttachments: MessageAttachment[] = [];
      const fileBlocks: string[] = [];

      for (const m of mentionMatches) {
        const filePath = m[1];
        if (seen.has(filePath)) continue;
        seen.add(filePath);
        try {
          const fileData = isBot && sid
            ? await invoke<any>('bot_workspace_read_file', { bot_id: sid, path: filePath })
            : sid
              ? await invoke<any>('workspace_read_file', { session_id: sid, path: filePath })
              : null;
          if (fileData && !fileData.is_binary && fileData.content) {
            if (isBot) {
              // Bots only support plain content — embed inline.
              fileBlocks.push(`\n<file path="${filePath}">\n${fileData.content}\n</file>`);
            } else {
              // Regular sessions: create a proper attachment.
              const ext = filePath.split('.').pop()?.toLowerCase() ?? '';
              const mimeMap: Record<string, string> = {
                ts: 'text/typescript', tsx: 'text/typescript', js: 'text/javascript',
                jsx: 'text/javascript', py: 'text/x-python', rs: 'text/x-rust',
                go: 'text/x-go', md: 'text/markdown', json: 'text/json',
                yaml: 'text/yaml', yml: 'text/yaml', toml: 'text/toml',
                html: 'text/html', css: 'text/css', sql: 'text/sql',
                sh: 'text/x-shellscript', xml: 'text/xml', csv: 'text/csv',
              };
              const mediaType = mimeMap[ext] ?? 'text/plain';
              fileAttachments.push({
                id: `at-${fileAttachments.length}`,
                filename: filePath,
                media_type: mediaType,
                data: btoa(unescape(encodeURIComponent(fileData.content))),
              });
            }
          }
        } catch {
          // File read failed — leave the token as-is for context
        }
      }

      // Replace @[path] tokens with plain path references in the message
      content = content.replace(atMentionPattern, (_match, path) => `\`${path}\``);

      // For bots: append inline file contents
      if (fileBlocks.length > 0) {
        content += '\n\n---\nReferenced files:' + fileBlocks.join('');
      }

      // For regular sessions: merge file attachments with any pending attachments
      if (fileAttachments.length > 0) {
        attachments.push(...fileAttachments);
      }
    }

    const agent_id = selectedAgentId();
    let session_id = selectedSessionId();
    if (!session_id) {
      const persona_id = agent_id !== 'general' ? agent_id: undefined;
      const created = await invoke<ChatSessionSnapshot>('chat_create_session', { modality: 'linear', persona_id });
      session_id = created.id;
      setSelectedSessionId(created.id);
      setSession(created);
    }

    await runAction('send', async () => {
      // For bot sessions, route through the bot message API.
      if (sessionEntityType() === 'bot') {
        // Reject messages to bots that have finished.
        const bot = botStore.bots().find(b => b.config.id === session_id);
        if (bot?.status === 'done' || bot?.status === 'error') return;
        await invoke('message_bot', { agent_id: session_id, content });
        setDraft('');
        setPendingAttachments([]);
        setPendingCanvasPosition(null);
        // Refresh session to show the new message.
        try {
          const updated = await invoke<ChatSessionSnapshot>('chat_get_session', { session_id });
          setSession(updated);
        } catch { /* bot session may not exist yet */ }
        return;
      }

      const response = await invoke<SendMessageResponse>('chat_send_message', {
        session_id,
        content,
        agent_id: agent_id !== 'general' ? agent_id: undefined,
        scan_decision: decision ?? null,
        data_class_override: selectedDataClass(),
        canvas_position: pendingCanvasPosition(),
        excluded_tools: excludedTools().length > 0 ? excludedTools() : undefined,
        excluded_skills: excludedSkills().length > 0 ? excludedSkills() : undefined,
        attachments: attachments.length > 0 ? attachments : undefined,
        skip_preempt: skipPreempt || undefined,
      });

      // Always clear input state after a successful send
      setDraft('');
      setPendingAttachments([]);
      setPendingCanvasPosition(null);

      if (response.kind === 'queued') {
        setPendingReview(null);
        setPendingReviewContent(null);
        setSession(response.session);
        updateSessions((await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? []);
        await loadSessionMemory(session_id!);
        await loadRiskScans(session_id!);

        // Subscribe to streaming events
        beginStreamingState();
        invoke('chat_subscribe_stream', { session_id }).catch(console.error);
      } else if (response.kind === 'review-required') {
        setPendingReview(response.review);
        setPendingReviewContent(content);
        await loadRiskScans(session_id!);
        return;
      } else if (response.kind === 'blocked') {
        setPendingReview(null);
        setPendingReviewContent(null);
        setErrorMessage(response.reason);
        await loadRiskScans(session_id!);
      }
    });
  };

  const interrupt = async (mode: 'soft' | 'hard') => {
    const session_id = selectedSessionId();
    if (!session_id) {
      return;
    }

    await runAction(mode === 'soft' ? 'pause' : 'stop', async () => {
      const snapshot = await invoke<ChatSessionSnapshot>('chat_interrupt', {
        session_id,
        mode,
      });
      setSession(snapshot);
      updateSessions((await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? []);
      await loadSessionMemory(session_id);
      await loadRiskScans(session_id);
    });
  };

  const resume = async () => {
    const session_id = selectedSessionId();
    if (!session_id) {
      return;
    }

    await runAction('resume', async () => {
      const snapshot = await invoke<ChatSessionSnapshot>('chat_resume', { session_id });
      setSession(snapshot);
      updateSessions((await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? []);
      await loadSessionMemory(session_id);
      await loadRiskScans(session_id);
    });
  };

  const selectSession = async (session_id: string) => {
    await runAction('select-session', async () => {
      setSessionEntityType('session');
      await syncChatState(session_id);
    });
  };

  // Select a bot and load its backing session into the ChatView.
  const selectBotSession = async (bot_id: string) => {
    botStore.selectBot(bot_id);
    setSessionEntityType('bot');
    // The bot's session ID equals the bot ID.
    let botSession: ChatSessionSnapshot;
    try {
      botSession = await invoke<ChatSessionSnapshot>('chat_get_session', { session_id: bot_id });
    } catch {
      // Bot may not have a session yet (pre-launch). Nothing to display.
      return;
    }
    batch(() => {
      const prevId = selectedSessionId();
      setSelectedSessionId(bot_id);
      setSession(botSession);
      if (bot_id !== prevId) {
        setExcludedTools([]);
        setExcludedSkills([]);
      }
      if (botSession.state !== 'running') {
        clearStreamingState();
      }
    });
    await loadSessionMemory(bot_id);
    await loadRiskScans(bot_id);

    // Hydrate allQuestions from question messages in the bot session snapshot.
    const questionEntries: PendingQuestionEntry[] = [];
    for (const msg of botSession.messages) {
      if (msg.interaction_request_id && msg.interaction_kind === 'question') {
        const meta = (msg.interaction_meta ?? {}) as Record<string, any>;
        questionEntries.push({
          request_id: msg.interaction_request_id,
          text: msg.content ?? '',
          choices: Array.isArray(meta.choices) ? meta.choices : [],
          allow_freeform: meta.allow_freeform !== false,
          multi_select: meta.multi_select === true,
          session_id: bot_id,
          agent_id: typeof meta.agent_id === 'string' ? meta.agent_id : undefined,
          agent_name: typeof meta.agent_name === 'string' ? meta.agent_name : undefined,
          message: typeof meta.message === 'string' ? meta.message : undefined,
          workflow_instance_id: typeof meta.workflow_instance_id === 'number' ? meta.workflow_instance_id : undefined,
          workflow_step_id: typeof meta.workflow_step_id === 'string' ? meta.workflow_step_id : undefined,
          timestamp: msg.created_at_ms ?? Date.now(),
          answer: msg.interaction_answer ?? undefined,
        });
      }
    }
    setAllQuestions(questionEntries);
  };

  const deleteSession = async (session_id: string, scrubKb: boolean = false) => {
    if (busyAction() !== null) return;
    await runAction('delete-session', async () => {
      await invoke<void>('chat_delete_session', { session_id, scrub_kb: scrubKb });
      if (selectedSessionId() === session_id) {
        setSelectedSessionId(null);
      }
      await syncChatState();
    });
  };

  const renameSession = async (session_id: string, title: string) => {
    try {
      await invoke<ChatSessionSnapshot>('chat_rename_session', { session_id, title });
      const list = (await invoke<ChatSessionSummary[] | null>('chat_list_sessions')) ?? [];
      updateSessions(list);
    } catch (error) {
      setErrorMessage(String(error));
    }
  };

  const uploadFiles = async () => {
    const session_id = selectedSessionId();
    if (!session_id) {
      return;
    }

    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = true;
    input.onchange = () => {
      const files = input.files;
      if (!files?.length) {
        return;
      }

      void runAction('upload-files', async () => {
        const failures: string[] = [];
        for (const file of Array.from(files)) {
          try {
            await invoke('chat_upload_file', {
              session_id,
              file_name: file.name,
              content: await fileToBase64(file),
            });
          } catch (error) {
            failures.push(`Failed to upload ${file.name}: ${String(error)}`);
          }
        }

        await syncChatState(session_id);
        if (failures.length > 0) {
          setErrorMessage(failures.join('\n'));
        }
      });
    };
    input.click();
  };

  const loadWorkspaceFiles = async () => {
    const session_id = selectedSessionId();
    if (!session_id) {
      return;
    }

    setWorkspaceLoading(true);
    try {
      const isBot = sessionEntityType() === 'bot';
      const files = isBot
        ? await invoke<any[]>('bot_workspace_list_files', { bot_id: session_id })
        : await invoke<any[]>('workspace_list_files', { session_id });
      setWorkspaceFiles(files);
      setWorkspaceLoadedForSession(session_id);
    } catch (error) {
      console.error('Failed to load workspace files:', error);
      setWorkspaceFiles([]);
      setWorkspaceLoadedForSession(null);
    } finally {
      setWorkspaceLoading(false);
    }
  };

  /** Lazily load the children of a directory and merge them into the tree. */
  const loadDirectoryChildren = async (dirPath: string): Promise<void> => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      const isBot = sessionEntityType() === 'bot';
      const children = isBot
        ? await invoke<any[]>('bot_workspace_list_files', { bot_id: session_id, path: dirPath })
        : await invoke<any[]>('workspace_list_files', { session_id, path: dirPath });
      setWorkspaceFiles((prev) => mergeChildrenIntoTree(prev, dirPath, children));
    } catch (error) {
      console.error(`Failed to load directory children for ${dirPath}:`, error);
    }
  };

  const loadSessionPerms = async () => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      const perms = await invoke<any>('get_session_permissions', { session_id });
      setSessionPerms(perms);
    } catch (error) {
      console.error('Failed to load session permissions:', error);
    }
  };

  const saveSessionPerms = async () => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      await invoke('set_session_permissions', { session_id, permissions: sessionPerms() });
    } catch (error) {
      console.error('Failed to save session permissions:', error);
    }
  };

  const openWorkspaceFile = async (filePath: string) => {
    const session_id = selectedSessionId();
    if (!session_id) {
      return;
    }

    setSelectedEntryPath(filePath);
    setSelectedFilePath(filePath);
    setFileContent(null);
    setFileEditorContent('');
    try {
      const isBot = sessionEntityType() === 'bot';
      const content = isBot
        ? await invoke<any>('bot_workspace_read_file', { bot_id: session_id, path: filePath })
        : await invoke<any>('workspace_read_file', { session_id, path: filePath });
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
    if (!session_id || !filePath) {
      return;
    }

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

  // ── Context menu & audit state ──────────────────────────────────
  const [contextMenu, setContextMenu] = createSignal<{
    x: number;
    y: number;
    entry: any;
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

  const runFileAudit = async () => {
    const target = auditTarget();
    const sid = selectedSessionId();
    if (!target || !auditModel() || !sid) return;
    setAuditRunning(true);
    setAuditStatus('Running security audit...');
    setAuditResult(null);
    try {
      const result = await invoke<any>('workspace_audit_file', {
        session_id: sid,
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
    const sid = selectedSessionId();
    if (!sid) return;
    try {
      await invoke('workspace_set_classification_override', {
        session_id: sid,
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
    const sid = selectedSessionId();
    if (!sid) return;
    try {
      await invoke('workspace_clear_classification_override', {
        session_id: sid,
        path,
      });
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to clear classification:', e);
    }
    setContextMenu(null);
  };

  const reindexFile = async (path: string) => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    try {
      await invoke('workspace_reindex_file', { session_id, path });
    } catch (e) {
      console.error('Failed to reindex file:', e);
    }
    setContextMenu(null);
  };

  const createNewFolder = async () => {
    const parent = newFolderParent();
    const name = newFolderName().trim();
    if (!parent || !name || !selectedSessionId()) return;
    const folderPath = parent === '.' ? name : `${parent}/${name}`;
    try {
      await invoke('workspace_create_directory', { session_id: selectedSessionId(), path: folderPath });
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
    const filePath = parent === '.' ? name : `${parent}/${name}`;
    try {
      await invoke('workspace_save_file', { session_id: selectedSessionId(), path: filePath, content: '' });
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
    const toPath = toDir === '.' ? fileName : `${toDir}/${fileName}`;
    if (fromPath === toPath) return;
    try {
      await invoke('workspace_move_entry', { session_id: selectedSessionId(), from: fromPath, to: toPath });
      void loadWorkspaceFiles();
    } catch (e) {
      console.error('Failed to move:', e);
    }
  };

  const resolveWorkspacePath = async () => {
    if (session()?.workspace_path) {
      return session()!.workspace_path;
    }
    const session_id = selectedSessionId();
    if (!session_id) return null;
    try {
      const snapshot = await invoke<ChatSessionSnapshot>('chat_get_session', { session_id });
      return snapshot.workspace_path;
    } catch (error) {
      console.error('Failed to resolve workspace path:', error);
      return null;
    }
  };

  const copyToClipboard = async (paths: string[]) => {
    const workspace_path = await resolveWorkspacePath();
    if (!workspace_path || paths.length === 0) return;
    const absolutePaths = paths.map((path) => (path === '.' ? workspace_path: `${workspace_path}/${path}`));
    try {
      await invoke('clipboard_copy_files', { paths: absolutePaths });
    } catch (error) {
      console.error('Failed to copy to clipboard:', error);
    }
  };

  const [pasteProgress, setPasteProgress] = createSignal<{
    current: number;
    total: number;
    fileName: string;
  } | null>(null);

  const [pasteConflict, setPasteConflict] = createSignal<{
    fileName: string;
    destination: string;
  } | null>(null);

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

  const pasteFromClipboard = async (targetDir = '.') => {
    const session_id = selectedSessionId();
    if (!session_id) return;
    let unlistenProgress: UnlistenFn | null = null;
    let unlistenConflict: UnlistenFn | null = null;
    try {
      const sourcePaths = await invoke<string[]>('clipboard_read_file_paths');
      if (sourcePaths.length === 0) return;
      unlistenProgress = await listen<{ session_id: string; current: number; total: number; fileName: string; done?: boolean; cancelled?: boolean }>('paste:progress', (event) => {
        if (event.payload.session_id !== session_id) return;
        if (event.payload.done || event.payload.cancelled) {
          setPasteProgress(null);
        } else {
          setPasteProgress({ current: event.payload.current, total: event.payload.total, fileName: event.payload.fileName });
        }
      });
      unlistenConflict = await listen<{ session_id: string; fileName: string; destination: string }>('paste:conflict', (event) => {
        if (event.payload.session_id !== session_id) return;
        setPasteConflict({ fileName: event.payload.fileName, destination: event.payload.destination });
      });
      await invoke('clipboard_paste_files', { session_id, target_dir: targetDir, source_paths: sourcePaths });
      void loadWorkspaceFiles();
    } catch (error) {
      console.error('Failed to paste from clipboard:', error);
    } finally {
      unlistenProgress?.();
      unlistenConflict?.();
      setPasteProgress(null);
      setPasteConflict(null);
    }
  };

  const loadLocalModels = async () => {
    const summary = await invoke<LocalModelSummary>('local_models_list');
    setLocalModels(summary.models);
    setStorageBytes(summary.total_size_bytes);
  };

  const updateModelParams = async (modelId: string, params: InferenceParams) => {
    try {
      await invoke('local_models_update_params', { model_id: modelId, params });
      await loadLocalModels();
    } catch (e) {
      console.error('Failed to update model params:', e);
    }
  };

  let paramsTimeout: ReturnType<typeof setTimeout> | null = null;
  const updateModelParamsDebounced = (modelId: string, params: InferenceParams) => {
    if (paramsTimeout) clearTimeout(paramsTimeout);
    paramsTimeout = setTimeout(() => updateModelParams(modelId, params), 300);
  };
  onCleanup(() => { if (paramsTimeout) clearTimeout(paramsTimeout); });

  let downloadPollInterval: ReturnType<typeof setInterval> | null = null;

  let downloadPollBusy = false;
  const pollDownloadsOnce = async () => {
    if (downloadPollBusy) return;
    downloadPollBusy = true;
    try {
      const downloads = await invoke<DownloadProgress[]>('local_models_downloads');
      setActiveDownloads(downloads);
      if (downloads.some(d => d.status === 'complete')) {
        await loadLocalModels();
      }
      if (downloads.length === 0 && downloadPollInterval) {
        clearInterval(downloadPollInterval);
        downloadPollInterval = null;
      }
    } catch (e) {
      console.error('Failed to poll downloads:', e);
    } finally {
      downloadPollBusy = false;
    }
  };

  const startDownloadPolling = () => {
    // Fire an immediate poll so the UI shows downloads right away.
    void pollDownloadsOnce();
    if (downloadPollInterval) return;
    downloadPollInterval = setInterval(() => void pollDownloadsOnce(), 3_000);
  };

  const stopDownloadPolling = () => {
    if (downloadPollInterval) {
      clearInterval(downloadPollInterval);
      downloadPollInterval = null;
    }
  };

  const loadHardwareInfo = async () => {
    const [hw, ru, st] = await Promise.allSettled([
      invoke<HardwareSummary>('local_models_hardware'),
      invoke<RuntimeResourceUsage>('local_models_resource_usage'),
      invoke<number>('local_models_storage'),
    ]);
    if (hw.status === 'fulfilled') {
      setHardwareInfo(hw.value.hardware);
      // HardwareSummary also contains usage data
      const u = hw.value.usage;
      setResourceUsage({
        loaded_models: u.models_loaded,
        total_memory_used_bytes: u.ram_used_bytes + u.vram_used_bytes,
        per_model: [],
      });
    }
    if (ru.status === 'fulfilled') setResourceUsage(ru.value);
    if (st.status === 'fulfilled') setStorageBytes(st.value);
  };

  const searchHubModels = async () => {
    const query = hubSearchQuery().trim();
    if (!query) {
      setHubSearchResults([]);
      return;
    }
    setHubSearchLoading(true);
    setHubSearchError(null);
    try {
      const result = await invoke<HubSearchResult>('local_models_search', {
        query,
        task: 'text-generation',
        limit: 20,
      });
      setHubSearchResults(result.models ?? []);
    } catch (e) {
      console.error('Hub search failed:', e);
      setHubSearchError(`${e}`);
      setHubSearchResults([]);
    } finally {
      setHubSearchLoading(false);
    }
  };

  const inferRuntime = (filename: string): string => {
    const lower = filename.toLowerCase();
    if (lower.endsWith('.gguf') || lower.endsWith('.ggml')) return 'llama-cpp';
    if (lower.endsWith('.onnx')) return 'onnx';
    if (lower.endsWith('.safetensors') || lower.endsWith('.bin')) return 'candle';
    return 'llama-cpp';
  };

  const openInstallDialog = async (model: HubModelInfo) => {
    setInstallTargetRepo(model);
    setInstallRepoFiles([]);
    setInstallableItems([]);
    setInstallFilesLoading(true);
    try {
      const result = await invoke<HubRepoFilesResult>('local_models_hub_files', {
        repo_id: model.id,
      });
      const modelFiles = result.files.filter((f: HubFileInfo) => {
        const lower = f.filename.toLowerCase();
        return lower.endsWith('.gguf') || lower.endsWith('.ggml');
      });
      setInstallRepoFiles(modelFiles);
      setInstallableItems(groupInstallableFiles(modelFiles));
    } catch (e) {
      const msg = `${e}`;
      if (isNoTokenError(msg)) {
        setErrorMessage('[HF_NO_TOKEN] You need a HuggingFace access token to download this model.');
      } else if (isLicenseError(msg)) {
        setErrorMessage(msg);
      } else {
        setErrorMessage(`Failed to list repo files: ${msg}`);
      }
    }finally {
      setInstallFilesLoading(false);
    }
  };

  const installModelFile = async (repo_id: string, filename: string) => {
    try {
      setInstallInProgress(true);
      const runtime = inferRuntime(filename);
      await invoke('local_models_install', {
        hub_repo: repo_id,
        filename,
        runtime,
      });
      setInstallTargetRepo(null);
      setInstallRepoFiles([]);
      setInstallableItems([]);
      setSettingsTab('downloads');
      startDownloadPolling();
    } catch (e) {
      const msg = `${e}`;
      if (isNoTokenError(msg)) {
        setErrorMessage('[HF_NO_TOKEN] You need a HuggingFace access token to download this model.');
        scrollToHfToken();
      } else if (isLicenseError(msg)) {
        setErrorMessage(msg);
      } else {
        setErrorMessage(`Install failed: ${msg}`);
      }
    }finally {
      setInstallInProgress(false);
    }
  };

  const removeModel = async (modelId: string) => {
    await invoke('local_models_remove', { model_id: modelId });
    await loadLocalModels();
  };

  const checkFirstRun = async () => {
    try {
      const cfg: HiveMindConfigData = await invoke('config_get');
      if (cfg.setup_completed) return;
      setShowSetupWizard(true);
    } catch { /* config not available yet */ }
  };

  // ── Bot SSE stream: drive chat refresh & processing feedback for bot sessions ──
  createEffect(() => {
    const currentSessionId = selectedSessionId();
    const entityType = sessionEntityType();
    const _epoch = daemonEpoch();
    if (!currentSessionId || entityType !== 'bot') return;

    let botStageUnlisten: UnlistenFn | null = null;
    let disposed = false;
    let refreshDebounce: ReturnType<typeof setTimeout> | undefined;

    const debouncedSync = () => {
      if (refreshDebounce) clearTimeout(refreshDebounce);
      refreshDebounce = setTimeout(() => {
        const sid = selectedSessionId();
        if (!disposed && sid) void syncChatState(sid);
      }, 300);
    };

    (async () => {
      try {
        await invoke('ensure_bot_stream');
      } catch { /* may already be running */ }

      if (disposed) return;

      botStageUnlisten = await listen<{ session_id: string; event: any }>(
        'stage:event',
        (ev) => {
          if (ev.payload.session_id !== '__service__') return;
          const event = ev.payload.event;
          if (!event?.type) return;

          // Filter: only process events related to the selected bot.
          // agent_id on spawned/status/output/completed/task events matches
          // the bot id or one of its child agents — but the bot itself has
          // agent_id equal to the session id.  For simplicity we process all
          // service-level events while a bot is selected (the sync call is
          // scoped to the selected session anyway).

          switch (event.type) {
            case 'agent_output': {
              const inner = event.event;
              if (!inner) break;
              switch (inner.type) {
                case 'model_call_started':
                  if (!isStreaming()) beginStreamingState();
                  pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
                  break;
                case 'model_call_completed':
                  completeActivity('inference');
                  debouncedSync();
                  break;
                case 'token_delta':
                  if (!isStreaming()) beginStreamingState();
                  completeActivity('inference');
                  if (inner.token) {
                    setStreamingContent(prev => prev + inner.token);
                  }
                  break;
                case 'tool_call_started': {
                  const tool_id = inner.tool_id ?? 'unknown';
                  const actId = `tool:${tool_id}:${Date.now()}`;
                  pushActivity({ id: actId, kind: 'tool', label: `Running ${tool_id}`, detail: truncate(JSON.stringify(inner.input ?? ''), 80) });
                  recordToolCallStart(actId, tool_id, `Running ${tool_id}`, JSON.stringify(inner.input ?? ''));
                  break;
                }
                case 'tool_call_completed': {
                  const tool_id = inner.tool_id ?? 'unknown';
                  const match = activities().filter(a => a.id.startsWith(`tool:${tool_id}:`) && !a.done).pop();
                  if (match) completeActivity(match.id, inner.is_error);
                  recordToolCallResult(tool_id, JSON.stringify(inner.output ?? ''), inner.is_error ?? false);
                  pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
                  break;
                }
                case 'completed':
                case 'failed':
                  syncChatStateAfterStream(currentSessionId);
                  break;
                case 'user_interaction_required': {
                  completeActivity('inference');
                  pushActivity({ id: 'feedback', kind: 'feedback', label: 'Waiting for your input' });
                  if (inner.request_id) {
                    setPendingToolApproval({
                      request_id: inner.request_id,
                      tool_id: inner.tool_id,
                      input: inner.input,
                      reason: inner.reason,
                    });
                  }
                  break;
                }
                case 'question_asked':
                  if (inner.request_id) {
                    addPendingQuestion({
                      request_id: inner.request_id,
                      text: inner.text ?? '',
                      choices: inner.choices ?? [],
                      allow_freeform: inner.allow_freeform !== false,
                      multi_select: inner.multi_select === true,
                      agent_id: event.agent_id,
                      agent_name: inner.agent_id,
                      session_id: currentSessionId,
                      message: inner.message,
                    });
                  }
                  break;
              }
              break;
            }
            case 'agent_status_changed':
            case 'agent_task_assigned':
              if (!isStreaming()) beginStreamingState();
              pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
              break;
            case 'agent_completed':
            case 'all_complete':
              syncChatStateAfterStream(currentSessionId);
              break;
          }
        },
      );

      if (disposed) {
        botStageUnlisten?.();
        botStageUnlisten = null;
      }
    })();

    onCleanup(() => {
      disposed = true;
      botStageUnlisten?.();
      botStageUnlisten = null;
      if (refreshDebounce) clearTimeout(refreshDebounce);
    });
  });

  createEffect(() => {
    const currentSessionId = selectedSessionId();
    const _epoch = daemonEpoch(); // re-subscribe on daemon reconnect
    let streamUnlisten: UnlistenFn | null = null;
    let doneUnlisten: UnlistenFn | null = null;
    let errorUnlisten: UnlistenFn | null = null;
    let disposed = false;

    const cleanupListeners = () => {
      streamUnlisten?.();
      doneUnlisten?.();
      errorUnlisten?.();
      streamUnlisten = null;
      doneUnlisten = null;
      errorUnlisten = null;
    };

    const setupStreamListeners = async () => {
      streamUnlisten = await listen<{ session_id: string; event: any }>('chat:event', (e) => {
        const { session_id: sid, event } = e.payload;
        if (sid !== currentSessionId) return;

        if (event.Token) {
          // If we receive tokens but are not streaming (e.g. after a Done from
          // a prior run, when an agent-injected follow-up starts), re-enter
          // streaming mode automatically.
          if (!isStreaming()) {
            beginStreamingState();
          }
          completeActivity('inference');
          setStreamingContent(prev => prev + event.Token.delta);
        } else if (event.Done) {
          syncChatStateAfterStream(sid);
        } else if (event.Error) {
          console.warn('[chat:event] Error event from daemon:', event.Error);
          clearStreamingState();
          void syncChatState(sid);
        } else if (event.AgentSessionMessage) {
          // An agent injected a message — sync state to display the
          // notification.  Don't enter streaming mode here: if a follow-up
          // worker was spawned the session will emit ModelLoading / Token
          // events which enter streaming mode on their own.  For buffered
          // workflow signals no worker is spawned, so entering streaming
          // would leave the UI stuck in "Thinking…" with no Done event.
          void syncChatState(sid);
        } else if (event.ToolCallStart) {
          const tool_id = event.ToolCallStart.tool_id ?? 'unknown';
          const isSkill = tool_id === 'core.activate_skill';
          const skillName = isSkill ? (tryParseJson(event.ToolCallStart.input)?.name ?? '') : '';
          const actId = `tool:${tool_id}:${Date.now()}`;
          const label = isSkill ? `Activating skill ${skillName}` : `Running ${tool_id}`;
          pushActivity({
            id: actId,
            kind: isSkill ? 'skill' : 'tool',
            label,
            detail: isSkill ? undefined : truncate(event.ToolCallStart.input, 80),
          });
          recordToolCallStart(actId, tool_id, label, event.ToolCallStart.input);
        } else if (event.ToolCallResult) {
          const tool_id = event.ToolCallResult.tool_id ?? 'unknown';
          const match = activities().filter(a => a.id.startsWith(`tool:${tool_id}:`) && !a.done).pop();
          if (match) completeActivity(match.id, event.ToolCallResult.is_error);
          recordToolCallResult(tool_id, event.ToolCallResult.output, event.ToolCallResult.is_error);
          pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
        } else if (event.ModelLoading) {
          pushActivity({
            id: 'model-load',
            kind: 'model',
            label: event.ModelLoading.model ?? 'model',
          });
        } else if (event.ModelDone) {
          completeActivity('model-load');
        } else if (event.UserInteractionRequired) {
          completeActivity('inference');
          pushActivity({ id: 'feedback', kind: 'feedback', label: 'Waiting for your input' });
          const { request_id, kind } = event.UserInteractionRequired;
          if (kind.type === 'tool_approval') {
            setPendingToolApproval({
              request_id: request_id,
              tool_id: kind.tool_id,
              input: kind.input,
              reason: kind.reason,
            });
          } else if (kind.type === 'question') {
            addPendingQuestion({
              request_id: request_id,
              session_id: sid,
              text: kind.text,
              choices: kind.choices || [],
              allow_freeform: kind.allow_freeform !== false,
              multi_select: kind.multi_select === true,
            });
            // Also sync to pick up the question message in the session snapshot
            void syncChatState(sid);
          }
        } else if (event.type === 'agent_output' && event.event) {
          // Supervisor events from workflow sub-agents arrive on the chat
          // SSE stream as SessionEvent::Supervisor (internally tagged with
          // "type" field) rather than the externally-tagged LoopEvent shape.
          const inner = event.event;
          if (inner.type === 'model_call_started') {
            if (!isStreaming()) beginStreamingState();
            pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
          } else if (inner.type === 'model_call_completed') {
            completeActivity('inference');
          } else if (inner.type === 'tool_call_started') {
            const tool_id = inner.tool_id ?? 'unknown';
            const actId = `tool:${tool_id}:${Date.now()}`;
            pushActivity({ id: actId, kind: 'tool', label: `Running ${tool_id}`, detail: truncate(JSON.stringify(inner.input ?? ''), 80) });
            recordToolCallStart(actId, tool_id, `Running ${tool_id}`, JSON.stringify(inner.input ?? ''));
          } else if (inner.type === 'tool_call_completed') {
            const tool_id = inner.tool_id ?? 'unknown';
            const match = activities().filter(a => a.id.startsWith(`tool:${tool_id}:`) && !a.done).pop();
            if (match) completeActivity(match.id, inner.is_error);
            recordToolCallResult(tool_id, JSON.stringify(inner.output ?? ''), inner.is_error ?? false);
            pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
          } else if (inner.type === 'user_interaction_required') {
            completeActivity('inference');
            pushActivity({ id: 'feedback', kind: 'feedback', label: 'Waiting for your input' });
            if (inner.request_id) {
              setPendingToolApproval({
                request_id: inner.request_id,
                tool_id: inner.tool_id,
                input: inner.input,
                reason: inner.reason,
              });
            }
          } else if (inner.type === 'question_asked') {
            completeActivity('inference');
            pushActivity({ id: 'feedback', kind: 'feedback', label: 'Waiting for your input' });
            if (inner.request_id) {
              addPendingQuestion({
                request_id: inner.request_id,
                session_id: sid,
                text: inner.text ?? '',
                choices: inner.choices ?? [],
                allow_freeform: inner.allow_freeform !== false,
                multi_select: inner.multi_select === true,
                agent_id: event.agent_id,
                agent_name: inner.agent_id,
              });
              void syncChatState(sid);
            }
          } else if (inner.type === 'completed' || inner.type === 'failed') {
            syncChatStateAfterStream(sid);
          }
        } else if (event.type === 'agent_status_changed' || event.type === 'agent_task_assigned') {
          if (!isStreaming()) beginStreamingState();
          pushActivity({ id: 'inference', kind: 'inference', label: 'Thinking...' });
        } else if (event.type === 'agent_completed' || event.type === 'all_complete') {
          syncChatStateAfterStream(sid);
        }
      });

      doneUnlisten = await listen<{ session_id: string }>('chat:done', (e) => {
        if (e.payload.session_id === currentSessionId) {
          syncChatStateAfterStream(e.payload.session_id);
        }
      });

      errorUnlisten = await listen<{ session_id: string; error: string; kind?: string }>('chat:error', (e) => {
        if (e.payload.session_id === currentSessionId) {
          clearStreamingState();
          const kind = e.payload.kind;
          if (kind === 'transport' || kind === 'stream') {
            // Transient transport/stream errors (decode failures, disconnects,
            // daemon restarts) resolve on the next poll — log instead of
            // showing a scary banner.
            console.warn(`[chat:error] transient ${kind} error:`, e.payload.error);
          } else {
            setErrorMessage(e.payload.error);
          }
        }
      });

      if (disposed) {
        cleanupListeners();
      }
    };

    void setupStreamListeners();

    onCleanup(() => {
      disposed = true;
      cleanupListeners();
    });
  });

  onMount(() => {
    // Pre-initialize the shiki Web Worker so WASM compilation happens
    // before the user opens their first file in the workspace explorer.
    warmUpHighlighter();

    void (async () => {
      try {
        await refreshAll();
        if (!daemonStatus()) {
          try {
            await invoke('daemon_start');
            await refreshAll();
          } catch { /* daemon start failed — user can try manually */ }
        }
        await checkFirstRun();
      } catch (error) {
        const msg = String(error);
        if (!isTauriInternalError(error)) {
          if (msg.includes('401')) {
            // Token mismatch — daemon may have just restarted with a new
            // token.  The blocking helpers already invalidated the cache,
            // so wait briefly and retry with a fresh keyring read.
            await new Promise((r) => setTimeout(r, 1000));
            try {
              await refreshAll();
            } catch (retryErr) {
              if (!isTauriInternalError(retryErr)) {
                setErrorMessage(String(retryErr));
              }
            }
          } else {
            setErrorMessage(msg);
          }
        }
      } finally {
        setInitializing(false);
        // Start the unified interaction store polling
        interactionStore.startPolling();
      }
    })();

    // ── Adaptive polling: slow down when idle, pause when hidden ──
    let mainPollBusy = false;
    let lastActivity = Date.now();
    let pollTimer: number | undefined;

    const markActivity = () => { lastActivity = Date.now(); };
    document.addEventListener('pointerdown', markActivity);
    document.addEventListener('keydown', markActivity);
    document.addEventListener('wheel', markActivity);

    const pollTick = () => {
      if (!mainPollBusy && !isStreaming() && !workflowStore.showDesigner() && !document.hidden) {
        mainPollBusy = true;
        void (async () => {
          try {
            const nextStatus = await refreshStatus();
            if (nextStatus) {
              // Config-driven state (model router, tools, skills) is refreshed
              // via push notifications from config:changed events.  The poll
              // only keeps chat state and MCP notifications current.
              await Promise.all([
                syncChatState(selectedSessionId()),
                loadMcpServers(),
                loadMcpNotifications(),
              ]);
            } else {
              setModelRouter(null);
              clearChatState();
            }
          } catch (error) {
            // Transient poll errors (e.g. decode failures, daemon restarts) are
            // expected when running idle — log and retry on next tick instead of
            // showing a scary banner to the user.
            console.warn('[poll] background refresh error:', error);
          } finally {
            mainPollBusy = false;
          }
        })();
      }
      schedulePoll();
    };

    const schedulePoll = () => {
      const idleMs = Date.now() - lastActivity;
      const delay = idleMs > 120_000 ? 30_000 : idleMs > 30_000 ? 15_000 : 5_000;
      pollTimer = window.setTimeout(pollTick, delay);
    };

    const handleVisibility = () => {
      if (!document.hidden) {
        lastActivity = Date.now();
        if (pollTimer === undefined) schedulePoll();
      } else {
        if (pollTimer !== undefined) { window.clearTimeout(pollTimer); pollTimer = undefined; }
      }
    };
    document.addEventListener('visibilitychange', handleVisibility);

    schedulePoll();

    onCleanup(() => {
      if (pollTimer !== undefined) window.clearTimeout(pollTimer);
      interactionStore.stopPolling();
      stopDownloadPolling();
      document.removeEventListener('pointerdown', markActivity);
      document.removeEventListener('keydown', markActivity);
      document.removeEventListener('wheel', markActivity);
      document.removeEventListener('visibilitychange', handleVisibility);
    });

    // Keyboard shortcut: Ctrl+Shift+F to toggle Flight Deck
    const handleKeydown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key === 'F') {
        e.preventDefault();
        setFlightDeckOpen((v) => !v);
      }
    };
    window.addEventListener('keydown', handleKeydown);
    onCleanup(() => window.removeEventListener('keydown', handleKeydown));

    // Global external-link handler: open http(s) links in the user's default browser
    // instead of navigating within the Tauri webview.
    const handleExternalLink = (e: MouseEvent) => {
      const anchor = (e.target as HTMLElement).closest?.('a');
      if (!anchor) return;
      const href = anchor.getAttribute('href');
      if (!href) return;
      if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('//')) {
        e.preventDefault();
        e.stopPropagation();
        void openExternal(href.startsWith('//') ? `https:${href}` : href);
      }
    };
    document.addEventListener('click', handleExternalLink, true);
    onCleanup(() => document.removeEventListener('click', handleExternalLink, true));

    // AFK heartbeat: send a heartbeat on any user interaction, throttled to every 30s.
    let lastHeartbeat = 0;
    const sendHeartbeat = () => {
      const now = Date.now();
      if (now - lastHeartbeat < 30_000) return;
      lastHeartbeat = now;
      invoke<{ status: string }>('status_heartbeat')
        .then((r) => setUserStatus(r.status))
        .catch(() => {}); // ignore if daemon is offline
    };
    window.addEventListener('mousemove', sendHeartbeat);
    window.addEventListener('keydown', sendHeartbeat);
    window.addEventListener('mousedown', sendHeartbeat);
    onCleanup(() => {
      window.removeEventListener('mousemove', sendHeartbeat);
      window.removeEventListener('keydown', sendHeartbeat);
      window.removeEventListener('mousedown', sendHeartbeat);
    });

    // Fetch initial status
    invoke<{ status: string }>('get_user_status')
      .then((r) => setUserStatus(r.status))
      .catch(() => {});

    // ── MCP push events: refetch servers, notifications & tools on any mcp:event ──
    let mcpUnlisten: UnlistenFn | undefined;
    let mcpDebounce: ReturnType<typeof setTimeout> | undefined;
    const mcpListenPromise = listen('mcp:event', () => {
      if (mcpDebounce) clearTimeout(mcpDebounce);
      mcpDebounce = setTimeout(() => {
        loadMcpServers();
        loadMcpNotifications();
        loadTools();
      }, 300);
    }).then((fn) => { mcpUnlisten = fn; return fn; });
    onCleanup(() => {
      if (mcpDebounce) clearTimeout(mcpDebounce);
      mcpListenPromise.then(fn => fn());
    });

    // ── Config push events: refetch model router, tools & skills on config:changed ──
    let cfgUnlisten: UnlistenFn | undefined;
    let cfgDebounce: ReturnType<typeof setTimeout> | undefined;
    const configChangedPromise = listen<string>('config:changed', () => {
      if (cfgDebounce) clearTimeout(cfgDebounce);
      cfgDebounce = setTimeout(() => {
        void Promise.allSettled([
          loadModelRouter(),
          loadTools(),
          loadInstalledSkills(),
          loadMcpServers(),
        ]).then((results) => {
          for (const r of results) {
            if (r.status === 'rejected') {
              console.warn('[config:changed] refresh error:', r.reason);
            }
          }
        });
      }, 300);
    }).then((fn) => { cfgUnlisten = fn; return fn; });
    onCleanup(() => {
      if (cfgDebounce) clearTimeout(cfgDebounce);
      configChangedPromise.then(fn => fn());
    });

    // ── Auto-update: listen for backend-triggered check requests ──
    let updateUnlisten: UnlistenFn | undefined;
    const updateCheckPromise = listen<string>('update:check', async (event) => {
      const source = event.payload;
      const isManual = source === 'manual';
      const requestId = ++updateCheckRequestId;

      if (isManual) {
        setPendingUpdate(null);
        setUpdateCheckError(null);
        setUpdateCheckState('checking');
        setShowUpdateDialog(true);
      }

      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        if (requestId !== updateCheckRequestId) return; // stale request
        const update = await check();
        if (requestId !== updateCheckRequestId) return; // stale request
        if (update) {
          setPendingUpdate(update);
          setUpdateCheckState('update-available');
          setShowUpdateDialog(true);
        } else if (isManual) {
          setUpdateCheckState('up-to-date');
        }
      } catch (e: any) {
        if (requestId !== updateCheckRequestId) return; // stale request
        if (isManual) {
          const msg = e?.message ?? String(e);
          // Distinguish plugin-not-available from network/server errors
          if (msg.includes('not found') || msg.includes('not implemented') || msg.includes('plugin')) {
            setUpdateCheckState('unavailable');
          } else {
            setUpdateCheckError(msg);
            setUpdateCheckState('error');
          }
        }
        // Automatic checks still fail silently.
      }
    }).then((fn) => { updateUnlisten = fn; return fn; });
    onCleanup(() => { updateCheckPromise.then(fn => fn()); });
  });

  // ── Setup Wizard (first-run bootstrap) ──────────────────────────
  const markSetupComplete = async () => {
    try {
      const cfg: HiveMindConfigData = await invoke('config_get');
      const updated = { ...cfg, setup_completed: true };
      await invoke('config_save', { config: updated });
    } catch { /* best-effort */ }
    setShowSetupWizard(false);
    await refreshAll();
  };


  return (
    <main class="app-shell">
      <Show when={initializing()}>
        <div class="initializing-overlay">
          <div class="initializing-content">
            <span class="spinner" />
            <span>Starting up…</span>
          </div>
        </div>
      </Show>

      <Show when={showSetupWizard()}>
        <SetupWizard
          localModels={localModels}
          startDownloadPolling={startDownloadPolling}
          loadLocalModels={loadLocalModels}
          context={context}
          onComplete={markSetupComplete}
        />
      </Show>

      <ErrorBanner errorMessage={errorMessage} setErrorMessage={setErrorMessage} />

      {/* ── Flight Deck global toggle (top-right corner) ────────────── */}
      <button
        class="flight-deck-toggle flight-deck-global-toggle"
        data-testid="flight-deck-toggle"
        onClick={() => setFlightDeckOpen(true)}
        title="Flight Deck (Ctrl+Shift+F)"
      >
        <Rocket size={16} />
        <Show when={flightDeckNeedsAttention()}>
          <span class="flight-deck-toggle-badge" />
        </Show>
      </button>

      {/* ── User status toggle (next to Flight Deck) ────────────── */}
      {(() => {
        const [statusOpen, setStatusOpen] = createSignal(false);
        const [popupStyle, setPopupStyle] = createSignal('');
        let btnRef!: HTMLButtonElement;
        const statusOptions = [
          { value: 'active', colorClass: 'bg-emerald-400', label: 'Active' },
          { value: 'idle', colorClass: 'bg-yellow-300', label: 'Idle' },
          { value: 'away', colorClass: 'bg-orange-400', label: 'Away' },
          { value: 'do_not_disturb', colorClass: 'bg-red-400', label: 'Do Not Disturb' },
        ];
        const dot = () => {
          const s = userStatus();
          return statusOptions.find(o => o.value === s) ?? { colorClass: 'bg-muted-foreground', label: 'Unknown' };
        };

        const closePopover = () => setStatusOpen(false);

        // Dismiss on Escape key
        const handleKeyDown = (e: KeyboardEvent) => {
          if (e.key === 'Escape' && statusOpen()) {
            closePopover();
          }
        };
        createEffect(() => {
          if (statusOpen()) {
            document.addEventListener('keydown', handleKeyDown);
          } else {
            document.removeEventListener('keydown', handleKeyDown);
          }
        });
        onCleanup(() => document.removeEventListener('keydown', handleKeyDown));

        return <>
          <button
            ref={btnRef}
            class="flight-deck-toggle status-global-toggle"
            data-testid="status-toggle"
            title={`Status: ${dot().label}`}
            aria-label={`Status: ${dot().label}`}
            onClick={(e: MouseEvent) => {
              e.stopPropagation();
              if (!statusOpen()) {
                const rect = btnRef.getBoundingClientRect();
                const popupHeight = statusOptions.length * 32 + 8;
                let top = rect.bottom + 6;
                if (top + popupHeight > window.innerHeight - 4) top = rect.top - popupHeight - 6;
                setPopupStyle(`position:fixed;top:${top}px;right:${window.innerWidth - rect.right}px;background:hsl(var(--card));border:1px solid hsl(var(--border));border-radius:8px;padding:4px;min-width:160px;z-index:10000;box-shadow:0 4px 12px hsl(0 0% 0% / 0.3);`);
              }
              setStatusOpen(!statusOpen());
            }}
          >
            <span class={`inline-block w-2.5 h-2.5 rounded-full ${dot().colorClass}`}></span>
          </button>
          <Portal>
            <Show when={statusOpen()}>
              <div
                style="position:fixed;top:0;left:0;right:0;bottom:0;z-index:9999;"
                onClick={closePopover}
                onPointerDown={closePopover}
              />
              <div style={popupStyle()} onClick={(e: MouseEvent) => e.stopPropagation()}>
                <For each={statusOptions}>
                  {(opt) => (
                    <button
                      style={`display:flex;align-items:center;gap:8px;width:100%;padding:6px 10px;background:${userStatus() === opt.value ? 'hsl(var(--muted))' : 'transparent'};border:none;color:hsl(var(--foreground));cursor:pointer;border-radius:4px;font-size:13px;`}
                      onClick={() => { handleSetUserStatus(opt.value); closePopover(); }}
                    >
                      <span class={`inline-block w-2 h-2 rounded-full ${opt.colorClass}`}></span>
                      {opt.label}
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </Portal>
        </>;
      })()}

      <SidebarProvider
        open={sidebarOpen()}
        onOpenChange={setSidebarOpen}
        class="h-[calc(100vh-4rem)]"
      >
        <UiSidebar collapsible="offcanvas">
          <Sidebar
            sessions={displayedSessions}
            selectedSessionId={selectedSessionId}
            daemonOnline={daemonOnline}
            busyAction={busyAction}
            inspectorOpen={inspectorOpen}
            setInspectorOpen={setInspectorOpen}
            showNewSessionDialog={showNewSessionDialog}
            setShowNewSessionDialog={setShowNewSessionDialog}
            createSession={createSession}
            personas={personas}
            selectSession={(id) => { setActiveScreen('session'); return selectSession(id); }}
            deleteSession={deleteSession}
            renameSession={renameSession}
            onOpenSettings={() => { void loadEditConfig(); void loadToolDefinitions(); }}
            onReorderSessions={reorderSessions}
            activeScreen={activeScreen}
            setActiveScreen={setActiveScreen}
            botStore={botStore}
            workflowStore={workflowStore}
            interactionStore={interactionStore}
            onSelectBot={(bot_id) => {
              void selectBotSession(bot_id);
            }}
          />
        </UiSidebar>
        <SidebarInset>

        {/* Global sidebar reopen button — visible on all screens when collapsed */}
        <Show when={!sidebarOpen()}>
          <button
            class="sidebar-reopen-btn"
            onClick={() => setSidebarOpen(true)}
            title="Expand sidebar"
            aria-label="Expand sidebar"
          >
            ☰
          </button>
        </Show>

        <Switch>
        <Match when={activeScreen() === 'bots' && !botStore.selectedBotId()}>
          <section style="position:relative;overflow:hidden;flex:1;display:flex;flex-direction:column;">
            <BotsPage
              availableTools={toolIdList}
              modelRouter={modelRouter}
              personas={personas}
              toolDefinitions={tools()}
              onBotQuestion={(q) => addPendingQuestion({
                request_id: q.request_id,
                text: q.text,
                choices: q.choices,
                allow_freeform: q.allow_freeform,
                multi_select: q.multi_select,
                agent_id: q.agent_id,
                is_bot: true,
              })}
              onBotQuestionAnswered={markQuestionAnswered}
            />
          </section>
        </Match>

        <Match when={activeScreen() === 'scheduler'}>
          <section style="position:relative;overflow:hidden;flex:1;display:flex;flex-direction:column;">
            <SchedulerPage
              daemon_url={() => context()?.daemon_url}
              personas={personas()}
              tools={tools()}
              eventTopics={eventTopics()}
              channels={channels()}
              workflowDefinitions={workflowStore.definitions().map(d => ({ name: d.name, version: d.version, description: d.description }))}
              fetchParsedWorkflow={(name) => workflowStore.getDefinitionParsed(name)}
            />
          </section>
        </Match>

        <Match when={activeScreen() === 'workflows'}>
          {(_) => (
            <ErrorBoundary fallback={(err, reset) => (
              <div class="flex flex-col items-center justify-center flex-1 gap-4 p-8 text-foreground">
                <h3 class="m-0 flex items-center gap-1.5 text-destructive"><TriangleAlert size={16} /> Workflows Error</h3>
                <pre class="max-w-[600px] overflow-auto bg-background p-3 rounded-lg text-[0.85em] text-yellow-400">{String(err)}</pre>
                <button class="primary" onClick={reset}>Retry</button>
              </div>
            )}>
              <section style="position:relative;overflow:hidden;flex:1;display:flex;flex-direction:column;">
                <Show when={workflowStore.sidebarSelectedInstanceId() !== null} fallback={
                  <WorkflowsPage
                    store={workflowStore}
                    interactionStore={interactionStore}
                    toolDefinitions={tools()}
                    personas={personas()}
                    channels={channels()}
                    eventTopics={eventTopics()}
                    onApprovalClick={(approval) => {
                      setExternalApproval(approval);
                      setFlightDeckOpen(true);
                    }}
                    onExportToKit={(workflowName) => {
                      agentKitStore.toggleWorkflow(workflowName);
                      setActiveScreen('agent-kits');
                    }}
                  />
                }>
                  <WorkflowDetailPanel
                    store={workflowStore}
                    interactionStore={interactionStore}
                    instanceId={workflowStore.sidebarSelectedInstanceId()!}
                    onApprovalClick={(approval) => {
                      setExternalApproval(approval);
                      setFlightDeckOpen(true);
                    }}
                  />
                </Show>
              </section>
            </ErrorBoundary>
          )}
        </Match>

        <Match when={activeScreen() === 'settings'}>
          <section style="position:relative;overflow:clip;flex:1;display:flex;flex-direction:column;min-height:0;">
            <SettingsModal
              cfg={editConfig}
              setEditConfig={setEditConfig}
              configDirty={configDirty}
              saveConfig={saveConfig}
              loadEditConfig={loadEditConfig}
              configLoadError={configLoadError}
              configSaveMsg={configSaveMsg}
              editingProviderIdx={editingProviderIdx}
              setEditingProviderIdx={setEditingProviderIdx}
              onClose={() => setActiveScreen('session')}
              context={context}
              daemonOnline={daemonOnline}
              daemonStatus={daemonStatus}
              busyAction={busyAction}
              settingsTab={settingsTab}
              setSettingsTab={setSettingsTab}
              updateDaemon={updateDaemon}
              updateApi={updateApi}
              updateOverridePolicy={updateOverridePolicy}
              updatePromptInjection={updatePromptInjection}
              updateLocalModels={updateLocalModels}
              updateCompaction={updateCompaction}
              updateAfk={updateAfk}
              addProvider={addProvider}
              removeProvider={removeProvider}
              moveProvider={moveProvider}
              updateProvider={updateProvider}
              addModelToProvider={addModelToProvider}
              removeModelFromProvider={removeModelFromProvider}
              localModels={localModels}
              localModelView={localModelView}
              setLocalModelView={setLocalModelView}
              storageBytes={storageBytes}
              expandedModel={expandedModel}
              setExpandedModel={setExpandedModel}
              loadLocalModels={loadLocalModels}
              loadHardwareInfo={loadHardwareInfo}
              updateModelParamsDebounced={updateModelParamsDebounced}
              removeModel={removeModel}
              hardwareInfo={hardwareInfo}
              resourceUsage={resourceUsage}
              hubSearchResults={hubSearchResults}
              hubSearchQuery={hubSearchQuery}
              setHubSearchQuery={setHubSearchQuery}
              hubSearchLoading={hubSearchLoading}
              hubSearchError={hubSearchError}
              searchHubModels={searchHubModels}
              installTargetRepo={installTargetRepo}
              setInstallTargetRepo={setInstallTargetRepo}
              installRepoFiles={installRepoFiles}
              installableItems={installableItems}
              installFilesLoading={installFilesLoading}
              installInProgress={installInProgress}
              openInstallDialog={openInstallDialog}
              installModelFile={installModelFile}
              inferRuntime={inferRuntime}
              activeDownloads={activeDownloads}
              setActiveDownloads={setActiveDownloads}
              startDownloadPolling={startDownloadPolling}
              toolDefinitions={toolDefinitions}
              connectors={channels}
              loadPersonas={loadPersonas}
              onConnectorsChanged={loadChannels}
              isNoTokenError={isNoTokenError}
              isLicenseError={isLicenseError}
              extractRepoFromError={extractRepoFromError}
              openExternal={openExternal}
              scrollToHfToken={scrollToHfToken}
              availableModels={availableModels()}
              onExportPersonaToKit={(persona_id) => {
                agentKitStore.togglePersona(persona_id);
                setActiveScreen('agent-kits');
              }}
            />
          </section>
        </Match>

        <Match when={activeScreen() === 'agent-kits'}>
          <section style="position:relative;overflow:hidden;flex:1;display:flex;flex-direction:column;">
            <AgentKitsPage store={agentKitStore} />
          </section>
        </Match>

        <Match when={activeScreen() === 'session' || (activeScreen() === 'bots' && botStore.selectedBotId())}>
        <section class="relative flex h-full flex-col overflow-hidden rounded-xl border border-border bg-card text-card-foreground">
          <header class="flex items-center gap-3 px-4 py-2">
            <span class="truncate text-base font-semibold text-foreground">
              {sessionEntityType() === 'bot'
                ? (() => {
                    const bot = botStore.bots().find(b => b.config.id === selectedSessionId());
                    return bot ? `${bot.config.avatar || '🤖'} ${bot.config.friendly_name}` : session()?.title ?? 'Bot';
                  })()
                : session()?.title ?? 'Chat'}
            </span>
            <Show when={session()?.workspace_path}>
              {(workspace_path) => (
                <span
                  class="truncate rounded-full bg-muted px-2 py-0.5 text-[0.7em] text-muted-foreground cursor-pointer hover:bg-muted/80"
                  title={`${workspace_path()} — click to open in file manager`}
                  onClick={() => void openExternal(workspace_path())}
                >
                  <FolderOpen size={14} /> {session()?.workspace_linked ? <><Link size={14} />{' '}</> : ''}{workspaceNameFromPath(workspace_path())}
                </span>
              )}
            </Show>
            <div class="ml-auto flex items-center gap-2">
              <select
                class="composer-inline-select"
                value={chatFontSize()}
                onChange={(e) => setChatFontSize(e.currentTarget.value as any)}
                title="Chat text size"
              >
                <option value="small">A⁻</option>
                <option value="medium">A</option>
                <option value="large">A⁺</option>
              </select>
              <span class={`pill ${statusClass(activeSessionState())}`}>
                {activeSessionState() ?? 'Offline'}
              </span>
              <span
                class="pill neutral"
                style="cursor: pointer; position: relative; white-space: nowrap;"
                onClick={() => setShowQueuedPopup((v) => !v)}
                title="Click to view queued messages"
              >
                {queueCount()} queued
              </span>
              <Show when={showQueuedPopup()}>
                <div
                  class="modal-overlay"
                  style="background: transparent;"
                  onClick={() => setShowQueuedPopup(false)}
                >
                  <div
                    class="queued-popup"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <header style="display:flex;align-items:center;gap:8px;margin-bottom:12px;">
                      <ClipboardList size={14} />
                      <h3 style="margin:0;font-size:0.95rem;">Queued Messages</h3>
                      <button
                        style="margin-left:auto;background:none;border:none;color:hsl(var(--muted-foreground));cursor:pointer;font-size:1rem;"
                        onClick={() => setShowQueuedPopup(false)}
                      >✕</button>
                    </header>
                    <Show
                      when={queuedMessages().length > 0}
                      fallback={<p style="color:hsl(var(--muted-foreground));font-size:0.85rem;margin:0;">No messages in queue.</p>}
                    >
                      <ul class="queued-list">
                        <For each={queuedMessages()}>
                          {(msg, idx) => (
                            <li class="queued-item">
                              <div class="queued-item-header">
                                <span class={`queued-badge ${msg.status}`}>{msg.status}</span>
                                <span class="queued-role">{msg.role}</span>
                                <Show when={msg.data_class}>
                                  <span class="queued-data-class">{msg.data_class}</span>
                                </Show>
                              </div>
                              <p class="queued-content">{msg.content.length > 200 ? msg.content.slice(0, 200) + '…' : msg.content}</p>
                              <div class="queued-item-meta">
                                <span>#{idx() + 1}</span>
                                <span>{new Date(msg.created_at_ms).toLocaleTimeString()}</span>
                              </div>
                            </li>
                          )}
                        </For>
                      </ul>
                    </Show>
                  </div>
                </div>
              </Show>
            </div>
          </header>

          <Show when={session()} fallback={<p class="empty-copy">Select a session to begin.</p>}>
            {(activeSession) => (
              <>
                <div style="display:flex;flex:1;overflow:hidden;position:relative;">
                  <nav class="session-tabs">
                    <button
                      class={`session-tab ${activeTab() === 'chat' ? 'active' : ''}`}
                      data-testid="tab-chat"
                      aria-label="Chat tab"
                      title={activeSession().modality === 'spatial' ? 'Canvas' : 'Chat'}
                      onClick={() => setActiveTab('chat')}
                    >
                      <span class="session-tab-icon">{activeSession().modality === 'spatial' ? <Compass size={18} /> : <MessageSquare size={18} />}</span>
                      <span class="session-tab-label">{activeSession().modality === 'spatial' ? 'Canvas' : 'Chat'}</span>
                    </button>
                    <button
                      class={`session-tab ${activeTab() === 'workspace' ? 'active' : ''}`}
                      data-testid="tab-workspace"
                      aria-label="Workspace tab"
                      title="Workspace"
                      onClick={() => setActiveTab('workspace')}
                    >
                      <span class="session-tab-icon"><FolderOpen size={18} /></span>
                      <span class="session-tab-label">Workspace</span>
                    </button>
                    <button
                      class={`session-tab ${activeTab() === 'stage' ? 'active' : ''}`}
                      data-testid="tab-stage"
                      aria-label="Sub-Agents tab"
                      title="Sub-Agents"
                      onClick={() => setActiveTab('stage')}
                    >
                      <span class="session-tab-icon"><Layers size={18} /></span>
                      <span class="session-tab-label">Sub-Agents</span>
                    </button>
                    <Show when={sessionEntityType() !== 'bot'}>
                      <button
                        class={`session-tab ${activeTab() === 'workflows' ? 'active' : ''}`}
                        data-testid="tab-workflows"
                        aria-label="Workflows tab"
                        title="Workflows"
                        onClick={() => setActiveTab('workflows')}
                      >
                        <span class="session-tab-icon"><GitBranch size={18} /></span>
                        <span class="session-tab-label">Workflows</span>
                      </button>
                    </Show>
                    <button
                      class={`session-tab ${activeTab() === 'events' ? 'active' : ''}`}
                      data-testid="tab-events"
                      aria-label="Events tab"
                      title="Events"
                      onClick={() => setActiveTab('events')}
                    >
                      <span class="session-tab-icon"><Activity size={18} /></span>
                      <span class="session-tab-label">Events</span>
                    </button>
                    <Show when={sessionEntityType() !== 'bot'}>
                      <button
                        class={`session-tab ${activeTab() === 'processes' ? 'active' : ''}`}
                        data-testid="tab-processes"
                        aria-label="Processes tab"
                        title="Processes"
                        onClick={() => setActiveTab('processes')}
                      >
                        <span class="session-tab-icon"><Terminal size={18} /></span>
                        <span class="session-tab-label">Processes</span>
                      </button>
                    </Show>
                    <button
                      class={`session-tab ${activeTab() === 'mcp' ? 'active' : ''}`}
                      data-testid="tab-mcp"
                      aria-label="MCP tab"
                      title="MCP Servers"
                      onClick={() => setActiveTab('mcp')}
                    >
                      <span class="session-tab-icon"><Plug size={18} /></span>
                      <span class="session-tab-label">MCP</span>
                    </button>
                    <Show when={sessionEntityType() === 'bot'}>
                      <button
                        class={`session-tab ${activeTab() === 'config' ? 'active' : ''}`}
                        data-testid="tab-config"
                        aria-label="Config tab"
                        title="Bot Config"
                        onClick={() => setActiveTab('config')}
                      >
                        <span class="session-tab-icon"><Settings size={18} /></span>
                        <span class="session-tab-label">Config</span>
                      </button>
                    </Show>
                  </nav>
                <div class="flex flex-1 flex-col overflow-hidden" style={`margin-left:42px;${busyAction() === 'select-session' ? 'opacity:0.5;pointer-events:none;transition:opacity 150ms' : 'transition:opacity 150ms'}`}>
                  <div class="flex flex-1 flex-col gap-4 overflow-y-auto py-4" style={activeTab() === 'chat' ? '' : 'display:none'}>
                <ChatView
                  session={session}
                  activeSessionState={activeSessionState}
                  queueCount={queueCount}
                  showDiagnostics={showDiagnostics}
                  setShowDiagnostics={setShowDiagnostics}
                  showMemoriesDialog={showMemoriesDialog}
                  setShowMemoriesDialog={setShowMemoriesDialog}
                  chatFontPx={chatFontPx}
                  expandedMsgIds={expandedMsgIds}
                  setExpandedMsgIds={setExpandedMsgIds}
                  streamingContent={streamingContent}
                  isStreaming={isStreaming}
                  activities={activities}
                  toolCallHistory={toolCallHistory}
                  pendingReview={pendingReview}
                  busyAction={busyAction}
                  daemonOnline={daemonOnline}
                  draft={draft}
                  setDraft={setDraft}
                  pendingAttachments={pendingAttachments}
                  setPendingAttachments={setPendingAttachments}
                  personas={personas}
                  selectedAgentId={selectedAgentId}
                  setSelectedAgentId={setSelectedAgentId}
                  tools={tools}
                  installedSkills={installedSkills}
                  selectedDataClass={selectedDataClass}
                  setSelectedDataClass={setSelectedDataClass}
                  excludedTools={excludedTools}
                  setExcludedTools={setExcludedTools}
                  excludedSkills={excludedSkills}
                  setExcludedSkills={setExcludedSkills}
                  selectedSessionId={selectedSessionId}
                  sendMessage={sendMessage}
                  uploadFiles={uploadFiles}
                  interrupt={interrupt}
                  resume={resume}
                  loadSessionPerms={loadSessionPerms}
                  setShowSessionPermsDialog={setShowSessionPermsDialog}
                  setShowSettings={(open: boolean) => { if (open) setActiveScreen('settings'); }}
                  setSettingsTab={setSettingsTab}
                  loadEditConfig={loadEditConfig}
                  loadToolDefinitions={loadToolDefinitions}
                  entityType={sessionEntityType()}
                  readOnly={sessionEntityType() === 'bot' && (() => {
                    const bot = botStore.bots().find(b => b.config.id === selectedSessionId());
                    return bot?.status === 'done' || bot?.status === 'error';
                  })()}
                  workspaceFiles={workspaceFiles}
                  allQuestions={allQuestions}
                  onQuestionAnswered={(request_id, answerText) => {
                    markQuestionAnswered(request_id, answerText);
                    // Refresh session to pick up the answered message
                    const sid = selectedSessionId();
                    if (sid) void syncChatState(sid);
                  }}
                  onSpatialSendMessage={(content, position) => {
                    setDraft(content);
                    if (position) {
                      setPendingCanvasPosition([position.x, position.y]);
                    } else {
                      setPendingCanvasPosition(null);
                    }
                    void sendMessage();
                  }}
                  chatWorkflowDefinitions={workflowStore.chatDefinitions}
                  chatWorkflows={chatWorkflows}
                  activeChatWorkflows={activeChatWorkflows}
                  terminalChatWorkflows={terminalChatWorkflows}
                  onLaunchChatWorkflow={launchChatWorkflow}
                  onPauseChatWorkflow={pauseChatWorkflow}
                  onResumeChatWorkflow={resumeChatWorkflow}
                  onKillChatWorkflow={killChatWorkflow}
                  onRespondWorkflowGate={respondWorkflowGate}
                  fetchParsedWorkflow={(name) => workflowStore.getDefinitionParsed(name)}
                />
              </div>

              <div class="flex flex-1 flex-col gap-4 overflow-y-auto py-4" style={activeTab() === 'workspace' ? '' : 'display:none'}>
                <ErrorBoundary fallback={(err, reset) => (
                  <div class="flex flex-col items-center justify-center flex-1 gap-4 p-8 text-foreground">
                    <h3 class="m-0 flex items-center gap-1.5 text-destructive"><TriangleAlert size={16} /> Workspace Error</h3>
                    <pre class="max-w-[600px] overflow-auto bg-background p-3 rounded-lg text-[0.85em] text-yellow-400">{String(err)}</pre>
                    <button class="primary" onClick={reset}>Retry</button>
                  </div>
                )}>
                <WorkspaceView
                  session={session}
                  availableModels={availableModels}
                  workspace={{
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
                    contextMenu,
                    setContextMenu,
                    auditTarget,
                    setAuditTarget,
                    auditModel,
                    setAuditModel,
                    auditRunning,
                    auditResult,
                    setAuditResult,
                    auditStatus,
                    newFolderParent,
                    setNewFolderParent,
                    newFolderName,
                    setNewFolderName,
                    newFileParent,
                    setNewFileParent,
                    newFileName,
                    setNewFileName,
                    dragOverPath,
                    setDragOverPath,
                    classificationColors,
                    auditIcon,
                    formatFileSize: (bytes: number) => {
                      if (bytes < 1024) return `${bytes} B`;
                      if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
                      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
                    },
                    loadWorkspaceFiles,
                    loadDirectoryChildren,
                    openWorkspaceFile,
                    saveWorkspaceFile,
                    runFileAudit,
                    setClassification,
                    clearClassification,
                    reindexFile,
                    indexStatus,
                    subscribeIndexStatus: async () => {},
                    cleanupIndexStatus: () => {},
                    createNewFolder,
                    createNewFile,
                    deleteEntry,
                    moveEntry,
                    copyToClipboard,
                    pasteFromClipboard,
                    pasteProgress,
                    pasteConflict,
                    cancelPaste,
                    resolveConflict,
                    resetFileState: () => {
                      setSelectedEntryPath(null);
                      setSelectedFilePath(null);
                      setFileContent(null);
                      setFileEditorContent('');
                      setWorkspaceFiles([]);
                    },
                  }}
                />
                </ErrorBoundary>
              </div>

              <div class="flex flex-1 flex-col gap-4 overflow-y-auto py-4" style={activeTab() === 'stage' ? '' : 'display:none'}>
                <Show
                  when={activeTab() === 'stage' && selectedSessionId()}
                  keyed
                  fallback={<p class="empty-copy">Select a session to view active agents.</p>}
                >
                  {(session_id) => <AgentStage
                    session_id={session_id}
                    modelRouter={modelRouter}
                    pendingQuestions={() => pendingQuestions().filter((q) => q.session_id === session_id)}
                    answeredQuestions={answeredQuestions}
                    onQuestionAnswered={markQuestionAnswered}
                    onAgentQuestion={(agent_id, request_id, text, choices, allow_freeform, message, multi_select) => {
                      addPendingQuestion({ request_id, text, choices, allow_freeform, multi_select, agent_id: agent_id, message, session_id });
                    }}
                    personas={personas}
                  />}
                </Show>
              </div>

              <div class="flex flex-1 flex-col overflow-hidden" style={activeTab() === 'workflows' ? '' : 'display:none'}>
                <SessionWorkflows
                  sessionId={selectedSessionId}
                  chatWorkflows={chatWorkflows}
                  activeChatWorkflows={activeChatWorkflows}
                  terminalChatWorkflows={terminalChatWorkflows}
                  onPause={pauseChatWorkflow}
                  onResume={resumeChatWorkflow}
                  onKill={killChatWorkflow}
                  onRespondWorkflowGate={respondWorkflowGate}
                  pendingQuestions={() => pendingQuestions().filter((q) => q.session_id === selectedSessionId())}
                  onQuestionAnswered={(request_id, answerText) => {
                    markQuestionAnswered(request_id, answerText);
                    const sid = selectedSessionId();
                    if (sid) void syncChatState(sid);
                  }}
                />
              </div>

              <div class="flex flex-1 flex-col overflow-hidden" style={activeTab() === 'events' ? '' : 'display:none'}>
                <SessionEvents
                  session_id={selectedSessionId}
                  daemonOnline={daemonOnline}
                  entityType={sessionEntityType}
                />
              </div>

              <div class="flex flex-1 flex-col overflow-hidden" style={activeTab() === 'processes' ? '' : 'display:none'}>
                <SessionProcesses
                  session_id={selectedSessionId}
                  daemonOnline={daemonOnline}
                />
              </div>

              <Show when={sessionEntityType() === 'bot' && selectedSessionId()}>
                <div class="flex flex-1 flex-col overflow-hidden" style={activeTab() === 'config' ? '' : 'display:none'}>
                  <BotConfigPanel
                    bot_id={selectedSessionId()!}
                    botStore={botStore}
                    personas={personas}
                  />
                </div>
              </Show>

              <Show when={selectedSessionId()}>
                <div class="flex flex-1 flex-col overflow-hidden" style={activeTab() === 'mcp' ? '' : 'display:none'}>
                  <SessionMcpPanel
                    session_id={selectedSessionId()!}
                    daemon_url={context()?.daemon_url ?? ''}
                  />
                </div>
              </Show>

            </div>
            </div>
          </>
        )}
      </Show>
    </section>
        </Match>
        </Switch>

        </SidebarInset>
      </SidebarProvider>

      {/* ── Inspector modal ─────────────────────────────────────────── */}
      <Show when={inspectorOpen()}>
        <InspectorModal
          daemonStatus={daemonStatus}
          tools={tools}
          sessionMemory={sessionMemory}
          memoryQuery={memoryQuery}
          setMemoryQuery={setMemoryQuery}
          daemonOnline={daemonOnline}
          busyAction={busyAction}
          runAction={runAction}
          runMemorySearch={runMemorySearch}
          riskScans={riskScans}
          kgStats={kgStats}
          kgView={kgView}
          setKgView={setKgView}
          loadKgNodes={loadKgNodes}
          loadKgStats={loadKgStats}
          kgNodeTypeFilter={kgNodeTypeFilter}
          setKgNodeTypeFilter={setKgNodeTypeFilter}
          kgNodes={kgNodes}
          loadKgNode={loadKgNode}
          kgDeleteNode={kgDeleteNode}
          kgSearchQuery={kgSearchQuery}
          setKgSearchQuery={setKgSearchQuery}
          runKgSearch={runKgSearch}
          kgSearchResults={kgSearchResults}
          kgNewNodeType={kgNewNodeType}
          setKgNewNodeType={setKgNewNodeType}
          kgNewNodeName={kgNewNodeName}
          setKgNewNodeName={setKgNewNodeName}
          kgNewNodeContent={kgNewNodeContent}
          setKgNewNodeContent={setKgNewNodeContent}
          kgNewNodeDataClass={kgNewNodeDataClass}
          setKgNewNodeDataClass={setKgNewNodeDataClass}
          kgCreateNode={kgCreateNode}
          kgSelectedNode={kgSelectedNode}
          setKgSelectedNode={setKgSelectedNode}
          kgNewEdgeTargetId={kgNewEdgeTargetId}
          setKgNewEdgeTargetId={setKgNewEdgeTargetId}
          kgNewEdgeType={kgNewEdgeType}
          setKgNewEdgeType={setKgNewEdgeType}
          kgCreateEdge={kgCreateEdge}
          kgDeleteEdge={kgDeleteEdge}
          memoryResults={memoryResults}
          onClose={() => setInspectorOpen(false)}
        />
      </Show>

      {/* ── Flight Deck ─────────────────────────────────────────────── */}
      <FlightDeck
        open={flightDeckOpen}
        onClose={() => setFlightDeckOpen(false)}
        daemonOnline={daemonOnline}
        modelRouter={modelRouter}
        pendingQuestions={pendingQuestions}
        answeredQuestions={answeredQuestions}
        onQuestionAnswered={markQuestionAnswered}
        externalApproval={externalApproval}
        onExternalApprovalHandled={() => setExternalApproval(null)}
        externalQuestion={externalQuestion}
        onExternalQuestionHandled={() => setExternalQuestion(null)}
        personas={personas}
        daemon_url={() => context()?.daemon_url}
        kgStats={kgStats}
        loadKgStats={loadKgStats}
      />

      {/* ── Tool Approval dialog ─────────────────────────────────────── */}
      <Show when={pendingToolApproval()}>
        <ToolApprovalDialog
          approval={pendingToolApproval}
          selectedSessionId={selectedSessionId}
          onDismiss={() => { setPendingToolApproval(null); completeActivity('feedback'); }}
        />
      </Show>

      {/* ── Agent approval toasts (always visible) ─────────────── */}
      <AgentApprovalToast />

      {/* ── Session permissions dialog ───────────────────────────── */}
      <SessionPermsDialog
        open={showSessionPermsDialog()}
        sessionPerms={sessionPerms}
        setSessionPerms={setSessionPerms}
        saveSessionPerms={saveSessionPerms}
        onClose={() => setShowSessionPermsDialog(false)}
        toolDefinitions={tools()}
      />

      {/* ── Auto-update dialog ────────────────────────────────────── */}
      <UpdateDialog
        open={showUpdateDialog()}
        update={pendingUpdate()}
        checkState={updateCheckState()}
        checkError={updateCheckError()}
        onClose={() => { setShowUpdateDialog(false); setUpdateCheckState('idle'); }}
        onRetry={() => {
          const _ = import('@tauri-apps/api/event').then(({ emit }) => emit('update:check', 'manual'));
        }}
      />

    </main>
  );
};

export default App;

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
