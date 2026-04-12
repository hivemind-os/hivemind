import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Accessor } from 'solid-js';
import type { AgentSpec, AgentStatus, InstalledSkill, ModelRouterSnapshot, BotSummary, TelemetrySnapshot, TokenUsage, Persona, PromptTemplate } from '../types';
import type { PendingQuestion } from './InlineQuestion';
import { dismissAgentApproval, pendingApprovalToasts, type PendingApproval } from './AgentApprovalToast';
import { logError, logInfo } from './ActivityLog';
import { highlightYaml } from './YamlHighlight';
import { isTauriInternalError } from '../utils';
import {
  answerQuestion as routeAnswerQuestion,
  respondToApproval as routeRespondToApproval,
  type PendingInteraction,
} from '~/lib/interactionRouting';
import PermissionRulesEditor, { type PermissionRule } from './PermissionRulesEditor';
import BotDetailPanel from './BotDetailPanel';
import EventLogList, { type SupervisorEvent } from './EventLogList';
import PromptParameterDialog from './shared/PromptParameterDialog';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { renderMarkdown } from '~/utils';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Rocket, ClipboardList, MessageSquare, Wrench, XCircle, CheckCircle2, BarChart3, ArrowLeftRight, RefreshCw, Play, Pause, Settings, Trash2, Send, Upload, Brain, Lock, HelpCircle, CheckCircle, ShieldAlert, BookOpen, X, Maximize2, RotateCcw, ZoomIn, ZoomOut } from 'lucide-solid';

interface AgentStageProps {
  session_id: string;
  mode?: 'session' | 'service';
  modelRouter: Accessor<ModelRouterSnapshot | null>;
  pendingQuestions: Accessor<PendingQuestion[]>;
  answeredQuestions: Accessor<Map<string, string>>;
  onQuestionAnswered: (request_id: string, answerText: string) => void;
  onAgentQuestion?: (agent_id: string, request_id: string, text: string, choices: string[], allow_freeform: boolean, message?: string, multi_select?: boolean) => void;
  personas?: Accessor<Persona[]>;
}

type AgentEntry = {
  agent_id: string;
  spec: AgentSpec;
  status: AgentStatus;
  last_error?: string | null;
  active_model?: string | null;
  tools: string[];
  parent_id?: string | null;
};

interface ReasoningEvent {
  type: string;
  model?: string;
  prompt_preview?: string;
  content?: string;
  token_count?: number;
  tool_id?: string;
  input?: any;
  output?: any;
  is_error?: boolean;
  request_id?: string;
  reason?: string;
  result?: string;
  error?: string;
  step_id?: string;
  description?: string;
  // QuestionAsked fields
  agent_id?: string;
  text?: string;
  choices?: string[];
  allow_freeform?: boolean;
  tool_result_counts?: Record<string, number>;
}

// ── Helpers ──────────────────────────────────────────────────────────

const statusColors: Record<AgentStatus, string> = {
  spawning: 'hsl(40 90% 84%)',
  active: 'hsl(160 60% 76%)',
  waiting: 'hsl(var(--primary))',
  paused: 'hsl(var(--muted-foreground))',
  blocked: 'hsl(24 93% 75%)',
  done: 'hsl(160 60% 76%)',
  error: 'hsl(var(--destructive))',
};

const totalTokens = (usage?: TokenUsage | null) =>
  (usage?.input_tokens ?? 0) + (usage?.output_tokens ?? 0);

const formatTokens = (tokens: number) => {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${tokens}`;
};

const titleCaseStatus = (status: AgentStatus) =>
  status.charAt(0).toUpperCase() + status.slice(1);

const truncate = (str: string, len: number) =>
  str.length > len ? str.slice(0, len) + '…' : str;

const shortModel = (model: string | null | undefined) => {
  if (!model) return '';
  const parts = model.split(':');
  return parts.length > 1 ? parts[1] : model;
};

// ── Event rendering ──────────────────────────────────────────────────

const renderEvent = (ev: SupervisorEvent) => {
  switch (ev.type) {
    case 'agent_spawned':
      return <span class="ev-spawn"><Rocket size={14} /> Agent spawned — {ev.spec?.friendly_name || ev.spec?.name}</span>;
    case 'agent_task_assigned':
      return <span class="ev-status"><ClipboardList size={14} /> Task assigned</span>;
    case 'agent_status_changed':
      return <span class="ev-status">◉ Status → {ev.status}</span>;
    case 'message_routed':
      return <span class="ev-msg"><Send size={14} /> {ev.msg_type} from {ev.from}</span>;
    case 'agent_completed':
      return <span class="ev-done"><CheckCircle size={14} /> Completed: {truncate(ev.result ?? '', 200)}</span>;
    case 'agent_output': {
      const re = ev.event;
      if (!re) return <span><Upload size={14} /> Output</span>;
      switch (re.type) {
        case 'model_call_started':
          return <span class="ev-model"><Brain size={14} /> Model call → {re.model}</span>;
        case 'model_call_completed':
          return (
            <span class="ev-model-done">
              <MessageSquare size={14} /> Response ({re.token_count} tokens): {truncate(re.content ?? '', 300)}
            </span>
          );
        case 'tool_call_started':
          return (
            <span class="ev-tool">
              <Wrench size={14} /> Tool: {re.tool_id}({typeof re.input === 'object' ? truncate(JSON.stringify(re.input), 120) : ''})
            </span>
          );
        case 'tool_call_completed':
          return (
            <span class={`ev-tool-done ${re.is_error ? 'ev-error' : ''}`}>
              {re.is_error ? <XCircle size={14} /> : <CheckCircle2 size={14} />} {re.tool_id} →{' '}
              {truncate(typeof re.output === 'object' ? JSON.stringify(re.output) : String(re.output ?? ''), 200)}
            </span>
          );
        case 'user_interaction_required':
          return (
            <span class="ev-approval"><Lock size={14} /> Approval needed: {re.tool_id} — {re.reason}</span>
          );
        case 'question_asked':
          return (
            <span class="ev-approval"><HelpCircle size={14} /> Question: {re.text}</span>
          );
        case 'completed':
          return <span class="ev-done"><CheckCircle size={14} /> {truncate(re.result ?? '', 200)}</span>;
        case 'failed':
          return <span class="ev-error"><XCircle size={14} /> {re.error}</span>;
        case 'step_started':
          return <span class="ev-step"><ClipboardList size={14} /> {re.description}</span>;
        default:
          return <span><Upload size={14} /> {re.type}</span>;
      }
    }
    default:
      return <span>{ev.type}</span>;
  }
};

// ── Edge drawing ─────────────────────────────────────────────────────

type NodePosition = { x: number; y: number };

/** Build a flat list of SVG path `d` attributes for edges between nodes. */
function computeEdges(
  agents: AgentEntry[],
  positions: Map<string, NodePosition>,
  serviceMode?: boolean,
): Array<{ from: string; to: string; d: string; active: boolean }> {
  if (serviceMode) return []; // Bots are flat — no edges
  const edges: Array<{ from: string; to: string; d: string; active: boolean }> = [];

  for (const agent of agents) {
    const parentKey = agent.parent_id ?? 'session';
    const parentPos = positions.get(parentKey);
    const childPos = positions.get(agent.agent_id);
    if (!parentPos || !childPos) continue;

    const isSessionParent = parentKey === 'session';
    const parentBottom = isSessionParent ? SESSION_H - 20 : NODE_H;
    const x1 = parentPos.x;
    const y1 = parentPos.y + parentBottom;
    const x2 = childPos.x;
    const y2 = childPos.y;

    let d: string;
    if (Math.abs(x1 - x2) < 1) {
      // Straight vertical line
      d = `M ${x1} ${y1} L ${x2} ${y2}`;
    } else {
      const midY = (y1 + y2) / 2;
      d = `M ${x1} ${y1} C ${x1} ${midY}, ${x2} ${midY}, ${x2} ${y2}`;
    }
    edges.push({
      from: parentKey,
      to: agent.agent_id,
      d,
      active: agent.status === 'active',
    });
  }
  return edges;
}

// ── Graph layout (simple tree) ───────────────────────────────────────

const NODE_W = 260;
const NODE_H = 110;
const SESSION_H = 44; // session node is shorter
const GAP_X = 32;
const GAP_Y = 80;

function layoutGraph(
  agents: AgentEntry[],
  serviceMode?: boolean,
): { positions: Map<string, NodePosition>; width: number; height: number } {
  const positions = new Map<string, NodePosition>();
  if (agents.length === 0) {
    if (!serviceMode) positions.set('session', { x: 0, y: 30 });
    return { positions, width: NODE_W, height: NODE_H + 40 };
  }

  // Service mode: flat grid layout — no tree hierarchy
  if (serviceMode) {
    const cols = Math.max(1, Math.min(agents.length, 3));
    const rows = Math.ceil(agents.length / cols);
    const rowWidth = cols * (NODE_W + GAP_X) - GAP_X;
    const startX = -rowWidth / 2;
    for (let i = 0; i < agents.length; i++) {
      const col = i % cols;
      const row = Math.floor(i / cols);
      positions.set(agents[i].agent_id, {
        x: startX + col * (NODE_W + GAP_X) + NODE_W / 2,
        y: row * (NODE_H + GAP_Y) + 30,
      });
    }
    return {
      positions,
      width: rowWidth + 40,
      height: rows * (NODE_H + GAP_Y) + 40,
    };
  }

  // Build children map
  const childrenOf = new Map<string, string[]>();
  childrenOf.set('session', []);
  for (const a of agents) {
    const parent = a.parent_id ?? 'session';
    if (!childrenOf.has(parent)) childrenOf.set(parent, []);
    childrenOf.get(parent)!.push(a.agent_id);
    if (!childrenOf.has(a.agent_id)) childrenOf.set(a.agent_id, []);
  }

  // BFS to determine depths
  type LevelEntry = { id: string; depth: number };
  const queue: LevelEntry[] = [{ id: 'session', depth: 0 }];
  const visited = new Set<string>();
  const levels: LevelEntry[] = [];

  while (queue.length > 0) {
    const entry = queue.shift()!;
    if (visited.has(entry.id)) continue;
    visited.add(entry.id);
    levels.push(entry);
    for (const child of childrenOf.get(entry.id) ?? []) {
      queue.push({ id: child, depth: entry.depth + 1 });
    }
  }

  // Group by depth
  const byDepth = new Map<number, string[]>();
  for (const { id, depth } of levels) {
    if (!byDepth.has(depth)) byDepth.set(depth, []);
    byDepth.get(depth)!.push(id);
  }

  let maxWidth = 0;
  for (const [depth, ids] of byDepth) {
    const rowWidth = ids.length * (NODE_W + GAP_X) - GAP_X;
    maxWidth = Math.max(maxWidth, rowWidth);
    const startX = -rowWidth / 2;
    for (let i = 0; i < ids.length; i++) {
      positions.set(ids[i], {
        x: startX + i * (NODE_W + GAP_X) + NODE_W / 2,
        y: depth * (NODE_H + GAP_Y) + 30,
      });
    }
  }

  const maxDepth = Math.max(...Array.from(byDepth.keys()));
  return {
    positions,
    width: maxWidth + 40,
    height: (maxDepth + 1) * (NODE_H + GAP_Y) + 40,
  };
}

// ── Bot → AgentEntry converter ───────────────────────────────────────

function botToAgentEntry(s: BotSummary): AgentEntry {
  return {
    agent_id: s.config.id,
    spec: {
      id: s.config.id,
      name: s.config.friendly_name,
      friendly_name: s.config.friendly_name,
      description: s.config.description,
      role: s.config.role as AgentSpec['role'],
      model: s.config.model || null,
      system_prompt: s.config.system_prompt,
      allowed_tools: s.config.allowed_tools,
      avatar: s.config.avatar || null,
      color: s.config.color || null,
      data_class: s.config.data_class,
      keep_alive: true,
    },
    status: s.status as AgentStatus,
    last_error: s.last_error || null,
    active_model: s.active_model || null,
    tools: s.tools,
    parent_id: null,
  };
}

// ── Main Component ───────────────────────────────────────────────────

const AgentStage = (props: AgentStageProps) => {
  const isService = () => props.mode === 'service';
  const [agents, setAgents] = createSignal<AgentEntry[]>([]);
  const [telemetry, setTelemetry] = createSignal<TelemetrySnapshot | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [sseFailed, setSseFailed] = createSignal(false);
  const [connectionTimeout, setConnectionTimeout] = createSignal(false);
  let connectionTimeoutTimer: ReturnType<typeof setTimeout> | undefined;
  let pollFallbackTimer: ReturnType<typeof setTimeout> | undefined;
  const edgeTimers = new Set<ReturnType<typeof setTimeout>>();
  const [busyAgentId, setBusyAgentId] = createSignal<string | null>(null);
  const [expandedAgentId, setExpandedAgentId] = createSignal<string | null>(null);
  const [agentEvents, setAgentEvents] = createSignal<SupervisorEvent[]>([]);
  const [agentEventsTotal, setAgentEventsTotal] = createSignal(0);
  const [eventsLoading, setEventsLoading] = createSignal(false);
  const PAGE_SIZE = 50;

  // Edge key → timestamp of last message_routed event
  const [edgeTimestamps, setEdgeTimestamps] = createSignal<Map<string, number>>(new Map());
  // Active edges (green glow, 10s) and recent edges (grey, 2min)
  const activeEdges = createMemo(() => {
    const now = Date.now();
    const ts = edgeTimestamps();
    const active = new Set<string>();
    for (const [key, t] of ts) {
      if (now - t < 10_000) active.add(key);
    }
    return active;
  });
  const recentEdges = createMemo(() => {
    const now = Date.now();
    const ts = edgeTimestamps();
    const recent = new Set<string>();
    for (const [key, t] of ts) {
      if (now - t >= 10_000 && now - t < 120_000) recent.add(key);
    }
    return recent;
  });
  let canvasRef: HTMLDivElement | undefined;

  // ── Viewport state (zoom & pan) ────────────────────────────────────
  const [centerX, setCenterX] = createSignal(0);
  const [centerY, setCenterY] = createSignal(0);
  const [stageZoom, setStageZoom] = createSignal(1);
  const [isPanning, setIsPanning] = createSignal(false);
  const [panStart, setPanStart] = createSignal({ x: 0, y: 0 });
  const [canvasSize, setCanvasSize] = createSignal({ width: 800, height: 400 });
  let hasAutoFit = false;

  const canvasToScreen = (cx: number, cy: number) => {
    const size = canvasSize();
    const z = stageZoom();
    return {
      x: (cx - centerX()) * z + size.width / 2,
      y: (cy - centerY()) * z + size.height / 2,
    };
  };

  const screenToCanvas = (sx: number, sy: number) => {
    const size = canvasSize();
    const z = stageZoom();
    return {
      x: (sx - size.width / 2) / z + centerX(),
      y: (sy - size.height / 2) / z + centerY(),
    };
  };

  const fitToView = () => {
    const l = layout();
    if (l.positions.size === 0) return;
    const size = canvasSize();
    if (size.width === 0 || size.height === 0) return;
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const [, pos] of l.positions) {
      minX = Math.min(minX, pos.x - NODE_W / 2);
      maxX = Math.max(maxX, pos.x + NODE_W / 2);
      minY = Math.min(minY, pos.y);
      maxY = Math.max(maxY, pos.y + NODE_H);
    }
    const contentW = maxX - minX + 80;
    const contentH = maxY - minY + 80;
    const scaleX = size.width / contentW;
    const scaleY = size.height / contentH;
    const newZoom = Math.max(0.1, Math.min(scaleX, scaleY, 1.5));
    setCenterX((minX + maxX) / 2);
    setCenterY((minY + maxY) / 2);
    setStageZoom(newZoom);
  };

  const resetZoom = () => {
    setCenterX(0);
    setCenterY(0);
    setStageZoom(1);
  };

  // ── Mouse event handlers (zoom & pan) ──────────────────────────────
  const handleStageWheel = (e: WheelEvent) => {
    e.preventDefault();
    const delta = e.deltaY > 0 ? 0.92 : 1.08;
    const newZoom = Math.max(0.1, Math.min(3, stageZoom() * delta));
    const rect = canvasRef?.getBoundingClientRect();
    if (!rect) return;
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const canvasBefore = screenToCanvas(mx, my);
    setStageZoom(newZoom);
    const canvasAfter = screenToCanvas(mx, my);
    setCenterX(cx => cx - (canvasAfter.x - canvasBefore.x));
    setCenterY(cy => cy - (canvasAfter.y - canvasBefore.y));
  };

  const handleStageMouseDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    // Don't pan when clicking on interactive elements
    if ((e.target as HTMLElement).closest('.agent-node, .agent-stage-header, .agent-stage-zoom-controls, button, input, textarea, a, [role="dialog"]')) return;
    setIsPanning(true);
    setPanStart({ x: e.clientX, y: e.clientY });
    e.preventDefault();
  };

  const handleStageMouseMove = (e: MouseEvent) => {
    if (!isPanning()) return;
    const z = stageZoom();
    const dx = e.clientX - panStart().x;
    const dy = e.clientY - panStart().y;
    setCenterX(cx => cx - dx / z);
    setCenterY(cy => cy - dy / z);
    setPanStart({ x: e.clientX, y: e.clientY });
  };

  const handleStageMouseUp = () => {
    setIsPanning(false);
  };

  const svgTransform = () => {
    const screen = canvasToScreen(0, 0);
    return `translate(${screen.x}, ${screen.y}) scale(${stageZoom()})`;
  };

  const [recastAgentId, setRecastAgentId] = createSignal<string | null>(null);
  const [configAgent, setConfigAgent] = createSignal<AgentEntry | null>(null);
  const [configModel, setConfigModel] = createSignal<string>('');
  const [configSkills, setConfigSkills] = createSignal<InstalledSkill[]>([]);
  // Per-agent: set of tool IDs enabled for this agent (derived from spec.allowed_tools)
  const [configEnabledTools, setConfigEnabledTools] = createSignal<Set<string>>(new Set());
  const [configPermRules, setConfigPermRules] = createSignal<PermissionRule[]>([]);
  const [confirmKill, setConfirmKill] = createSignal<{ id: string; name: string } | null>(null);
  const [questionDialogQuestion, setQuestionDialogQuestion] = createSignal<PendingQuestion | null>(null);
  const [questionFreeText, setQuestionFreeText] = createSignal('');
  const [questionSending, setQuestionSending] = createSignal(false);
  const [approvalDialogItem, setApprovalDialogItem] = createSignal<PendingApproval | null>(null);
  const [approvalSending, setApprovalSending] = createSignal(false);
  // Prompt template state for "Send Prompt" to active agent
  const [promptAgentId, setPromptAgentId] = createSignal<string | null>(null);
  const [promptPickerFor, setPromptPickerFor] = createSignal<string | null>(null);
  const [activePromptTemplate, setActivePromptTemplate] = createSignal<{ agent_id: string; persona: Persona; template: PromptTemplate } | null>(null);

  /** Return prompt templates scoped to a specific persona ID. */
  const promptsForPersona = (persona_id: string | null | undefined): { persona: Persona; template: PromptTemplate }[] => {
    if (!persona_id) return [];
    const personas = props.personas?.() ?? [];
    const persona = personas.find((p) => p.id === persona_id);
    if (!persona) return [];
    return (persona.prompts ?? []).map((t) => ({ persona, template: t }));
  };
  const telemetryByAgent = createMemo(() => new Map(telemetry()?.per_agent ?? []));

  // Build available models list for the model switcher
  const availableModels = createMemo(() => {
    const router = props.modelRouter();
    if (!router) return [] as { id: string; label: string }[];
    const models: { id: string; label: string }[] = [];
    for (const p of router.providers) {
      if (!p.available) continue;
      for (const m of p.models) {
        models.push({
          id: `${p.id}:${m}`,
          label: `${m} (${p.name || p.id})`,
        });
      }
    }
    return models;
  });

  // Compute graph layout
  const [layoutOverride, setLayoutOverride] = createSignal<Map<string, NodePosition> | null>(null);
  const layout = createMemo(() => {
    const override = layoutOverride();
    const base = layoutGraph(agents(), isService());
    if (!override) return base;
    // Merge override positions into base (preserving height/width from override agent bounds)
    const merged = new Map(base.positions);
    let minX = Infinity, maxX = -Infinity, maxY = 0;
    for (const [id, pos] of override) {
      merged.set(id, pos);
      minX = Math.min(minX, pos.x - NODE_W / 2);
      maxX = Math.max(maxX, pos.x + NODE_W / 2);
      maxY = Math.max(maxY, pos.y + NODE_H);
    }
    return {
      positions: merged,
      width: Math.max(base.width, maxX - minX + 40),
      height: Math.max(base.height, maxY + 40),
    };
  });
  // Clear rearranged layout when agent list changes
  createEffect(() => { agents().length; setLayoutOverride(null); });
  const graphEdges = createMemo(() =>
    computeEdges(agents(), layout().positions, isService()),
  );

  // Dynamic communication edges (for bots: message_routed creates temporary lines)
  // "active" = green glow (first 10s), "recent" = grey (10s–2min)
  const dynamicEdges = createMemo(() => {
    const active = activeEdges();
    const recent = recentEdges();
    if (active.size === 0 && recent.size === 0) return [];
    const positions = layout().positions;
    const edges: Array<{ key: string; d: string; tier: 'active' | 'recent' }> = [];
    const seen = new Set<string>();
    const addEdge = (edgeKey: string, tier: 'active' | 'recent') => {
      if (seen.has(edgeKey)) return;
      seen.add(edgeKey);
      const [fromId, toId] = edgeKey.split('->');
      if (!fromId || !toId) return;
      const fromPos = positions.get(fromId);
      const toPos = positions.get(toId);
      if (!fromPos || !toPos) return;
      const x1 = fromPos.x;
      const y1 = fromPos.y + NODE_H / 2;
      const x2 = toPos.x;
      const y2 = toPos.y + NODE_H / 2;
      const midX = (x1 + x2) / 2;
      const d = `M ${x1} ${y1} C ${midX} ${y1}, ${midX} ${y2}, ${x2} ${y2}`;
      edges.push({ key: edgeKey, d, tier });
    };
    for (const edgeKey of active) addEdge(edgeKey, 'active');
    for (const edgeKey of recent) addEdge(edgeKey, 'recent');
    return edges;
  });

  // ── Event-driven telemetry refresh (replaces fixed-interval poll) ──
  let telemetryTimer: ReturnType<typeof setTimeout> | null = null;
  let telemetryBusy = false;
  const TELEMETRY_DEBOUNCE_MS = 2000;

  const fetchTelemetry = async () => {
    if (telemetryBusy) return;
    telemetryBusy = true;
    try {
      const telem = isService()
        ? await invoke<TelemetrySnapshot>('get_bot_telemetry')
        : await invoke<TelemetrySnapshot>('get_agent_telemetry', { session_id: props.session_id });
      setTelemetry(telem);
    } catch { /* ignore */ }
    finally { telemetryBusy = false; }
  };

  const debouncedFetchTelemetry = () => {
    if (telemetryTimer) clearTimeout(telemetryTimer);
    telemetryTimer = setTimeout(() => {
      telemetryTimer = null;
      void fetchTelemetry();
    }, TELEMETRY_DEBOUNCE_MS);
  };

  onCleanup(() => {
    if (telemetryTimer) clearTimeout(telemetryTimer);
    edgeTimers.forEach(t => clearTimeout(t));
    edgeTimers.clear();
  });

  // ── SSE event handling ──────────────────────────────────────────────

  const handleStageEvent = (event: SupervisorEvent) => {
    switch (event.type) {
      case 'agent_spawned': {
        if (event.agent_id && event.spec) {
          setAgents((prev) => {
            if (prev.some((a) => a.agent_id === event.agent_id)) return prev;
            return [
              ...prev,
              {
                agent_id: event.agent_id!,
                spec: event.spec!,
                status: 'spawning' as AgentStatus,
                tools: [],
                parent_id: event.parent_id ?? null,
              },
            ];
          });
        }
        break;
      }
      case 'agent_status_changed': {
        if (event.agent_id && event.status) {
          setAgents((prev) =>
            prev.map((a) =>
              a.agent_id === event.agent_id
                ? {
                    ...a,
                    status: event.status as AgentStatus,
                    last_error: event.status !== 'error' ? null : a.last_error,
                  }
                : a,
            ),
          );
        }
        break;
      }
      case 'agent_output': {
        if (event.agent_id && event.event) {
          if (event.event.type === 'model_call_started' && event.event.model) {
            setAgents((prev) =>
              prev.map((a) =>
                a.agent_id === event.agent_id
                  ? { ...a, active_model: event.event!.model ?? a.active_model }
                  : a,
              ),
            );
          }
          if (event.event.type === 'failed' && event.event.error) {
            setAgents((prev) =>
              prev.map((a) =>
                a.agent_id === event.agent_id
                  ? { ...a, last_error: event.event!.error }
                  : a,
              ),
            );
          }
          if (event.event.type === 'question_asked' && event.event.request_id && props.onAgentQuestion) {
            const ev = event.event as any;
            props.onAgentQuestion(
              event.agent_id!,
              ev.request_id,
              ev.text ?? '',
              ev.choices ?? [],
              ev.allow_freeform !== false,
              ev.message,
              ev.multi_select === true,
            );
          }
        }
        break;
      }
      case 'message_routed': {
        if (event.from && event.to) {
          const edgeKey = `${event.from}->${event.to}`;
          const now = Date.now();
          setEdgeTimestamps((prev) => new Map(prev).set(edgeKey, now));
          // Refresh reactive state when edge transitions from active→recent (10s)
          const t1 = setTimeout(() => {
            edgeTimers.delete(t1);
            setEdgeTimestamps((prev) => new Map(prev));
          }, 10_000);
          edgeTimers.add(t1);
          // Remove edge entirely after 2 minutes
          const t2 = setTimeout(() => {
            edgeTimers.delete(t2);
            setEdgeTimestamps((prev) => {
              const next = new Map(prev);
              // Only remove if this is still the same timestamp (no newer message)
              if (next.get(edgeKey) === now) next.delete(edgeKey);
              return next;
            });
          }, 120_000);
          edgeTimers.add(t2);
        }
        break;
      }
      case 'agent_completed': {
        if (event.agent_id) {
          setAgents((prev) =>
            prev.map((a) =>
              a.agent_id === event.agent_id
                ? { ...a, status: a.status === 'error' ? 'error' : ('done' as AgentStatus) }
                : a,
            ),
          );
        }
        break;
      }
    }

    // Refresh telemetry on events that affect token / tool counts
    if (event.type === 'agent_output' || event.type === 'agent_completed') {
      debouncedFetchTelemetry();
    }
  };

  // ── Actions ─────────────────────────────────────────────────────────

  const respondToApproval = async (agent_id: string, request_id: string, approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => {
    dismissAgentApproval(request_id);
    try {
      await routeRespondToApproval(
        {
          request_id,
          entity_id: `agent/${agent_id}`,
          source_name: agent_id,
          type: 'tool_approval',
          session_id: isService() ? undefined : props.session_id,
          agent_id,
        } as PendingInteraction,
        { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
      );
      const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
      logInfo('approval', `${approved ? 'Approved' : 'Denied'}${scope} tool for agent ${agent_id}`);
    } catch (err: any) {
      logError('approval', `Failed to respond to approval ${request_id}: ${err}`);
    }
  };

  interface PagedEventsResponse { events: SupervisorEvent[]; total: number; }

  const loadAgentEvents = async (agent_id: string, loadEarlier = false) => {
    setEventsLoading(true);
    try {
      const currentEvents = loadEarlier ? agentEvents() : [];
      let offset: number;
      let limit = PAGE_SIZE;

      if (!loadEarlier) {
        // Initial load: probe for total, then fetch most recent page
        const probe = isService()
          ? await invoke<PagedEventsResponse>('get_bot_events', { agent_id, offset: 0, limit: 0 })
          : await invoke<PagedEventsResponse>('get_agent_events', { session_id: props.session_id, agent_id, offset: 0, limit: 0 });
        const total = probe.total;
        setAgentEventsTotal(total);
        offset = Math.max(0, total - PAGE_SIZE);
        limit = PAGE_SIZE;
      } else {
        // Load earlier: prepend older events
        const total = agentEventsTotal();
        const alreadyLoaded = currentEvents.length;
        const remaining = total - alreadyLoaded;
        if (remaining <= 0) return;
        offset = Math.max(0, remaining - PAGE_SIZE);
        limit = remaining - offset;
      }

      const resp = isService()
        ? await invoke<PagedEventsResponse>('get_bot_events', { agent_id, offset, limit })
        : await invoke<PagedEventsResponse>('get_agent_events', { session_id: props.session_id, agent_id, offset, limit });

      setAgentEventsTotal(resp.total);

      if (loadEarlier) {
        setAgentEvents([...resp.events, ...currentEvents]);
      } else {
        setAgentEvents(resp.events);
      }
    } catch {
      if (!loadEarlier) {
        setAgentEvents([]);
        setAgentEventsTotal(0);
      }
    } finally {
      setEventsLoading(false);
    }
  };

  const toggleExpand = (agent_id: string) => {
    if (expandedAgentId() === agent_id) {
      setExpandedAgentId(null);
      setAgentEvents([]);
      setAgentEventsTotal(0);
    } else {
      setExpandedAgentId(agent_id);
      void loadAgentEvents(agent_id);
    }
  };

  const runAgentAction = async (agent_id: string, status: AgentStatus) => {
    setBusyAgentId(agent_id);
    setError(null);
    try {
      if (isService()) {
        if (status === 'paused' || status === 'waiting') {
          await invoke('activate_bot', { agent_id });
        } else {
          await invoke('deactivate_bot', { agent_id });
        }
      } else {
        if (status === 'paused') {
          await invoke('resume_session_agent', { session_id: props.session_id, agent_id });
        } else {
          await invoke('pause_session_agent', { session_id: props.session_id, agent_id });
        }
      }
    } catch (err: any) {
      setError(err?.toString?.() ?? 'Failed to update agent state.');
    } finally {
      setBusyAgentId(null);
    }
  };

  const killAgent = async (agent_id: string, agent_name: string) => {
    setConfirmKill({ id: agent_id, name: agent_name });
  };

  const executeKill = async () => {
    const target = confirmKill();
    if (!target) return;
    setConfirmKill(null);
    const agent_id = target.id;
    setBusyAgentId(agent_id);
    setError(null);
    try {
      if (isService()) {
        await invoke('delete_bot', { agent_id });
      } else {
        await invoke('kill_session_agent', { session_id: props.session_id, agent_id });
      }
      // Remove from local state immediately
      setAgents(prev => prev.filter(a => a.agent_id !== agent_id));
      if (expandedAgentId() === agent_id) {
        setExpandedAgentId(null);
        setAgentEvents([]);
      }
    } catch (err: any) {
      setError(err?.toString?.() ?? 'Failed to remove agent.');
    } finally {
      setBusyAgentId(null);
    }
  };

  const restartAgent = async (agent_id: string, model?: string, allowed_tools?: string[]) => {
    if (isService()) return; // Bots don't support restart
    setBusyAgentId(agent_id);
    setError(null);
    try {
      await invoke('restart_session_agent', {
        session_id: props.session_id,
        agent_id,
        model: model || null,
        allowed_tools: allowed_tools || null,
      });
      setRecastAgentId(null);
      setConfigAgent(null);
      logInfo('agent-stage', `Restarted agent ${agent_id}${model ? ` with model ${model}` : ''}`);
    } catch (err: any) {
      setError(err?.toString?.() ?? 'Failed to restart agent.');
    } finally {
      setBusyAgentId(null);
    }
  };

  const openConfigDialog = (entry: AgentEntry) => {
    setConfigModel(entry.active_model || entry.spec.model || '');
    setConfigAgent(entry);

    // Initialise per-agent enabled tools from the agent's current tool list
    setConfigEnabledTools(new Set(entry.tools));

    // Load installed skills (try per-persona first, fall back to global)
    const role = entry.spec?.role;
    const persona_id = typeof role === 'object' && role !== null && 'custom' in role
      ? (role as { custom: string }).custom
      : typeof role === 'string' ? role : null;
    if (persona_id) {
      invoke<InstalledSkill[]>('skills_list_installed_for_persona', { persona_id })
        .then((skills) => setConfigSkills(skills))
        .catch(() => setConfigSkills([]));
    } else {
      setConfigSkills([]);
    }

    // Load per-agent permission rules
    if (isService()) {
      invoke<{ rules: PermissionRule[] }>('get_bot_permissions', { agent_id: entry.agent_id })
        .then((perms) => setConfigPermRules(perms.rules ?? []))
        .catch(() => setConfigPermRules([]));
    }
  };

  const canToggleAgent = (status: AgentStatus) => !['done', 'error'].includes(status);
  const canKillAgent = (status: AgentStatus) => isService() || !['done', 'error'].includes(status);

  // ── Connection helpers ───────────────────────────────────────────────

  const startConnectionTimer = () => {
    clearTimeout(connectionTimeoutTimer);
    setConnectionTimeout(false);
    connectionTimeoutTimer = setTimeout(() => {
      if (loading()) setConnectionTimeout(true);
    }, 10_000);
  };

  const subscribeAndPoll = async () => {
    try {
      if (isService()) {
        await invoke('bot_subscribe');
      } else {
        await invoke('agent_stage_subscribe', { session_id: props.session_id });
      }

      // Fallback: if no snapshot arrives within 3s, load via polling once
      clearTimeout(pollFallbackTimer);
      pollFallbackTimer = setTimeout(async () => {
        if (loading()) {
          try {
            if (isService()) {
              const [summaries, telem] = await Promise.all([
                invoke<BotSummary[]>('list_bots'),
                invoke<TelemetrySnapshot>('get_bot_telemetry'),
              ]);
              setAgents(Array.isArray(summaries) ? summaries.map(botToAgentEntry) : []);
              setTelemetry(telem);
            } else {
              const [agentRows, telem] = await Promise.all([
                invoke<AgentEntry[]>('list_session_agents', { session_id: props.session_id }),
                invoke<TelemetrySnapshot>('get_agent_telemetry', { session_id: props.session_id }),
              ]);
              setAgents(Array.isArray(agentRows) ? agentRows : []);
              setTelemetry(telem);
            }
          } catch {
            setSseFailed(true);
          }
          setLoading(false);
        }
      }, 3000);
    } catch (err: any) {
      if (!isTauriInternalError(err)) {
        setError(err?.toString?.() ?? 'Failed to subscribe to agent stage.');
      }
      setLoading(false);
    }
  };

  const retryConnection= async () => {
    setConnectionTimeout(false);
    setLoading(true);
    setSseFailed(false);
    setError(null);
    startConnectionTimer();
    await subscribeAndPoll();
  };

  // ── Lifecycle: subscribe to SSE ─────────────────────────────────────

  onMount(async () => {
    let unlisten: UnlistenFn | null = null;
    let unlistenError: UnlistenFn | null = null;

    startConnectionTimer();

    try {
      // Listen for SSE events
      unlisten = await listen<{ session_id: string; event: any }>(
        'stage:event',
        (ev) => {
          if (ev.payload.session_id !== props.session_id) return;
          const data = ev.payload.event;

          // Handle initial snapshot
          if (data.type === 'snapshot') {
            const rawAgents = data.agents ?? [];
            setAgents(isService() ? rawAgents.map(botToAgentEntry) : rawAgents);
            setTelemetry(data.telemetry ?? null);
            setLoading(false);
            return;
          }

          handleStageEvent(data as SupervisorEvent);
        },
      );

      unlistenError = await listen<{ session_id: string; error: string }>(
        'stage:error',
        (ev) => {
          if (ev.payload.session_id !== props.session_id) return;
          setError(ev.payload.error);
        },
      );

      // Start the SSE subscription + fallback polling
      await subscribeAndPoll();

      onCleanup(() => {
        clearTimeout(connectionTimeoutTimer);
        clearTimeout(pollFallbackTimer);
        unlisten?.();
        unlistenError?.();
      });
    } catch (err: any) {
      if (!isTauriInternalError(err)) {
        setError(err?.toString?.() ?? 'Failed to subscribe to agent stage.');
      }
      setLoading(false);
    }
  });

  // Clear connection timeout when loading finishes
  createEffect(() => {
    if (!loading()) {
      clearTimeout(connectionTimeoutTimer);
      setConnectionTimeout(false);
    }
  });

  // Track canvas size reactively for viewport transforms
  createEffect(() => {
    if (loading()) return;
    const el = canvasRef;
    if (!el) return;
    setCanvasSize({ width: el.clientWidth, height: el.clientHeight });
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setCanvasSize({ width: entry.contentRect.width, height: entry.contentRect.height });
      }
    });
    ro.observe(el);
    onCleanup(() => ro.disconnect());
  });

  // Auto-fit view when agents first load
  createEffect(() => {
    const count = agents().length;
    if (count > 0 && !loading() && !hasAutoFit) {
      hasAutoFit = true;
      // Delay to allow layout to settle
      requestAnimationFrame(() => fitToView());
    }
  });

  // Refresh event log when expanded agent changes via SSE updates
  createEffect(() => {
    const eid = expandedAgentId();
    if (!eid) return;
    // Re-fetch events whenever agents signal updates
    agents();
    void loadAgentEvents(eid);
  });

  // ── Rearrange layout (minimize edge crossings) ──────────────────────

  const rearrangeLayout = () => {
    const agentList = agents();
    if (agentList.length < 2) return;

    // Build adjacency weights from communication history
    const weights = new Map<string, Map<string, number>>();
    for (const [edgeKey, _ts] of edgeTimestamps()) {
      const [from, to] = edgeKey.split('->');
      if (!from || !to) continue;
      if (!weights.has(from)) weights.set(from, new Map());
      if (!weights.has(to)) weights.set(to, new Map());
      weights.get(from)!.set(to, (weights.get(from)!.get(to) ?? 0) + 1);
      weights.get(to)!.set(from, (weights.get(to)!.get(from) ?? 0) + 1);
    }

    // Greedy ordering: start with the most-connected agent, then add
    // the agent with the strongest connection to already-placed agents
    const ids = agentList.map((a) => a.agent_id);
    const placed: string[] = [];
    const remaining = new Set(ids);

    // Seed: pick the agent with the most total connections
    let bestSeed = ids[0];
    let bestScore = -1;
    for (const id of ids) {
      const conns = weights.get(id);
      if (conns) {
        let score = 0;
        for (const w of conns.values()) score += w;
        if (score > bestScore) { bestScore = score; bestSeed = id; }
      }
    }
    placed.push(bestSeed);
    remaining.delete(bestSeed);

    while (remaining.size > 0) {
      let bestId = '';
      let bestW = -1;
      for (const id of remaining) {
        let totalW = 0;
        const conns = weights.get(id);
        if (conns) {
          for (const p of placed) {
            totalW += conns.get(p) ?? 0;
          }
        }
        if (totalW > bestW || bestId === '') {
          bestW = totalW;
          bestId = id;
        }
      }
      placed.push(bestId);
      remaining.delete(bestId);
    }

    // Lay out in grid order
    const cols = Math.max(1, Math.min(placed.length, 3));
    const rows = Math.ceil(placed.length / cols);
    const rowWidth = cols * (NODE_W + GAP_X) - GAP_X;
    const startX = -rowWidth / 2;
    const positions = new Map<string, NodePosition>();
    for (let i = 0; i < placed.length; i++) {
      const col = i % cols;
      const row = Math.floor(i / cols);
      positions.set(placed[i], {
        x: startX + col * (NODE_W + GAP_X) + NODE_W / 2,
        y: row * (NODE_H + GAP_Y) + 30,
      });
    }
    setLayoutOverride(positions);
  };

  // ── Rendering ───────────────────────────────────────────────────────

  return (
    <div class="agent-stage">
      {/* Header bar */}
      <div class="agent-stage-header">
        <div class="agent-stage-summary">
          <span><BarChart3 size={14} /> {agents().length} agent{agents().length === 1 ? '' : 's'}</span>
          <span title="Input + Output tokens">{formatTokens(totalTokens(telemetry()?.total))} tokens ({formatTokens(telemetry()?.total?.input_tokens ?? 0)}↑ {formatTokens(telemetry()?.total?.output_tokens ?? 0)}↓)</span>
          <span>{telemetry()?.total?.model_calls ?? 0} LLM calls</span>
          <span>{telemetry()?.total?.tool_calls ?? 0} tool calls</span>
        </div>
        <Show when={agents().length > 1}>
          <button
            class="btn-rearrange"
            onClick={rearrangeLayout}
            title="Rearrange agents to minimize edge crossings"
          >
            <ArrowLeftRight size={14} /> Rearrange
          </button>
        </Show>
      </div>

      <Show when={error()}>
        <div class="banner error">{error()}</div>
      </Show>

      <Show when={sseFailed() && agents().length === 0}>
        <div class="empty-copy" style="text-align:center;">
          <p>Could not connect to agent stage.</p>
          <button class="icon-btn" style="margin-top:0.5rem;" onClick={() => { setSseFailed(false); setLoading(true); location.reload(); }}><RefreshCw size={14} /> Retry</button>
        </div>
      </Show>

      <Show when={!sseFailed() && !loading() && agents().length === 0}>
        <div class="empty-copy" style="text-align:center;">
          <p>{isService() ? 'No bots running. Launch a bot to see it here.' : 'No agents active in this session.'}</p>
        </div>
      </Show>

      <Show when={!loading()} fallback={
        connectionTimeout()
          ? <div class="empty-copy" style="text-align:center;">
              <p>Could not connect to agent stage.</p>
              <button class="icon-btn" style="margin-top:0.5rem;" onClick={retryConnection}><RefreshCw size={14} /> Retry</button>
            </div>
          : <p class="empty-copy">Connecting to agent stage…</p>
      }>
        {/* Graph canvas */}
        <div
          class={`agent-stage-canvas ${isPanning() ? 'panning' : ''}`}
          ref={(el) => { canvasRef = el; }}
          onWheel={handleStageWheel}
          onMouseDown={handleStageMouseDown}
          onMouseMove={handleStageMouseMove}
          onMouseUp={handleStageMouseUp}
          onMouseLeave={handleStageMouseUp}
        >
          {/* Transformed content layer */}
          <svg
            class="agent-stage-edges"
            style={{
              width: '100%',
              height: '100%',
              overflow: 'visible',
            }}
          >
            <g transform={svgTransform()}>
              {/* Arrow marker definition */}
              <defs>
                <marker
                  id="arrow-active"
                  markerWidth="8"
                  markerHeight="6"
                  refX="8"
                  refY="3"
                  orient="auto"
                >
                  <path d="M 0 0 L 8 3 L 0 6 Z" fill="hsl(160 60% 76%)" />
                </marker>
              </defs>
              <For each={graphEdges()}>
                {(edge) => {
                  const isActive = () =>
                    activeEdges().has(`${edge.from}->${edge.to}`) || edge.active;
                  return (
                    <path
                      d={edge.d}
                      fill="none"
                      stroke={isActive() ? 'hsl(160 60% 76%)' : 'hsl(var(--primary))'}
                      stroke-width={isActive() ? 3 : 2}
                      stroke-opacity={isActive() ? 0.9 : 0.45}
                      class={`agent-edge ${isActive() ? 'agent-edge-active' : ''}`}
                    />
                  );
                }}
              </For>
              {/* Dynamic communication edges (agent-to-agent messages) */}
              <For each={dynamicEdges()}>
                {(edge) => (
                  <>
                    <path
                      d={edge.d}
                      fill="none"
                      stroke={edge.tier === 'active' ? 'hsl(160 60% 76%)' : 'hsl(var(--muted-foreground))'}
                      stroke-width={edge.tier === 'active' ? 3 : 1.5}
                      stroke-opacity={edge.tier === 'active' ? 0.85 : 0.45}
                      stroke-dasharray={edge.tier === 'active' ? '6 4' : '4 6'}
                      marker-end={edge.tier === 'active' ? 'url(#arrow-active)' : undefined}
                      class={`agent-edge ${edge.tier === 'active' ? 'agent-edge-active' : ''}`}
                    />
                    {/* Animated dot traveling along active edges */}
                    <Show when={edge.tier === 'active'}>
                      <circle r="4" fill="hsl(160 60% 76%)" opacity="0.9">
                        <animateMotion
                          dur="1.5s"
                          repeatCount="indefinite"
                          path={edge.d}
                        />
                      </circle>
                    </Show>
                  </>
                )}
              </For>
            </g>
          </svg>

          {/* Session root node — only shown in session mode */}
          <Show when={!isService()}>
          <div
            class="agent-node agent-node-session"
            style={{
              left: `${canvasToScreen((layout().positions.get('session')?.x ?? 0), 0).x - (NODE_W * stageZoom()) / 2}px`,
              top: `${canvasToScreen(0, (layout().positions.get('session')?.y ?? 0) - 20).y}px`,
              width: `${NODE_W}px`,
              transform: `scale(${stageZoom()})`,
              'transform-origin': 'top left',
            }}
          >
            <div class="agent-node-avatar session-avatar"><MessageSquare size={16} /></div>
            <div class="agent-node-label">Chat Session</div>
          </div>
          </Show>

          {/* Agent nodes */}
          <For each={agents()}>
            {(entry) => {
              const agent_id = entry.agent_id;
              const pos = () => layout().positions.get(agent_id);
              const usage = () => telemetryByAgent().get(agent_id);
              const color = () => statusColors[entry.status] ?? 'hsl(var(--muted-foreground))';
              const busy = () => busyAgentId() === agent_id;
              const modelDisplay = () => shortModel(entry.active_model || entry.spec.model);
              const agentQuestions = () => props.pendingQuestions().filter((q) => q.agent_id === agent_id);
              const agentApprovals = () => pendingApprovalToasts().filter((a) => a.agent_id === agent_id);
              const personaLabel = () => {
                const pid = entry.spec.persona_id;
                if (!pid) return null;
                const p = (props.personas?.() ?? []).find((pp) => pp.id === pid);
                return p ? p.name : pid.includes('/') ? pid.split('/').pop()! : pid;
              };

              return (
                <Show when={pos()}>
                  <div
                    class={`agent-node agent-node-agent ${entry.status === 'active' ? 'active' : ''}`}
                    style={{
                      left: `${canvasToScreen(pos()!.x, 0).x - (NODE_W * stageZoom()) / 2}px`,
                      top: `${canvasToScreen(0, pos()!.y).y}px`,
                      width: `${NODE_W}px`,
                      'border-color': entry.spec.color ? `${entry.spec.color}44` : undefined,
                      transform: `scale(${stageZoom()})`,
                      'transform-origin': 'top left',
                    }}
                  >
                    {/* Card header */}
                    <div class="agent-card-header" onClick={() => toggleExpand(agent_id)}>
                      <div
                        class="agent-card-avatar"
                        style={{
                          'border-color': entry.spec.color ?? color(),
                          'background-color': `${(entry.spec.color ?? color())}22`,
                        }}
                      >
                        {entry.spec.avatar || '🤖'}
                      </div>

                      <div class="agent-card-name-row">
                        <div class="agent-card-name">{entry.spec.friendly_name || entry.spec.name}</div>
                        <Show when={personaLabel()}>
                          <div class="agent-card-persona" title={entry.spec.persona_id ?? ''}>
                            {personaLabel()}
                          </div>
                        </Show>
                        <div
                          class={`agent-card-status ${entry.status === 'active' ? 'status-pulse' : ''}`}
                          style={{
                            color: color(),
                            'border-color': `${color()}55`,
                            'background-color': `${color()}18`,
                          }}
                        >
                          ● {titleCaseStatus(entry.status)}
                        </div>
                        <Show when={agentQuestions().length > 0}>
                          <div
                            class="agent-card-question-badge"
                            title="Click to answer pending question"
                            onClick={(e) => { e.stopPropagation(); setQuestionDialogQuestion(agentQuestions()[0]); setQuestionFreeText(''); setQuestionSending(false); }}
                          >
                            <HelpCircle size={14} /> {agentQuestions().length}
                          </div>
                        </Show>
                        <Show when={agentApprovals().length > 0}>
                          <div
                            class="agent-card-approval-badge"
                            title="Click to review pending approval"
                            onClick={(e) => { e.stopPropagation(); setApprovalDialogItem(agentApprovals()[0]); setApprovalSending(false); }}
                          >
                            <Lock size={14} /> {agentApprovals().length}
                          </div>
                        </Show>
                      </div>

                      <div class="agent-card-model">
                        <Show when={modelDisplay()} fallback={<span class="agent-card-role">{entry.spec.role as string}</span>}>
                          <span class="agent-model-badge" title={entry.active_model || entry.spec.model || ''}>
                            <Brain size={14} /> {modelDisplay()}
                          </span>
                        </Show>
                      </div>

                      <div class="agent-card-telemetry">
                        <span title="Input ↑ / Output ↓">{formatTokens(usage()?.input_tokens ?? 0)}↑ {formatTokens(usage()?.output_tokens ?? 0)}↓</span>
                        <span>{usage()?.model_calls ?? 0} calls</span>
                        <span>{usage()?.tool_calls ?? 0} tools</span>
                      </div>
                      <Show when={usage()?.per_model && Object.keys(usage()!.per_model).length > 0}>
                        <div class="agent-card-models">
                          <For each={Object.entries(usage()?.per_model ?? {})}>
                            {([model, mu]) => (
                              <span class="model-usage-badge" title={model}>
                                {shortModel(model)}: {mu.calls}× {formatTokens(mu.input_tokens)}↑ {formatTokens(mu.output_tokens)}↓
                              </span>
                            )}
                          </For>
                        </div>
                      </Show>
                    </div>

                    {/* Controls */}
                    <div class="agent-card-controls">
                      <Show when={canToggleAgent(entry.status)}>
                        <button
                          disabled={busy()}
                          onClick={(e) => { e.stopPropagation(); void runAgentAction(agent_id, entry.status); }}
                          title={entry.status === 'paused' ? 'Resume' : 'Pause'}
                        >
                          {entry.status === 'paused' ? <Play size={14} /> : <Pause size={14} />}
                        </button>
                      </Show>
                      <Show when={promptsForPersona(entry.spec.persona_id).length > 0 && (entry.status === 'active' || entry.status === 'waiting')}>
                        <Popover
                          open={promptPickerFor() === agent_id}
                          onOpenChange={(open) => setPromptPickerFor(open ? agent_id: null)}
                        >
                          <PopoverTrigger
                            as={(triggerProps: any) => (
                              <button
                                disabled={busy()}
                                title="Send prompt template"
                                {...triggerProps}
                                onClick={(e: MouseEvent) => {
                                  e.stopPropagation();
                                  setPromptPickerFor(promptPickerFor() === agent_id ? null : agent_id);
                                }}
                              >
                                <BookOpen size={14} />
                              </button>
                            )}
                          />
                          <PopoverContent class="prompt-picker-dropdown-portal">
                            <For each={promptsForPersona(entry.spec.persona_id)}>
                              {(item) => (
                                <div
                                  class="prompt-picker-item"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setPromptPickerFor(null);
                                    if (!item.template.input_schema?.properties || Object.keys(item.template.input_schema.properties as any).length === 0) {
                                      void invoke('send_prompt_to_bot', {
                                        agent_id,
                                        persona_id: item.persona.id,
                                        prompt_id: item.template.id,
                                        params: {},
                                      }).catch((err: any) => logError('AgentStage', `Failed to send prompt: ${err}`));
                                    } else {
                                      setActivePromptTemplate({ agent_id, persona: item.persona, template: item.template });
                                    }
                                  }}
                                >
                                  <span class="prompt-picker-item-name">{item.template.name || item.template.id}</span>
                                  <Show when={item.template.description}>
                                    <span class="prompt-picker-item-desc">{item.template.description}</span>
                                  </Show>
                                </div>
                              )}
                            </For>
                          </PopoverContent>
                        </Popover>
                      </Show>
                      <button
                        class="config-btn"
                        disabled={busy()}
                        onClick={(e) => { e.stopPropagation(); openConfigDialog(entry); }}
                        title="Reconfigure agent"
                      >
                        <Settings size={14} />
                      </button>
                      <Show when={canKillAgent(entry.status)}>
                        <button
                          class="danger-outline kill-btn"
                          disabled={busy()}
                          onClick={(e) => { e.stopPropagation(); void killAgent(agent_id, entry.spec.name); }}
                          title="Kill agent"
                        >
                          ✕
                        </button>
                      </Show>
                    </div>

                    {/* Error display */}
                    <Show when={entry.status === 'error' && entry.last_error}>
                      <div class="agent-card-error"><XCircle size={14} /> {entry.last_error}</div>
                    </Show>
                  </div>
                </Show>
              );
            }}
          </For>

          {/* Zoom controls overlay */}
          <div class="agent-stage-zoom-controls">
            <button onClick={fitToView} title="Fit all agents in view" class="zoom-btn">
              <Maximize2 size={14} />
            </button>
            <button onClick={() => setStageZoom(z => Math.min(3, z * 1.2))} title="Zoom in" class="zoom-btn">
              <ZoomIn size={14} />
            </button>
            <span class="zoom-level">{Math.round(stageZoom() * 100)}%</span>
            <button onClick={() => setStageZoom(z => Math.max(0.1, z * 0.8))} title="Zoom out" class="zoom-btn">
              <ZoomOut size={14} />
            </button>
            <button onClick={resetZoom} title="Reset zoom to 100%" class="zoom-btn">
              <RotateCcw size={14} />
            </button>
          </div>
        </div>
      </Show>

      {/* Reconfigure dialog */}
      <Dialog open={!!configAgent()} onOpenChange={(open) => { if (!open) setConfigAgent(null); }}>
      <DialogContent class="max-w-[520px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e: Event) => e.preventDefault()}>
      <Show when={configAgent()}>
        {(agent) => {
          const a = agent();
          const currentModel = () => a.active_model || a.spec.model || '';
          const modelChanged = () => configModel() !== currentModel();
          const toolsChanged = () => {
            const current = new Set(a.tools);
            const configured = configEnabledTools();
            if (current.size !== configured.size) return true;
            for (const t of current) if (!configured.has(t)) return true;
            return false;
          };
          const configChanged = () => modelChanged() || toolsChanged();

          return (
              <>
                <DialogHeader class="flex flex-row items-center gap-3 px-6 pt-6 pb-2">
                  <span class="text-2xl">{a.spec.avatar || '🤖'}</span>
                  <div class="flex-1">
                    <DialogTitle class="text-sm font-semibold text-foreground">Reconfigure {a.spec.friendly_name || a.spec.name}</DialogTitle>
                    <div class="text-xs text-muted-foreground">{a.spec.description}</div>
                  </div>
                  <Show when={!isService()}>
                    <Button
                      variant="destructive"
                      size="sm"
                      disabled={busyAgentId() === a.agent_id}
                      onClick={() => void restartAgent(
                        a.agent_id,
                        modelChanged() ? configModel() : undefined,
                      )}
                      title="Restart agent"
                    >
                      {busyAgentId() === a.agent_id ? 'Restarting…' : <><RefreshCw size={14} /> Restart</>}
                    </Button>
                  </Show>
                </DialogHeader>

                <div class="flex-1 overflow-y-auto px-6 py-4">
                  <div class="agent-config-section">
                    <label class="agent-config-label">Status</label>
                    <div class="agent-config-value">
                      <span
                        class="agent-card-status"
                        style={{
                          color: statusColors[a.status],
                          'border-color': `${statusColors[a.status]}55`,
                          'background-color': `${statusColors[a.status]}18`,
                        }}
                      >
                        ● {titleCaseStatus(a.status)}
                      </span>
                    </div>
                  </div>

                  <Show when={a.status === 'error' && a.last_error}>
                    <div class="agent-config-section">
                      <label class="agent-config-label">Error</label>
                      <div class="agent-config-error">{a.last_error}</div>
                    </div>
                  </Show>

                  <div class="agent-config-section">
                    <label class="agent-config-label">Model</label>
                    <select
                      class="agent-config-select"
                      value={configModel()}
                      onInput={(e) => setConfigModel(e.currentTarget.value)}
                    >
                      <Show when={currentModel() && !availableModels().some(m => m.id === currentModel())}>
                        <option value={currentModel()}>{currentModel()} (current)</option>
                      </Show>
                      <For each={availableModels()}>
                        {(model) => (
                          <option value={model.id} selected={model.id === configModel()}>
                            {model.label}{model.id === currentModel() ? ' (current)' : ''}
                          </option>
                        )}
                      </For>
                    </select>
                  </div>

                  <div class="agent-config-section">
                    <label class="agent-config-label">Role</label>
                    <div class="agent-config-value">{a.spec.role as string}</div>
                  </div>

                  <div class="agent-config-section">
                    <label class="agent-config-label">Tools ({a.tools.length})</label>
                    <div class="agent-config-tools">
                      <For each={a.tools}>
                        {(tool_id) => <span class="agent-tool-chip">{tool_id}</span>}
                      </For>
                    </div>
                  </div>

                  {/* ── Permissions (bots only) ────────────────── */}
                  <Show when={isService()}>
                    <div class="agent-config-section">
                      <label class="agent-config-label">Permission Rules</label>
                      <PermissionRulesEditor
                        rules={configPermRules}
                        setRules={(rules) => setConfigPermRules(rules)}
                      />
                    </div>
                  </Show>
                </div>

                <DialogFooter class="px-6 pb-6 pt-2">
                  <Button
                    variant="outline"
                    onClick={() => setConfigAgent(null)}
                  >
                    Cancel
                  </Button>
                  <Button
                    disabled={busyAgentId() === a.agent_id}
                    onClick={async () => {
                      try {
                        if (isService()) {
                          await invoke('set_bot_permissions', { agent_id: a.agent_id, permissions: { rules: configPermRules() } });
                          logInfo('agent-config', 'Bot config saved');
                        } else if (modelChanged()) {
                          await restartAgent(a.agent_id, configModel());
                        }
                        setConfigAgent(null);
                      } catch (err: any) {
                        setError(err?.toString?.() ?? 'Failed to save config.');
                      }
                    }}
                  >
                    Save
                  </Button>
                </DialogFooter>
              </>
          );
        }}
      </Show>
      </DialogContent>
      </Dialog>

      {/* Confirm kill dialog */}
      <Dialog open={!!confirmKill()} onOpenChange={(open) => { if (!open) setConfirmKill(null); }}>
      <DialogContent class="max-w-[380px] w-[90vw] p-5" onInteractOutside={(e: Event) => e.preventDefault()}>
      <Show when={confirmKill()}>
        {(target) => (
              <>
              <DialogHeader>
                <DialogTitle class="text-sm font-semibold">
                  {isService() ? <><Trash2 size={14} /> Delete</> : <><XCircle size={14} /> Kill</>} Agent
                </DialogTitle>
              </DialogHeader>
              <p class="text-sm text-muted-foreground">
                Are you sure you want to {isService() ? 'delete' : 'kill'} <strong>{target().name}</strong>?
                {isService() ? ' This will remove the agent and its configuration.' : ''}
              </p>
              <DialogFooter class="flex flex-row gap-2 justify-end">
                <Button variant="outline" onClick={() => setConfirmKill(null)}>Cancel</Button>
                <Button
                  variant="destructive"
                  onClick={() => void executeKill()}
                >
                  {isService() ? 'Delete' : 'Kill'}
                </Button>
              </DialogFooter>
              </>
        )}
      </Show>
      </DialogContent>
      </Dialog>

      {/* Question dialog — opens when clicking ❓ badge */}
      <Dialog open={!!questionDialogQuestion()} onOpenChange={(open) => { if (!open) setQuestionDialogQuestion(null); }}>
      <DialogContent class="max-w-[520px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e: Event) => e.preventDefault()}>
      <Show when={questionDialogQuestion()}>
        {(q) => {
          const [stageQMsSelected, setStageQMsSelected] = createSignal<Set<number>>(new Set());
          const answerQuestion = async (choiceIdx?: number, text?: string, selected_choices?: number[]) => {
            if (questionSending()) return;
            setQuestionSending(true);
            const question = q();
            let label: string;
            if (selected_choices && selected_choices.length > 0) {
              label = selected_choices.map((i) => question.choices[i]).join(', ');
            } else {
              label = text || (choiceIdx !== undefined ? question.choices[choiceIdx] : '');
            }
            try {
              await routeAnswerQuestion(
                {
                  request_id: question.request_id,
                  entity_id: question.agent_id ? `agent/${question.agent_id}` : `session/${props.session_id}`,
                  source_name: question.agent_name ?? '',
                  type: 'question',
                  session_id: isService() ? undefined : props.session_id,
                  agent_id: question.agent_id,
                } as PendingInteraction,
                {
                  ...(choiceIdx !== undefined ? { selected_choice: choiceIdx } : {}),
                  ...(selected_choices !== undefined ? { selected_choices } : {}),
                  ...(text ? { text } : {}),
                },
              );
              props.onQuestionAnswered(question.request_id, label);
              setQuestionDialogQuestion(null);
            } catch (err) {
              console.error('Failed to respond:', err);
              setQuestionSending(false);
            }
          };

          return (
              <>
                <DialogHeader class="flex flex-row items-center gap-3 px-6 pt-6 pb-2">
                  <span class="text-2xl"><HelpCircle size={16} /></span>
                  <div class="flex-1">
                    <DialogTitle class="text-sm font-semibold text-foreground">Question from agent</DialogTitle>
                    <Show when={q().agent_name}>
                      <div class="text-xs text-muted-foreground">{q().agent_name}</div>
                    </Show>
                  </div>
                  <button
                    class="text-muted-foreground hover:text-foreground transition-colors"
                    onClick={() => setQuestionDialogQuestion(null)}
                    aria-label="Dismiss"
                  >
                    <X size={16} />
                  </button>
                </DialogHeader>

                <div class="flex-1 overflow-y-auto px-6 py-4">
                  <div class="agent-question-dialog-text" innerHTML={renderMarkdown(q().text)} />

                  <Show when={q().choices.length > 0}>
                    <div class="agent-question-dialog-choices">
                      <For each={q().choices}>
                        {(choice, idx) => (
                          <button
                            class={q().multi_select && stageQMsSelected().has(idx()) ? 'agent-question-dialog-choice agent-question-dialog-choice-selected' : 'agent-question-dialog-choice'}
                            disabled={questionSending()}
                            onClick={() => {
                              if (q().multi_select) {
                                setStageQMsSelected((prev) => {
                                  const next = new Set(prev);
                                  if (next.has(idx())) next.delete(idx());
                                  else next.add(idx());
                                  return next;
                                });
                              } else {
                                void answerQuestion(idx(), choice);
                              }
                            }}
                          >
                            {choice}
                          </button>
                        )}
                      </For>
                    </div>
                    <Show when={q().multi_select}>
                      <button
                        class="agent-question-dialog-choice"
                        disabled={stageQMsSelected().size === 0 || questionSending()}
                        onClick={() => {
                          const indices = [...stageQMsSelected()].sort((a, b) => a - b);
                          void answerQuestion(undefined, undefined, indices);
                        }}
                      >
                        {questionSending() ? '…' : 'Submit'}
                      </button>
                    </Show>
                  </Show>

                  <div class="agent-question-dialog-freeform">
                    <input
                      type="text"
                      placeholder="Type your answer…"
                      value={questionFreeText()}
                      onInput={(e) => setQuestionFreeText(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' && questionFreeText().trim()) {
                          e.preventDefault();
                          void answerQuestion(undefined, questionFreeText().trim());
                        }
                      }}
                      disabled={questionSending()}
                    />
                    <button
                      disabled={!questionFreeText().trim() || questionSending()}
                      onClick={() => void answerQuestion(undefined, questionFreeText().trim())}
                    >
                      {questionSending() ? '…' : '→'}
                    </button>
                  </div>
                </div>
              </>
          );
        }}
      </Show>
      </DialogContent>
      </Dialog>

      {/* Approval dialog — opens when clicking 🔐 badge */}
      <Dialog open={!!approvalDialogItem()} onOpenChange={(open) => { if (!open) setApprovalDialogItem(null); }}>
      <DialogContent class="max-w-[520px] w-[90vw] max-h-[80vh] flex flex-col p-0" onInteractOutside={(e: Event) => e.preventDefault()}>
      <Show when={approvalDialogItem()}>
        {(item) => {
          const handleApproval = async (approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => {
            if (approvalSending()) return;
            setApprovalSending(true);
            const a = item();
            dismissAgentApproval(a.request_id);
            try {
              await routeRespondToApproval(
                {
                  request_id: a.request_id,
                  entity_id: `agent/${a.agent_id}`,
                  source_name: a.agent_name || a.agent_id,
                  type: 'tool_approval',
                  session_id: isService() ? undefined : props.session_id,
                  agent_id: a.agent_id,
                } as PendingInteraction,
                { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
              );
              const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
              logInfo('approval', `${approved ? 'Approved' : 'Denied'}${scope} ${a.tool_id} for ${a.agent_name}`);
              setApprovalDialogItem(null);
            } catch (err: any) {
              logError('approval', `Failed to respond: ${err}`);
              setApprovalSending(false);
            }
          };

          return (
              <>
                <DialogHeader class="flex flex-row items-center gap-3 px-6 pt-6 pb-2">
                  <span class="text-2xl"><ShieldAlert size={16} /></span>
                  <div>
                    <DialogTitle class="text-sm font-semibold text-foreground">Tool Approval Required</DialogTitle>
                    <div class="text-xs text-muted-foreground">{item().agent_name || item().agent_id}</div>
                  </div>
                </DialogHeader>

                <div class="flex-1 overflow-y-auto px-6 py-4">
                  <div class="agent-config-section">
                    <label class="agent-config-label">Tool</label>
                    <div class="agent-config-value"><strong>{item().tool_id}</strong></div>
                  </div>

                  <div class="agent-config-section">
                    <label class="agent-config-label">Reason</label>
                    <div class="agent-config-value">{item().reason}</div>
                  </div>

                  <Show when={item().input}>
                    <div class="agent-config-section">
                      <label class="agent-config-label">Input</label>
                      <pre class="agent-approval-dialog-input" innerHTML={highlightYaml(item().input!)} />
                    </div>
                  </Show>
                </div>

                <DialogFooter class="px-6 pb-6 pt-2 flex-wrap gap-2">
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(false)}
                  >
                    <XCircle size={14} /> Deny
                  </Button>
                  <Button
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true)}
                  >
                    {approvalSending() ? 'Sending…' : <><CheckCircle size={14} /> Approve</>}
                  </Button>
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true, { allow_agent: true })}
                  >
                    Allow for Agent
                  </Button>
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true, { allow_session: true })}
                  >
                    Allow for Session
                  </Button>
                </DialogFooter>
              </>
          );
        }}
      </Show>
      </DialogContent>
      </Dialog>

      {/* Event log / detail panel — opens when clicking agent card header */}
      <Show when={expandedAgentId()}>
        {(eid) => {
          const logAgent = () => agents().find((a) => a.agent_id === eid());
          return (
            <Show when={logAgent()}>
              {(la) => (
                <Show when={isService()} fallback={
                  /* Session agents: event log with EventLogList */
                  <Dialog open={!!expandedAgentId()} onOpenChange={(open) => { if (!open) { setExpandedAgentId(null); setAgentEvents([]); setAgentEventsTotal(0); } }}>
                  <DialogContent class="max-w-[640px] w-[90vw] max-h-[80vh] flex flex-col p-0">
                    <>
                      <DialogHeader class="flex flex-row items-center gap-3 px-6 pt-6 pb-2">
                        <span class="text-2xl">{la().spec.avatar || '🤖'}</span>
                        <div class="flex-1">
                          <DialogTitle class="text-sm font-semibold text-foreground">
                            {la().spec.friendly_name || la().spec.name} — Event Log
                            <Show when={agentEventsTotal() > 0}>
                              <span class="font-normal text-[0.85em] text-muted-foreground ml-1.5">
                                ({agentEventsTotal()} events)
                              </span>
                            </Show>
                          </DialogTitle>
                          <div class="text-xs text-muted-foreground">{la().agent_id}</div>
                        </div>
                        <button
                          class="icon-btn ml-auto shrink-0"
                          aria-label="Close"
                          onClick={() => { setExpandedAgentId(null); setAgentEvents([]); setAgentEventsTotal(0); }}
                        >✕</button>
                      </DialogHeader>
                      <div class="flex-1 overflow-y-auto px-6 py-4 agent-log-body">
                        <EventLogList
                          events={agentEvents()}
                          totalCount={agentEventsTotal()}
                          loading={eventsLoading()}
                          hasMore={agentEvents().length < agentEventsTotal()}
                          onLoadMore={() => void loadAgentEvents(eid(), true)}
                          onApprove={(reqId, approved) => void respondToApproval(eid(), reqId, approved)}
                        />
                      </div>
                    </>
                  </DialogContent>
                  </Dialog>
                }>
                  {/* Bots: tabbed detail panel */}
                  <BotDetailPanel
                    agent_id={la().agent_id}
                    agent_name={la().spec.friendly_name || la().spec.name}
                    agentAvatar={la().spec.avatar || '🤖'}
                    agentStatus={la().status}
                    events={agentEvents()}
                    totalCount={agentEventsTotal()}
                    eventsLoading={eventsLoading()}
                    onClose={() => { setExpandedAgentId(null); setAgentEvents([]); setAgentEventsTotal(0); }}
                    onApprove={(reqId, approved) => void respondToApproval(eid(), reqId, approved)}
                    onLoadMore={() => void loadAgentEvents(eid(), true)}
                  />
                </Show>
              )}
            </Show>
          );
        }}
      </Show>
      {/* Prompt parameter dialog for sending to agent */}
      <Show when={activePromptTemplate()}>
        {(data) => {
          const d = data();
          return (
            <PromptParameterDialog
              template={d.template}
              submitLabel="Send to Agent"
              onSubmit={(rendered, params) => {
                const pid = d.persona.id;
                void invoke('send_prompt_to_bot', {
                  agent_id: d.agent_id,
                  persona_id: pid,
                  prompt_id: d.template.id,
                  params,
                }).catch((err: any) => logError('AgentStage', `Failed to send prompt: ${err}`));
                setActivePromptTemplate(null);
              }}
              onCancel={() => setActivePromptTemplate(null)}
            />
          );
        }}
      </Show>
    </div>
  );
};

export default AgentStage;
