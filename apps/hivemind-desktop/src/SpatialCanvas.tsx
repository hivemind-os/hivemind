import { Component, createSignal, onMount, onCleanup, For, createEffect, createMemo, Show, untrack, type JSX, type Accessor, type Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { highlightYaml } from './components/YamlHighlight';
import { renderMarkdown } from './utils';
import { getThemeFamily } from './stores/themeStore';
import type { ActivityItem } from './stores/streamingStore';
import type { ChatRunState, Persona, MessageAttachment, PromptInjectionReview } from './types';
import type { PendingQuestion } from './components/InlineQuestion';
import InlineQuestion from './components/InlineQuestion';
import { MessageSquare, Bot, FileText, Pin, Folder, GitBranch, Wrench, Diamond, Link, X, Zap, Target, TreePine, TriangleAlert, ChevronUp, ChevronDown, RefreshCw, Ban, Lightbulb, Pencil, Wind, Trash2, Search, Ruler, Tag, Package, Brain, Pause, Square, Play, Paperclip, FolderOpen, Settings, Shield, Loader2, Copy, Check, GitBranch as Workflow } from 'lucide-solid';

// Types matching the backend hive-canvas types
interface CanvasNode {
  id: string;
  canvas_id: string;
  card_type: 'prompt' | 'response' | 'artifact' | 'reference' | 'cluster' | 'decomposition' | 'tool_call' | 'decision_point' | 'synthesis' | 'dead_end';
  x: number;
  y: number;
  width: number;
  height: number;
  content: { text?: string; [key: string]: unknown };
  status: 'active' | 'dead_end' | 'archived';
  created_by: string;
  created_at: number;
}

interface CanvasEdge {
  id: string;
  canvas_id: string;
  source_id: string;
  target_id: string;
  edge_type: string;
  metadata: unknown;
  created_at: number;
}

type LayoutAlgorithm = 'tree' | 'force_directed' | 'radial';

interface LayoutProposal {
  proposalId: string;
  algorithm: string;
  positions: Array<{ node_id: string; x: number; y: number }>;
  message: string;
}

interface ToolCallRecord {
  id: string;
  tool_id: string;
  label: string;
  input?: string;
  output?: string;
  isError: boolean;
  startedAt: number;
  completedAt?: number;
}

interface SpatialCanvasProps {
  session_id: string;
  onSendMessage: (content: string, position?: { x: number; y: number }) => void;
  messages?: { id: string; role: string; content: string; created_at_ms: number; [key: string]: any }[];
  streamingContent?: string;

  // Session state
  activeSessionState?: Accessor<ChatRunState | null>;
  daemonOnline?: Accessor<boolean>;
  isStreaming?: Accessor<boolean>;
  activities?: Accessor<ActivityItem[]>;
  busyAction?: Accessor<string | null>;

  // Tool / question / review state
  toolCallHistory?: Accessor<Record<string, ToolCallRecord[]>>;
  allQuestions?: Accessor<(PendingQuestion & { answer?: string })[]>;
  onQuestionAnswered?: (request_id: string, answerText: string) => void;
  pendingReview?: Accessor<PromptInjectionReview | null>;
  sendDecision?: (decision?: any) => Promise<void>;

  // Session controls
  interrupt?: (mode: 'soft' | 'hard') => Promise<void>;
  resume?: () => Promise<void>;

  // Config/dialog triggers (dialogs live in ChatView)
  onShowConfig?: () => void;
  onShowPermissions?: () => void;
  onShowSettings?: (tab?: string) => void;
  onShowMemories?: () => void;
  onUploadFiles?: () => Promise<void>;
  onShowWorkflowLauncher?: () => void;
  onShowToolCall?: (tc: ToolCallRecord) => void;

  // Composer enrichment
  personas?: Accessor<Persona[]>;
  selectedAgentId?: Accessor<string>;
  chatFontPx?: () => string;
  draft?: Accessor<string>;
  setDraft?: Setter<string>;
  pendingAttachments?: Accessor<MessageAttachment[]>;
  setPendingAttachments?: Setter<MessageAttachment[]>;

  // Workflow state
  activeChatWorkflows?: Accessor<{ instanceId: number; instance: any; events: any[] }[]>;
  terminalChatWorkflows?: Accessor<{ instanceId: number; instance: any; events: any[] }[]>;
  onPauseChatWorkflow?: (instanceId: number) => void;
  onResumeChatWorkflow?: (instanceId: number) => void;
  onKillChatWorkflow?: (instanceId: number) => void;

  // Session info
  workspace_linked?: boolean;
}

// Shared icons for card types (theme-independent)
const CARD_ICONS: Record<string, () => JSX.Element> = {
  prompt: () => <MessageSquare size={14} />,
  response: () => <Bot size={14} />,
  artifact: () => <FileText size={14} />,
  reference: () => <Pin size={14} />,
  cluster: () => <Folder size={14} />,
  decomposition: () => <GitBranch size={14} />,
  tool_call: () => <Wrench size={14} />,
  decision_point: () => <Diamond size={14} />,
  synthesis: () => <Link size={14} />,
  dead_end: () => <X size={14} />,
};

// Card background/border colors per theme
const CARD_COLORS_DARK: Record<string, { bg: string; border: string }> = {
  prompt: { bg: '#1e3a5f', border: '#3b82f6' },
  response: { bg: '#1a3a2a', border: '#22c55e' },
  artifact: { bg: '#3a2a1a', border: '#f59e0b' },
  reference: { bg: '#2a1a3a', border: '#a855f7' },
  cluster: { bg: '#1a2a3a', border: '#06b6d4' },
  decomposition: { bg: '#2a2a1a', border: '#eab308' },
  tool_call: { bg: '#1a2a2a', border: '#14b8a6' },
  decision_point: { bg: '#3a1a2a', border: '#ec4899' },
  synthesis: { bg: '#1a3a1a', border: '#10b981' },
  dead_end: { bg: '#2a2a2a', border: '#6b7280' },
};

const CARD_COLORS_LIGHT: Record<string, { bg: string; border: string }> = {
  prompt: { bg: '#e8f0fe', border: '#3b82f6' },
  response: { bg: '#e6f4ea', border: '#22c55e' },
  artifact: { bg: '#fef3e2', border: '#f59e0b' },
  reference: { bg: '#f3e8ff', border: '#a855f7' },
  cluster: { bg: '#e0f7fa', border: '#06b6d4' },
  decomposition: { bg: '#fef9e7', border: '#eab308' },
  tool_call: { bg: '#e0f2f1', border: '#14b8a6' },
  decision_point: { bg: '#fce4ec', border: '#ec4899' },
  synthesis: { bg: '#e8f5e9', border: '#10b981' },
  dead_end: { bg: '#e8e8e8', border: '#6b7280' },
};

interface ThemePalette {
  canvasBg: string;
  gridStroke: string;
  textPrimary: string;
  textSecondary: string;
  textMuted: string;
  textDead: string;
  surfacePrimary: string;
  surfaceHover: string;
  surfaceBorder: string;
  surfaceOverlay: string;
  surfaceOverlay95: string;
  surfaceShadow: string;
  selectedBorder: string;
  selectedGlow: string;
  conflictBorder: string;
  conflictGlow: string;
  searchMatchBorder: string;
  searchMatchGlow: string;
  accentBlue: string;
  accentGreen: string;
  destructive: string;
  linkHighlight: string;
  deadCardBg: string;
  buttonExpandBg: string;
  clusterFill: string;
  clusterStroke: string;
  minimapBg: string;
  minimapViewportStroke: string;
  minimapViewportFill: string;
  layoutGhostBorder: string;
  layoutGhostBg: string;
}

const THEME_DARK: ThemePalette = {
  canvasBg: '#0d0d1a',
  gridStroke: 'rgba(255, 255, 255, 0.05)',
  textPrimary: '#cdd6f4',
  textSecondary: '#a6adc8',
  textMuted: '#6b7280',
  textDead: '#6b7280',
  surfacePrimary: '#1e1e2e',
  surfaceHover: '#313244',
  surfaceBorder: '#45475a',
  surfaceOverlay: 'rgba(30,30,46,0.9)',
  surfaceOverlay95: 'rgba(30,30,46,0.95)',
  surfaceShadow: 'rgba(0,0,0,0.35)',
  selectedBorder: '#fff',
  selectedGlow: 'rgba(255,255,255,0.2)',
  conflictBorder: '#ef4444',
  conflictGlow: 'rgba(239,68,68,0.3)',
  searchMatchBorder: '#f59e0b',
  searchMatchGlow: 'rgba(245,158,11,0.6)',
  accentBlue: '#3b82f6',
  accentGreen: '#10b981',
  destructive: '#f38ba8',
  linkHighlight: '#89b4fa',
  deadCardBg: '#1a1a1a',
  buttonExpandBg: 'rgba(17,17,27,0.5)',
  clusterFill: 'rgba(6,182,212,0.08)',
  clusterStroke: 'rgba(6,182,212,0.6)',
  minimapBg: 'rgba(13,13,26,0.9)',
  minimapViewportStroke: 'rgba(255,255,255,0.9)',
  minimapViewportFill: 'rgba(255,255,255,0.16)',
  layoutGhostBorder: 'rgba(137,180,250,0.6)',
  layoutGhostBg: 'rgba(137,180,250,0.08)',
};

const THEME_LIGHT: ThemePalette = {
  canvasBg: '#f0f2f5',
  gridStroke: 'rgba(0, 0, 0, 0.06)',
  textPrimary: '#1e1e2e',
  textSecondary: '#5c5f77',
  textMuted: '#9ca3af',
  textDead: '#9ca3af',
  surfacePrimary: '#ffffff',
  surfaceHover: '#f0f0f5',
  surfaceBorder: '#d4d4d8',
  surfaceOverlay: 'rgba(255,255,255,0.95)',
  surfaceOverlay95: 'rgba(255,255,255,0.97)',
  surfaceShadow: 'rgba(0,0,0,0.12)',
  selectedBorder: '#1e1e2e',
  selectedGlow: 'rgba(0,0,0,0.12)',
  conflictBorder: '#ef4444',
  conflictGlow: 'rgba(239,68,68,0.2)',
  searchMatchBorder: '#f59e0b',
  searchMatchGlow: 'rgba(245,158,11,0.3)',
  accentBlue: '#3b82f6',
  accentGreen: '#10b981',
  destructive: '#dc2626',
  linkHighlight: '#3b82f6',
  deadCardBg: '#e8e8e8',
  buttonExpandBg: 'rgba(255,255,255,0.7)',
  clusterFill: 'rgba(6,182,212,0.06)',
  clusterStroke: 'rgba(6,182,212,0.5)',
  minimapBg: 'rgba(240,242,245,0.95)',
  minimapViewportStroke: 'rgba(0,0,0,0.7)',
  minimapViewportFill: 'rgba(0,0,0,0.08)',
  layoutGhostBorder: 'rgba(59,130,246,0.5)',
  layoutGhostBg: 'rgba(59,130,246,0.06)',
};

const EDGE_COLORS: Record<string, string> = {
  reply_to: '#6b7280',
  references: '#3b82f6',
  contradicts: '#ef4444',
  evolves: '#8b5cf6',
  tool_io: '#14b8a6',
  decomposes_to: '#eab308',
  synthesizes: '#10b981',
  delegation: '#f59e0b',
  context_share: '#06b6d4',
  artifact_pass: '#f97316',
  feedback_loop: '#d946ef',
  blocked_by: '#dc2626',
};

const setsEqual = (a: Set<string>, b: Set<string>) => {
  if (a.size !== b.size) return false;
  for (const value of a) {
    if (!b.has(value)) return false;
  }
  return true;
};

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

const LAYOUT_ALGORITHM_LABELS: Record<LayoutAlgorithm, string> = {
  tree: 'Tree',
  force_directed: 'Force',
  radial: 'Radial',
};

const SpatialCanvas: Component<SpatialCanvasProps> = (props) => {
  // ── Layout persistence helpers ──────────────────────────────────
  const layoutKey = () => `spatial-layout-${props.session_id}`;
  const viewportKey = () => `spatial-viewport-${props.session_id}`;

  const loadSavedLayout = (): Map<string, { x: number; y: number }> => {
    try {
      const raw = localStorage.getItem(layoutKey());
      if (!raw) return new Map();
      const entries = JSON.parse(raw) as Array<[string, { x: number; y: number }]>;
      return new Map(entries);
    } catch { return new Map(); }
  };

  const saveLayout = (currentNodes: CanvasNode[]) => {
    const entries = currentNodes.map(n => [n.id, { x: n.x, y: n.y }] as [string, { x: number; y: number }]);
    try { localStorage.setItem(layoutKey(), JSON.stringify(entries)); } catch {}
  };

  const loadSavedViewport = (): { cx: number; cy: number; z: number } | null => {
    try {
      const raw = localStorage.getItem(viewportKey());
      return raw ? JSON.parse(raw) : null;
    } catch { return null; }
  };

  const saveViewport = (cx: number, cy: number, z: number) => {
    try { localStorage.setItem(viewportKey(), JSON.stringify({ cx, cy, z })); } catch {}
  };

  const savedViewport = loadSavedViewport();

  // Viewport state
  const [centerX, setCenterX] = createSignal(savedViewport?.cx ?? 0);
  const [centerY, setCenterY] = createSignal(savedViewport?.cy ?? 0);
  const [zoom, setZoom] = createSignal(savedViewport?.z ?? 1);

  // Canvas data (client-side for now; backend persistence comes with S6)
  const [nodes, setNodes] = createSignal<CanvasNode[]>([]);
  const [edges, setEdges] = createSignal<CanvasEdge[]>([]);

  // Track session switches — restore viewport & clear stale nodes
  const [prevSessionId, setPrevSessionId] = createSignal(props.session_id);
  createEffect(() => {
    const sid = props.session_id;
    lastSequence = 0;
    const prev = untrack(() => prevSessionId());
    if (sid !== prev) {
      // Save outgoing session state before switching
      const outgoingNodes = untrack(() => nodes());
      if (outgoingNodes.length > 0) {
        saveLayout(outgoingNodes);
      }
      saveViewport(untrack(centerX), untrack(centerY), untrack(zoom));

      // Clear nodes so message sync will load saved positions for new session
      setNodes([]);
      setEdges([]);
      setLayoutProposal(null);
      setClusterStatus(null);
      if (proposalAnimationFrame !== null) {
        cancelAnimationFrame(proposalAnimationFrame);
        proposalAnimationFrame = null;
      }

      // Restore viewport for new session
      const vp = loadSavedViewport();
      setCenterX(vp?.cx ?? 0);
      setCenterY(vp?.cy ?? 0);
      setZoom(vp?.z ?? 1);

      setPrevSessionId(sid);
    }
  });

  // Interaction state
  const [dragging, setDragging] = createSignal<string | null>(null);
  const [dragOffset, setDragOffset] = createSignal({ x: 0, y: 0 });
  const [panning, setPanning] = createSignal(false);
  const [panStart, setPanStart] = createSignal({ x: 0, y: 0 });
  const [selectedNodes, setSelectedNodes] = createSignal<Set<string>>(new Set());
  const [contextPrompt, setContextPrompt] = createSignal<{ x: number; y: number } | null>(null);
  const [promptText, setPromptText] = createSignal('');
  const [globalPromptText, setGlobalPromptText] = createSignal('');
  const [showPersonaPicker, setShowPersonaPicker] = createSignal(false);
  const [contextMenu, setContextMenu] = createSignal<{ x: number; y: number; nodeId: string } | null>(null);
  const [autoLayout, setAutoLayout] = createSignal(false);
  const [expandedCards, setExpandedCards] = createSignal<Set<string>>(new Set());
  const [positionHints, setPositionHints] = createSignal<Map<string, { x: number; y: number }>>(new Map());
  const [linking, setLinking] = createSignal<{ source_id: string; cursorX: number; cursorY: number } | null>(null);
  const [redirecting, setRedirecting] = createSignal<{ nodeId: string; cursorX: number; cursorY: number } | null>(null);
  const [linkTypePicker, setLinkTypePicker] = createSignal<{ source_id: string; targetId: string; x: number; y: number } | null>(null);
  const [selectedEdge, setSelectedEdge] = createSignal<string | null>(null);
  const [showArchived, setShowArchived] = createSignal(false);
  const [searchQuery, setSearchQuery] = createSignal('');
  const [showSearch, setShowSearch] = createSignal(false);
  const [clusterStatus, setClusterStatus] = createSignal<string | null>(null);
  const [layoutProposal, setLayoutProposal] = createSignal<LayoutProposal | null>(null);
  const [layoutAlgorithm, setLayoutAlgorithm] = createSignal<LayoutAlgorithm>('tree');
  const [copiedCardId, setCopiedCardId] = createSignal<string | null>(null);
  const [isDraggingFile, setIsDraggingFile] = createSignal(false);

  let containerRef: HTMLDivElement | undefined;
  let canvasRef: HTMLCanvasElement | undefined;
  let promptInputRef: HTMLTextAreaElement | undefined;
  let globalInputRef: HTMLInputElement | undefined;
  let wsRef: WebSocket | null = null;
  let attachmentIdCounter = 0;

  const ACCEPTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
  const MAX_IMAGE_BYTES = 10 * 1024 * 1024;

  const addImageFiles = (files: FileList | File[]) => {
    if (!props.setPendingAttachments) return;
    for (const file of Array.from(files)) {
      if (!ACCEPTED_IMAGE_TYPES.includes(file.type)) continue;
      if (file.size > MAX_IMAGE_BYTES) continue;
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result;
        if (typeof result !== 'string') return;
        const base64 = result.split(',', 2)[1] ?? '';
        const att: MessageAttachment = {
          id: `spatial-att-${attachmentIdCounter++}`,
          filename: file.name,
          media_type: file.type,
          data: base64,
        };
        props.setPendingAttachments!((prev) => [...prev, att]);
      };
      reader.readAsDataURL(file);
    }
  };

  const handleFileDragOver = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDraggingFile(true);
  };

  const handleFileDragLeave = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDraggingFile(false);
  };

  const handleFileDrop = (e: DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDraggingFile(false);
    if (e.dataTransfer?.files?.length) {
      addImageFiles(e.dataTransfer.files);
    }
  };

  const handleInputPaste = (e: ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items || !props.setPendingAttachments) return;
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
  let lastSequence = 0;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let layoutFrame: number | null = null;
  let proposalAnimationFrame: number | null = null;
  const layoutVelocities = new Map<string, { x: number; y: number }>();
  const pinnedNodes = new Set<string>();
  const scrollPositions = new Map<string, number>();

  // Theme-aware palette
  const isDark = () => getThemeFamily() === 'dark';
  const theme = createMemo(() => isDark() ? THEME_DARK : THEME_LIGHT);
  const cardColors = createMemo(() => isDark() ? CARD_COLORS_DARK : CARD_COLORS_LIGHT);

  // Coordinate transforms
  const containerRect = () => containerRef?.getBoundingClientRect() ?? { width: 0, height: 0, left: 0, top: 0 };

  const isBackgroundTarget = (target: EventTarget | null) => {
    const el = target as HTMLElement | null;
    return !!el && (el === containerRef || el === canvasRef || el.tagName.toLowerCase() === 'svg');
  };

  const canvasToScreen = (cx: number, cy: number) => {
    const rect = containerRect();
    const z = zoom();
    return {
      x: (cx - centerX()) * z + rect.width / 2,
      y: (cy - centerY()) * z + rect.height / 2,
    };
  };

  const screenToCanvas = (sx: number, sy: number) => {
    const rect = containerRect();
    const z = zoom();
    return {
      x: (sx - rect.width / 2) / z + centerX(),
      y: (sy - rect.height / 2) / z + centerY(),
    };
  };

  const sendCanvasMessage = (message: Record<string, unknown>) => {
    if (wsRef?.readyState === WebSocket.OPEN) {
      wsRef.send(JSON.stringify(message));
    }
  };

  const clearSelection = () => setSelectedNodes(prev => (prev.size ? new Set<string>() : prev));

  const formatCardType = (type: string) => type.replace(/_/g, ' ');

  const getNodeText = (node: CanvasNode) => {
    if (typeof node.content?.text === 'string' && node.content.text.trim()) {
      return node.content.text;
    }
    return JSON.stringify(node.content, null, 2);
  };

  const getNodeTitle = (node: CanvasNode) => {
    const text = getNodeText(node).replace(/\s+/g, ' ').trim();
    if (!text) return formatCardType(node.card_type);
    return text.length > 42 ? `${text.slice(0, 39)}…` : text;
  };

  const focusSearchInput = () => {
    window.setTimeout(() => document.getElementById('spatial-search-input')?.focus(), 50);
  };

  const activeNodes = createMemo(() => {
    const all = nodes();
    return showArchived() ? all : all.filter(node => node.status !== 'archived');
  });

  const searchMatches = createMemo(() => {
    const q = searchQuery().toLowerCase().trim();
    if (!q) return null;
    const matches = new Set<string>();
    for (const node of nodes()) {
      const text = node.content?.text ?? '';
      if (text.toLowerCase().includes(q)) matches.add(node.id);
    }
    return matches;
  });

  // Visible nodes (viewport culling)
  const visibleNodes = createMemo(() => {
    const rect = containerRect();
    const eligible = activeNodes();
    if (!rect.width) return eligible;
    const z = zoom();
    const buffer = Math.max(400, 800 / z);
    const halfW = rect.width / 2 / z;
    const halfH = rect.height / 2 / z;
    const cx = centerX();
    const cy = centerY();

    return eligible.filter(node =>
      node.x + node.width >= cx - halfW - buffer &&
      node.x <= cx + halfW + buffer &&
      node.y + node.height >= cy - halfH - buffer &&
      node.y <= cy + halfH + buffer
    );
  });

  const visibleEdges = createMemo(() => {
    const visible = new Set(visibleNodes().map(node => node.id));
    const archived = showArchived();
    const allNodes = nodes();
    return edges().filter(edge => {
      const srcNode = allNodes.find(node => node.id === edge.source_id);
      const tgtNode = allNodes.find(node => node.id === edge.target_id);
      if (!srcNode || !tgtNode) return false;
      if (!archived && (srcNode.status === 'archived' || tgtNode.status === 'archived')) return false;
      return visible.has(edge.source_id) || visible.has(edge.target_id);
    });
  });

  const activeCardNodes = createMemo(() => activeNodes().filter(node => node.card_type !== 'cluster'));

  const visibleCardNodes = createMemo(() => visibleNodes().filter(node => node.card_type !== 'cluster'));

  const clusterCount = createMemo(() => nodes().filter(node => node.card_type === 'cluster').length);

  const conflictedNodes = createMemo(() => {
    const ids = new Set<string>();
    for (const edge of edges()) {
      if (edge.edge_type === 'contradicts') {
        ids.add(edge.source_id);
        ids.add(edge.target_id);
      }
    }
    return ids;
  });

  const selectedCanvasNodes = createMemo(() => {
    const selected = selectedNodes();
    return activeNodes().filter(node => selected.has(node.id));
  });

  const selectionCount = createMemo(() => selectedCanvasNodes().length);

  const synthesizeAnchor = createMemo(() => {
    const selected = selectedCanvasNodes();
    const rect = containerRect();
    if (selected.length < 2 || !rect.width || !rect.height) return null;

    const centroid = selected.reduce(
      (acc, node) => {
        const screen = canvasToScreen(node.x + node.width / 2, node.y + node.height / 2);
        return { x: acc.x + screen.x, y: acc.y + screen.y };
      },
      { x: 0, y: 0 }
    );

    return {
      x: clamp(centroid.x / selected.length, 96, rect.width - 96),
      y: clamp(centroid.y / selected.length - 52, 16, rect.height - 48),
    };
  });

  const minimapData = createMemo(() => {
    const items = activeCardNodes();
    if (!items.length) return null;

    const rect = containerRect();
    const width = 180;
    const height = 120;

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;

    for (const node of items) {
      minX = Math.min(minX, node.x);
      minY = Math.min(minY, node.y);
      maxX = Math.max(maxX, node.x + node.width);
      maxY = Math.max(maxY, node.y + node.height);
    }

    const spanX = Math.max(1, maxX - minX);
    const spanY = Math.max(1, maxY - minY);
    const padX = Math.max(40, spanX * 0.1);
    const padY = Math.max(40, spanY * 0.1);
    const paddedMinX = minX - padX;
    const paddedMinY = minY - padY;
    const paddedWidth = spanX + padX * 2;
    const paddedHeight = spanY + padY * 2;
    const scale = Math.min(width / paddedWidth, height / paddedHeight);
    const contentWidth = paddedWidth * scale;
    const contentHeight = paddedHeight * scale;
    const offsetX = (width - contentWidth) / 2;
    const offsetY = (height - contentHeight) / 2;
    const viewportWidth = rect.width ? rect.width / zoom() * scale : width;
    const viewportHeight = rect.height ? rect.height / zoom() * scale : height;
    const viewportX = offsetX + (centerX() - rect.width / 2 / zoom() - paddedMinX) * scale;
    const viewportY = offsetY + (centerY() - rect.height / 2 / zoom() - paddedMinY) * scale;

    return {
      width,
      height,
      paddedWidth,
      paddedHeight,
      offsetX,
      offsetY,
      scale,
      minX: paddedMinX,
      minY: paddedMinY,
      viewport: {
        x: viewportX,
        y: viewportY,
        width: viewportWidth,
        height: viewportHeight,
      },
      items: items.map(node => ({
        id: node.id,
        x: offsetX + (node.x + node.width / 2 - paddedMinX) * scale - 2,
        y: offsetY + (node.y + node.height / 2 - paddedMinY) * scale - 1.5,
        color: (cardColors()[node.card_type] || cardColors().response).border,
      })),
    };
  });

  const contextMenuNode = createMemo(() => {
    const menu = contextMenu();
    return menu ? nodes().find(node => node.id === menu.nodeId) ?? null : null;
  });

  const clusterRegions = createMemo(() => {
    const eligibleNodes = activeNodes();
    const nodeById = new Map(eligibleNodes.map(node => [node.id, node]));
    const regions: Array<{ clusterId: string; label: string; x: number; y: number; width: number; height: number; color: string }> = [];

    for (const cluster of eligibleNodes) {
      if (cluster.card_type !== 'cluster') continue;

      const members = edges()
        .filter(edge => edge.source_id === cluster.id && edge.edge_type === 'context_share')
        .map(edge => nodeById.get(edge.target_id))
        .filter((node): node is CanvasNode => !!node && node.card_type !== 'cluster');

      if (!members.length) continue;

      let minX = Infinity;
      let minY = Infinity;
      let maxX = -Infinity;
      let maxY = -Infinity;

      for (const member of members) {
        minX = Math.min(minX, member.x);
        minY = Math.min(minY, member.y);
        maxX = Math.max(maxX, member.x + member.width);
        maxY = Math.max(maxY, member.y + member.height);
      }

      const label = typeof cluster.content?.text === 'string' && cluster.content.text.trim()
        ? cluster.content.text.trim()
        : 'Cluster';

      regions.push({
        clusterId: cluster.id,
        label,
        x: minX - 40,
        y: minY - 40,
        width: maxX - minX + 80,
        height: maxY - minY + 80,
        color: cardColors().cluster.border,
      });
    }

    return regions;
  });

  createEffect(() => {
    const currentNodes = nodes();
    const allIds = new Set(currentNodes.map(node => node.id));
    const selectableIds = new Set(currentNodes.filter(node => node.status !== 'archived').map(node => node.id));

    setSelectedNodes(prev => {
      const next = new Set([...prev].filter(id => selectableIds.has(id)));
      return setsEqual(prev, next) ? prev : next;
    });

    setExpandedCards(prev => {
      const next = new Set([...prev].filter(id => allIds.has(id)));
      return setsEqual(prev, next) ? prev : next;
    });

    const menu = contextMenu();
    if (menu && !allIds.has(menu.nodeId)) {
      setContextMenu(null);
    }

    for (const id of [...layoutVelocities.keys()]) {
      if (!allIds.has(id)) layoutVelocities.delete(id);
    }

    for (const id of [...pinnedNodes]) {
      if (!allIds.has(id)) pinnedNodes.delete(id);
    }
  });

  // Draw background grid on canvas
  const drawGrid = () => {
    const canvas = canvasRef;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;

    const rect = containerRect();
    if (!rect.width) return;

    const dpr = window.devicePixelRatio || 1;
    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);

    ctx.clearRect(0, 0, rect.width, rect.height);
    ctx.strokeStyle = theme().gridStroke;
    ctx.lineWidth = 1;

    const z = zoom();
    const gridSize = 50 * z;
    if (gridSize < 4) return; // skip grid at extreme zoom-out

    const offsetX = (-centerX() * z + rect.width / 2) % gridSize;
    const offsetY = (-centerY() * z + rect.height / 2) % gridSize;

    ctx.beginPath();
    for (let x = offsetX; x < rect.width; x += gridSize) {
      ctx.moveTo(x, 0);
      ctx.lineTo(x, rect.height);
    }
    for (let y = offsetY; y < rect.height; y += gridSize) {
      ctx.moveTo(0, y);
      ctx.lineTo(rect.width, y);
    }
    ctx.stroke();
  };

  // Redraw grid on viewport / theme change
  createEffect(() => {
    centerX();
    centerY();
    zoom();
    theme();
    drawGrid();
  });

  // Persist viewport and layout on changes (debounced via untrack)
  createEffect(() => {
    const cx = centerX();
    const cy = centerY();
    const z = zoom();
    saveViewport(cx, cy, z);
  });

  createEffect(() => {
    const currentNodes = nodes();
    if (currentNodes.length > 0) {
      saveLayout(currentNodes);
    }
  });

  // SVG transform string for the edge layer
  const svgTransform = createMemo(() => {
    const rect = containerRect();
    const tx = rect.width / 2 - centerX() * zoom();
    const ty = rect.height / 2 - centerY() * zoom();
    return `translate(${tx}, ${ty}) scale(${zoom()})`;
  });

  const handleCanvasClick = (e: MouseEvent) => {
    setContextMenu(null);
    setLinkTypePicker(null);
    setSelectedEdge(null);
    if (linking()) { setLinking(null); return; }
    if (redirecting()) { setRedirecting(null); return; }
    if (isBackgroundTarget(e.target)) {
      clearSelection();
    }
  };

  // Mouse handlers for panning
  const handleMouseDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    if (isBackgroundTarget(e.target)) {
      setContextMenu(null);
      setPanning(true);
      setPanStart({ x: e.clientX, y: e.clientY });
      e.preventDefault();
    }
  };

  const handleMouseMove = (e: MouseEvent) => {
    if (panning()) {
      const z = zoom();
      const dx = e.clientX - panStart().x;
      const dy = e.clientY - panStart().y;
      setCenterX(cx => cx - dx / z);
      setCenterY(cy => cy - dy / z);
      setPanStart({ x: e.clientX, y: e.clientY });
    }
    if (dragging()) {
      const rect = containerRect();
      const pos = screenToCanvas(
        e.clientX - rect.left,
        e.clientY - rect.top
      );
      const offset = dragOffset();
      setNodes(ns => ns.map(n =>
        n.id === dragging()
          ? { ...n, x: pos.x - offset.x, y: pos.y - offset.y }
          : n
      ));
    }
    if (linking()) {
      const rect = containerRect();
      setLinking(prev => prev ? { ...prev, cursorX: e.clientX - rect.left, cursorY: e.clientY - rect.top } : null);
    }
    if (redirecting()) {
      const rect = containerRect();
      setRedirecting(prev => prev ? { ...prev, cursorX: e.clientX - rect.left, cursorY: e.clientY - rect.top } : null);
    }
  };

  const handleMouseUp = (e: MouseEvent) => {
    // Send position update via WebSocket when a card drag ends
    const dragId = dragging();
    if (dragId && wsRef?.readyState === WebSocket.OPEN) {
      const node = nodes().find(n => n.id === dragId);
      if (node) {
        wsRef.send(JSON.stringify({
          type: 'position_update',
          card_id: node.id,
          x: node.x,
          y: node.y,
        }));
      }
    }
    setPanning(false);
    setDragging(null);
  };

  // Zoom with scroll wheel (zoom toward cursor)
  const handleWheel = (e: WheelEvent) => {
    e.preventDefault();
    const delta = e.deltaY > 0 ? 0.9 : 1.1;
    const newZoom = Math.max(0.05, Math.min(3, zoom() * delta));

    const rect = containerRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const canvasBefore = screenToCanvas(mx, my);
    setZoom(newZoom);
    const canvasAfter = screenToCanvas(mx, my);
    setCenterX(cx => cx - (canvasAfter.x - canvasBefore.x));
    setCenterY(cy => cy - (canvasAfter.y - canvasBefore.y));
  };

  // Double-click to create contextual prompt
  const handleDoubleClick = (e: MouseEvent) => {
    if (isBackgroundTarget(e.target)) {
      const rect = containerRect();
      const pos = screenToCanvas(e.clientX - rect.left, e.clientY - rect.top);
      setContextPrompt(pos);
      setContextMenu(null);
      setPromptText('');
      requestAnimationFrame(() => promptInputRef?.focus());
    }
  };

  // Submit contextual prompt
  const submitContextPrompt = () => {
    const pos = contextPrompt();
    const text = promptText().trim();
    if (pos && text) {
      setPositionHints(prev => {
        const next = new Map(prev);
        next.set(text, { x: pos.x - 140, y: pos.y - 30 });
        return next;
      });
      props.onSendMessage(text, pos);
    }
    setContextPrompt(null);
    setPromptText('');
  };

  // Submit global prompt
  const submitGlobalPrompt = () => {
    const text = globalPromptText().trim();
    if (!text) return;
    props.onSendMessage(text);
    setGlobalPromptText('');
  };

  const synthesizeSelectedCards = () => {
    const selected = selectedCanvasNodes();
    if (selected.length < 2) return;
    const combinedText = selected
      .map((node, index) => `[${index + 1}] ${getNodeText(node).trim() || getNodeTitle(node)}`)
      .join('\n\n');
    props.onSendMessage(`Synthesize the following insights into a coherent summary:\n\n${combinedText}`);
    clearSelection();
  };

  const setNodeStatus = (nodeId: string, status: CanvasNode['status']) => {
    setNodes(ns => ns.map(node =>
      node.id === nodeId ? { ...node, status } : node
    ));
    if (status === 'archived') {
      setSelectedNodes(prev => {
        if (!prev.has(nodeId)) return prev;
        const next = new Set(prev);
        next.delete(nodeId);
        return next;
      });
    }
    sendCanvasMessage({ type: 'status_change', node_id: nodeId, status });
    setContextMenu(null);
  };

  const deleteNode = (nodeId: string) => {
    setNodes(ns => ns.filter(node => node.id !== nodeId));
    setEdges(es => es.filter(edge => edge.source_id !== nodeId && edge.target_id !== nodeId));
    setSelectedNodes(prev => {
      if (!prev.has(nodeId)) return prev;
      const next = new Set(prev);
      next.delete(nodeId);
      return next;
    });
    setExpandedCards(prev => {
      if (!prev.has(nodeId)) return prev;
      const next = new Set(prev);
      next.delete(nodeId);
      return next;
    });
    pinnedNodes.delete(nodeId);
    layoutVelocities.delete(nodeId);
    if (dragging() === nodeId) setDragging(null);
    sendCanvasMessage({ type: 'delete_node', node_id: nodeId });
    setContextMenu(null);
  };

  const renameCluster = (clusterId: string) => {
    const cluster = nodes().find(node => node.id === clusterId && node.card_type === 'cluster');
    if (!cluster) return;

    const currentLabel = typeof cluster.content?.text === 'string' ? cluster.content.text : '';
    const nextLabel = window.prompt('Rename cluster', currentLabel);
    if (nextLabel === null) {
      setContextMenu(null);
      return;
    }

    const trimmed = nextLabel.trim();
    if (!trimmed || trimmed === currentLabel.trim()) {
      setContextMenu(null);
      return;
    }

    setNodes(ns => ns.map(node => (
      node.id === clusterId
        ? { ...node, content: { ...node.content, text: trimmed } }
        : node
    )));
    setContextMenu(null);
  };

  const dissolveCluster = (clusterId: string) => {
    deleteNode(clusterId);
  };

  const stopProposalAnimation = () => {
    if (proposalAnimationFrame !== null) {
      cancelAnimationFrame(proposalAnimationFrame);
      proposalAnimationFrame = null;
    }
  };

  const cycleLayoutAlgorithm = () => {
    setLayoutAlgorithm((prev) => {
      if (prev === 'tree') return 'force_directed';
      if (prev === 'force_directed') return 'radial';
      return 'tree';
    });
  };

  const triggerLayoutProposal = async () => {
    setClusterStatus('Calculating layout…');
    try {
      await invoke('propose_layout', { session_id: props.session_id, algorithm: layoutAlgorithm() });
    } catch (error) {
      console.warn('propose_layout failed', error);
      setClusterStatus('Layout proposal failed');
      setTimeout(() => setClusterStatus(null), 3000);
    }
    setContextMenu(null);
  };

  const acceptLayout = () => {
    const proposal = layoutProposal();
    if (!proposal) return;

    stopProposalAnimation();

    const currentNodes = nodes();
    const startPositions = new Map<string, { x: number; y: number }>();
    const targetPositions = new Map<string, { x: number; y: number }>();

    for (const pos of proposal.positions) {
      const node = currentNodes.find((entry) => entry.id === pos.node_id);
      if (node) {
        startPositions.set(pos.node_id, { x: node.x, y: node.y });
        targetPositions.set(pos.node_id, { x: pos.x, y: pos.y });
      }
    }

    setLayoutProposal(null);
    if (!targetPositions.size) return;

    const duration = 500;
    const startTime = performance.now();
    const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

    const animate = (now: number) => {
      const elapsed = now - startTime;
      const rawT = Math.min(elapsed / duration, 1);
      const t = easeOutCubic(rawT);

      setNodes(prev => prev.map(node => {
        const start = startPositions.get(node.id);
        const target = targetPositions.get(node.id);
        if (!start || !target) return node;
        return {
          ...node,
          x: start.x + (target.x - start.x) * t,
          y: start.y + (target.y - start.y) * t,
        };
      }));

      if (rawT < 1) {
        proposalAnimationFrame = requestAnimationFrame(animate);
        return;
      }

      proposalAnimationFrame = null;
      for (const [nodeId, target] of targetPositions) {
        sendCanvasMessage({
          type: 'position_update',
          card_id: nodeId,
          x: target.x,
          y: target.y,
        });
      }
    };

    proposalAnimationFrame = requestAnimationFrame(animate);
  };

  const rejectLayout = () => {
    stopProposalAnimation();
    setLayoutProposal(null);
  };

  const triggerManualRecluster = async () => {
    setClusterStatus('Clustering…');
    try {
      const result: any = await invoke('recluster_canvas', { session_id: props.session_id });
      const count = result?.clusters_created ?? 0;
      setClusterStatus(count > 0 ? `Created ${count} cluster events` : 'No clusters found');
    } catch (error) {
      console.warn('recluster_canvas failed', error);
      setClusterStatus('Clustering failed');
    }
    setContextMenu(null);
    setTimeout(() => setClusterStatus(null), 3000);
  };

  const forkNode = (node: CanvasNode) => {
    const sourceText = getNodeText(node).trim();
    const forkText = sourceText ? `What if... ${sourceText}` : 'What if...';
    setPositionHints(prev => {
      const next = new Map(prev);
      next.set(forkText, { x: node.x, y: node.y });
      return next;
    });
    props.onSendMessage(forkText, { x: node.x + node.width / 2, y: node.y + node.height / 2 });
    setContextMenu(null);
  };

  // Collect all descendant node IDs reachable via outgoing edges
  const getSubtreeIds = (rootId: string): Set<string> => {
    const result = new Set<string>();
    const queue = [rootId];
    while (queue.length > 0) {
      const current = queue.shift()!;
      if (result.has(current)) continue;
      result.add(current);
      for (const edge of edges()) {
        if (edge.source_id === current && !result.has(edge.target_id)) {
          queue.push(edge.target_id);
        }
      }
    }
    return result;
  };

  const pruneSubtree = (nodeId: string) => {
    const subtreeIds = getSubtreeIds(nodeId);
    setNodes(ns => ns.filter(n => !subtreeIds.has(n.id)));
    setEdges(es => es.filter(e => !subtreeIds.has(e.source_id) && !subtreeIds.has(e.target_id)));
    setSelectedNodes(prev => {
      const next = new Set([...prev].filter(id => !subtreeIds.has(id)));
      return setsEqual(prev, next) ? prev : next;
    });
    setExpandedCards(prev => {
      const next = new Set([...prev].filter(id => !subtreeIds.has(id)));
      return setsEqual(prev, next) ? prev : next;
    });
    for (const id of subtreeIds) {
      pinnedNodes.delete(id);
      layoutVelocities.delete(id);
      sendCanvasMessage({ type: 'delete_node', node_id: id });
    }
    if (dragging() && subtreeIds.has(dragging()!)) setDragging(null);
    setContextMenu(null);
  };

  const promoteNode = (node: CanvasNode) => {
    // Center view on the promoted node and remove all incoming edges
    setEdges(es => es.filter(e => e.target_id !== node.id));
    setCenterX(node.x + node.width / 2);
    setCenterY(node.y + node.height / 2);
    setZoom(1);
    sendCanvasMessage({ type: 'promote_node', node_id: node.id });
    setContextMenu(null);
  };

  const LINK_EDGE_TYPES = [
    { type: 'references', label: 'References', icon: () => <Pin size={12} />, color: '#3b82f6' },
    { type: 'contradicts', label: 'Contradicts', icon: () => <Zap size={12} />, color: '#ef4444' },
    { type: 'evolves', label: 'Evolves', icon: () => <RefreshCw size={12} />, color: '#8b5cf6' },
    { type: 'synthesizes', label: 'Synthesizes', icon: () => <Link size={12} />, color: '#10b981' },
    { type: 'blocked_by', label: 'Blocked By', icon: () => <Ban size={12} />, color: '#dc2626' },
    { type: 'context_share', label: 'Context Share', icon: () => <Lightbulb size={12} />, color: '#06b6d4' },
  ];

  const createEdgeLink = (source_id: string, targetId: string, edgeType: string) => {
    const edgeId = `edge-manual-${source_id}-${targetId}-${Date.now()}`;
    const newEdge: CanvasEdge = {
      id: edgeId,
      canvas_id: props.session_id,
      source_id: source_id,
      target_id: targetId,
      edge_type: edgeType,
      metadata: null,
      created_at: Date.now(),
    };
    setEdges(es => [...es, newEdge]);
    sendCanvasMessage({ type: 'edge_created', edge: newEdge });
    setLinkTypePicker(null);
  };

  const toggleExpanded = (nodeId: string) => {
    setExpandedCards(prev => {
      const next = new Set(prev);
      if (next.has(nodeId)) next.delete(nodeId);
      else next.add(nodeId);
      return next;
    });
  };

  const startLinking = (nodeId: string, e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    setContextMenu(null);
    setLinkTypePicker(null);
    setRedirecting(null);
    const rect = containerRect();
    setLinking({ source_id: nodeId, cursorX: e.clientX - rect.left, cursorY: e.clientY - rect.top });
  };

  const handleNodeClick = (nodeId: string, e: MouseEvent) => {
    e.stopPropagation();
    setContextMenu(null);

    const redirect = redirecting();
    if (redirect && redirect.nodeId !== nodeId) {
      setEdges(es => es.filter(edge => edge.target_id !== redirect.nodeId));
      setEdges(es => [...es, {
        id: `edge-redirect-${Date.now()}`,
        canvas_id: props.session_id,
        source_id: nodeId,
        target_id: redirect.nodeId,
        edge_type: 'reply_to',
        metadata: null,
        created_at: Date.now(),
      }]);
      setRedirecting(null);
      return;
    }

    // Complete a link if in linking mode
    const link = linking();
    if (link && link.source_id !== nodeId) {
      const rect = containerRect();
      setLinkTypePicker({ source_id: link.source_id, targetId: nodeId, x: e.clientX - rect.left, y: e.clientY - rect.top });
      setLinking(null);
      return;
    }

    if (e.shiftKey) {
      setSelectedNodes(prev => {
        const next = new Set(prev);
        if (next.has(nodeId)) next.delete(nodeId);
        else next.add(nodeId);
        return next;
      });
      return;
    }
    setSelectedNodes(prev => (prev.size === 1 && prev.has(nodeId) ? prev : new Set([nodeId])));
  };

  const handleNodeContextMenu = (nodeId: string, e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const rect = containerRect();
    if (!selectedNodes().has(nodeId)) {
      setSelectedNodes(new Set([nodeId]));
    }
    setContextMenu({
      x: clamp(e.clientX - rect.left, 8, Math.max(8, rect.width - 168)),
      y: clamp(e.clientY - rect.top, 8, Math.max(8, rect.height - 220)),
      nodeId,
    });
  };

  // Get edge path between two nodes (quadratic bezier)
  const getEdgePath = (edge: CanvasEdge) => {
    const source = nodes().find(n => n.id === edge.source_id);
    const target = nodes().find(n => n.id === edge.target_id);
    if (!source || !target) return '';
    if (!showArchived() && (source.status === 'archived' || target.status === 'archived')) return '';

    const sx = source.x + source.width / 2;
    const sy = source.y + source.height;
    const tx = target.x + target.width / 2;
    const ty = target.y;

    const midY = (sy + ty) / 2;
    return `M ${sx} ${sy} Q ${sx} ${midY}, ${tx} ${ty}`;
  };

  // Zoom level for LOD
  const zoomLevel = createMemo(() => {
    const z = zoom();
    if (z < 0.15) return 'galaxy';
    if (z < 0.4) return 'constellation';
    if (z < 1.2) return 'system';
    return 'surface';
  });

  // ── Sync session messages → canvas nodes ──────────────────────────
  createEffect(() => {
    const msgs = props.messages ?? [];
    const sid = props.session_id;
    const hints = positionHints();
    const currentNodes = untrack(() => nodes());
    const savedPositions = loadSavedLayout();

    const msgNodes: CanvasNode[] = [];
    const msgEdges: CanvasEdge[] = [];
    let autoY = 0;
    let lastUserNodeId: string | null = null;

    for (const msg of msgs) {
      if (msg.role === 'system') continue;
      const nodeId = `msg-${msg.id}`;
      const isUser = msg.role === 'user';
      const isNotification = msg.role === 'notification';

      const existing = currentNodes.find(n => n.id === nodeId);
      const hint = isUser ? hints.get(msg.content) : undefined;
      const saved = savedPositions.get(nodeId);

      const cardType = isUser ? 'prompt' as const
        : isNotification ? 'reference' as const
        : 'response' as const;

      msgNodes.push({
        id: nodeId,
        canvas_id: sid,
        card_type: cardType,
        x: existing?.x ?? saved?.x ?? hint?.x ?? -150,
        y: existing?.y ?? saved?.y ?? hint?.y ?? autoY,
        width: existing?.width ?? (isNotification ? 240 : 280),
        height: existing?.height ?? (isNotification ? 60 : 80),
        content: { text: msg.content, provider_id: msg.provider_id, isNotification },
        status: existing?.status ?? 'active',
        created_by: isUser ? 'user' : isNotification ? 'system' : 'assistant',
        created_at: msg.created_at_ms,
      });

      if (isUser) lastUserNodeId = nodeId;
      else if (lastUserNodeId && !isNotification) {
        msgEdges.push({
          id: `edge-${lastUserNodeId}-${nodeId}`,
          canvas_id: sid,
          source_id: lastUserNodeId,
          target_id: nodeId,
          edge_type: 'reply_to',
          metadata: null,
          created_at: msg.created_at_ms,
        });
        lastUserNodeId = null;
      }

      autoY += 120;
    }

    setNodes(prev => {
      const canvasOnly = prev.filter(n => !n.id.startsWith('msg-') && n.id !== 'streaming-response');
      // Reuse existing object references so <For> preserves DOM (and scroll state)
      const merged = msgNodes.map(newNode => {
        const existing = prev.find(n => n.id === newNode.id);
        if (existing
          && existing.content?.text === newNode.content?.text
          && existing.status === newNode.status
          && existing.x === newNode.x
          && existing.y === newNode.y) {
          return existing;
        }
        return newNode;
      });
      return [...canvasOnly, ...merged];
    });
    setEdges(prev => {
      const canvasOnly = prev.filter(e => !e.id.startsWith('edge-msg-') && e.id !== 'edge-streaming');
      const merged = msgEdges.map(newEdge => {
        const existing = prev.find(e => e.id === newEdge.id);
        return existing || newEdge;
      });
      return [...canvasOnly, ...merged];
    });
  });

  // ── Streaming content → temporary response node ────────────────────
  createEffect(() => {
    const content = props.streamingContent ?? '';
    if (!content) {
      setNodes(prev => prev.filter(n => n.id !== 'streaming-response'));
      setEdges(prev => prev.filter(e => e.id !== 'edge-streaming'));
      return;
    }

    const currentNodes = untrack(() => nodes());
    const msgNodes = currentNodes
      .filter(n => n.id.startsWith('msg-'))
      .sort((a, b) => a.y - b.y);
    const lastNode = msgNodes[msgNodes.length - 1];
    const lastUserNode = [...msgNodes].reverse().find(n => n.card_type === 'prompt');
    const y = lastNode ? lastNode.y + 120 : 0;

    setNodes(prev => {
      const without = prev.filter(n => n.id !== 'streaming-response');
      return [...without, {
        id: 'streaming-response',
        canvas_id: props.session_id,
        card_type: 'response' as const,
        x: -150,
        y,
        width: 280,
        height: 80,
        content: { text: content },
        status: 'active' as const,
        created_by: 'assistant',
        created_at: Date.now(),
      }];
    });

    if (lastUserNode) {
      setEdges(prev => {
        const without = prev.filter(e => e.id !== 'edge-streaming');
        return [...without, {
          id: 'edge-streaming',
          canvas_id: props.session_id,
          source_id: lastUserNode.id,
          target_id: 'streaming-response',
          edge_type: 'reply_to',
          metadata: null,
          created_at: Date.now(),
        }];
      });
    }
  });

  // Handle incoming canvas events from WebSocket
  const handleCanvasEvent = (event: Record<string, unknown>) => {
    switch (event.type) {
      case 'node_created': {
        const newNode = event.node as CanvasNode;
        // Skip if a message-synced node already has the same content
        const existingText = newNode.content?.text;
        if (existingText) {
          const existing = nodes().find(n =>
            n.id !== newNode.id
            && n.content?.text === existingText
            && n.id.startsWith('msg-')
          );
          if (existing) break;
        }
        // Skip if this exact node ID already exists
        if (nodes().some(n => n.id === newNode.id)) break;
        setNodes(ns => [...ns, newNode]);
        break;
      }
      case 'node_updated': {
        const patch = event.patch as Record<string, unknown>;
        setNodes(ns => ns.map(n =>
          n.id === event.node_id
            ? {
                ...n,
                ...(patch.x != null ? { x: patch.x as number } : {}),
                ...(patch.y != null ? { y: patch.y as number } : {}),
                ...(patch.content != null ? { content: patch.content as CanvasNode['content'] } : {}),
                ...(patch.status != null ? { status: patch.status as CanvasNode['status'] } : {}),
              }
            : n
        ));
        break;
      }
      case 'node_status_changed':
        setNodes(ns => ns.map(n =>
          n.id === event.node_id
            ? { ...n, status: event.status as CanvasNode['status'] }
            : n
        ));
        break;
      case 'edge_created':
        setEdges(es => [...es, event.edge as CanvasEdge]);
        break;
      case 'layout_proposal':
        stopProposalAnimation();
        setClusterStatus(null);
        setLayoutProposal({
          proposalId: event.proposal_id as string,
          algorithm: event.algorithm as string,
          positions: event.positions as Array<{ node_id: string; x: number; y: number }>,
          message: event.message as string,
        });
        break;
      case 'stream_token':
        setNodes(ns => ns.map(n =>
          n.id === event.node_id
            ? { ...n, content: { ...n.content, text: (n.content?.text || '') + (event.token as string) } }
            : n
        ));
        break;
    }
  };

  const scheduleReconnect = (session_id: string, delayMs: number) => {
    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
    }
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      if (props.session_id === session_id && !wsRef) {
        void connectWebSocket(session_id);
      }
    }, delayMs);
  };

  // Connect WebSocket for real-time canvas sync
  const connectWebSocket = async (session_id = props.session_id) => {
    try {
      const wsUrl = await invoke<string>('canvas_ws_url', { session_id });
      if (session_id !== props.session_id) return;
      const url = lastSequence > 0 ? `${wsUrl}?last_sequence=${lastSequence}` : wsUrl;
      const ws = new WebSocket(url);

      ws.onmessage = (event) => {
        if (session_id !== props.session_id) return;
        try {
          const msg = JSON.parse(event.data);
          switch (msg.type) {
            case 'welcome':
              console.log('Canvas WS connected, client:', msg.client_id);
              lastSequence = msg.sequence;
              break;
            case 'canvas_event':
              lastSequence = msg.sequence;
              handleCanvasEvent(msg.event);
              break;
            case 'replay':
              (msg.events as Array<{ event: Record<string, unknown>; sequence: number }>).forEach((entry) => {
                lastSequence = Math.max(lastSequence, entry.sequence);
                handleCanvasEvent(entry.event);
              });
              break;
            case 'error':
              console.error('Canvas WS error:', msg.message);
              break;
          }
        } catch (err) {
          console.error('Failed to parse WS message:', err);
        }
      };

      ws.onclose = () => {
        if (wsRef === ws) {
          wsRef = null;
        }
        scheduleReconnect(session_id, 2000);
      };

      ws.onerror = (err) => {
        console.error('Canvas WS error:', err);
      };

      if (session_id !== props.session_id) {
        ws.close();
        return;
      }
      wsRef = ws;
    } catch (e) {
      console.error('WebSocket connection failed:', e);
      scheduleReconnect(session_id, 5000);
    }
  };

  const stopAutoLayoutLoop = () => {
    if (layoutFrame !== null) {
      cancelAnimationFrame(layoutFrame);
      layoutFrame = null;
    }
  };

  const runAutoLayoutStep = () => {
    const currentNodes = nodes().filter(node => node.status !== 'archived');
    if (!currentNodes.length) return;

    const dragId = dragging();
    const isPinned = (nodeId: string) => nodeId === dragId || pinnedNodes.has(nodeId);
    const byId = new Map(currentNodes.map(node => [node.id, node]));
    const forces = new Map(currentNodes.map(node => [node.id, { x: 0, y: 0 }]));

    for (let i = 0; i < currentNodes.length; i += 1) {
      for (let j = i + 1; j < currentNodes.length; j += 1) {
        const a = currentNodes[i];
        const b = currentNodes[j];
        const aCenterX = a.x + a.width / 2;
        const aCenterY = a.y + a.height / 2;
        const bCenterX = b.x + b.width / 2;
        const bCenterY = b.y + b.height / 2;
        let dx = bCenterX - aCenterX;
        let dy = bCenterY - aCenterY;
        let distance = Math.sqrt(dx * dx + dy * dy) || 1;
        if (distance < 0.001) {
          dx = 1;
          dy = 0;
          distance = 1;
        }
        const nx = dx / distance;
        const ny = dy / distance;
        const effectiveDistance = Math.max(distance, 50);
        const force = 5000 / (effectiveDistance * effectiveDistance);

        const forceA = forces.get(a.id)!;
        const forceB = forces.get(b.id)!;
        if (!isPinned(a.id)) {
          forceA.x -= nx * force;
          forceA.y -= ny * force;
        }
        if (!isPinned(b.id)) {
          forceB.x += nx * force;
          forceB.y += ny * force;
        }
      }
    }

    for (const edge of edges()) {
      const source = byId.get(edge.source_id);
      const target = byId.get(edge.target_id);
      if (!source || !target) continue;
      const sourceCenterX = source.x + source.width / 2;
      const sourceCenterY = source.y + source.height / 2;
      const targetCenterX = target.x + target.width / 2;
      const targetCenterY = target.y + target.height / 2;
      const dx = targetCenterX - sourceCenterX;
      const dy = targetCenterY - sourceCenterY;
      const distance = Math.sqrt(dx * dx + dy * dy) || 1;
      const nx = dx / distance;
      const ny = dy / distance;
      const force = 0.01 * (distance - 200);

      const sourceForce = forces.get(source.id)!;
      const targetForce = forces.get(target.id)!;
      if (!isPinned(source.id)) {
        sourceForce.x += nx * force;
        sourceForce.y += ny * force;
      }
      if (!isPinned(target.id)) {
        targetForce.x -= nx * force;
        targetForce.y -= ny * force;
      }
    }

    for (const node of currentNodes) {
      if (isPinned(node.id)) continue;
      const centerNodeX = node.x + node.width / 2;
      const centerNodeY = node.y + node.height / 2;
      const nodeForce = forces.get(node.id)!;
      nodeForce.x += -centerNodeX * 0.001;
      nodeForce.y += -centerNodeY * 0.001;
    }

    setNodes(prev => prev.map(node => {
      if (!forces.has(node.id)) return node;
      if (isPinned(node.id)) {
        layoutVelocities.set(node.id, { x: 0, y: 0 });
        return node;
      }
      const velocity = layoutVelocities.get(node.id) ?? { x: 0, y: 0 };
      const nodeForce = forces.get(node.id)!;
      const nextVelocity = {
        x: clamp((velocity.x + nodeForce.x) * 0.85, -24, 24),
        y: clamp((velocity.y + nodeForce.y) * 0.85, -24, 24),
      };
      layoutVelocities.set(node.id, nextVelocity);
      return {
        ...node,
        x: node.x + nextVelocity.x,
        y: node.y + nextVelocity.y,
      };
    }));
  };

  createEffect(() => {
    if (!autoLayout()) {
      stopAutoLayoutLoop();
      return;
    }

    stopAutoLayoutLoop();
    const tick = () => {
      runAutoLayoutStep();
      layoutFrame = requestAnimationFrame(tick);
    };
    layoutFrame = requestAnimationFrame(tick);

    onCleanup(() => stopAutoLayoutLoop());
  });

  const toggleAutoLayout = () => {
    pinnedNodes.clear();
    layoutVelocities.clear();
    setAutoLayout(enabled => !enabled);
  };

  createEffect(() => {
    const session_id = props.session_id;

    if (reconnectTimer) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    if (wsRef) {
      wsRef.onclose = null;
      wsRef.close();
      wsRef = null;
    }

    void connectWebSocket(session_id);

    onCleanup(() => {
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      if (wsRef) {
        wsRef.onclose = null;
        wsRef.close();
        wsRef = null;
      }
    });
  });

  // Initialize canvas
  onMount(() => {
    const obs = new ResizeObserver(() => drawGrid());
    if (containerRef) obs.observe(containerRef);

    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        setShowSearch(true);
        focusSearchInput();
        return;
      }
      if (e.key === 'Delete' || e.key === 'Backspace') {
        const edgeId = selectedEdge();
        if (edgeId && !(e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement)) {
          e.preventDefault();
          setEdges(es => es.filter(edge => edge.id !== edgeId));
          setSelectedEdge(null);
        }
      }
      if (e.key === 'Escape' && showSearch()) {
        setShowSearch(false);
        setSearchQuery('');
        return;
      }
      if (e.key === 'Escape') {
        setSelectedEdge(null);
      }
    };
    window.addEventListener('keydown', handleKeyDown);

    globalInputRef?.focus();

    onCleanup(() => {
      window.removeEventListener('keydown', handleKeyDown);
      obs.disconnect();
      stopAutoLayoutLoop();
      stopProposalAnimation();
      pinnedNodes.clear();
      layoutVelocities.clear();
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
    });
  });

  // Node drag start handler
  const startNodeDrag = (nodeId: string, e: MouseEvent) => {
    if (e.button !== 0) return;
    if (redirecting()) return;
    e.stopPropagation();
    setContextMenu(null);
    setLinkTypePicker(null);

    const rect = containerRect();
    const pos = screenToCanvas(e.clientX - rect.left, e.clientY - rect.top);
    const node = nodes().find(n => n.id === nodeId);
    if (node) {
      if (autoLayout()) pinnedNodes.add(nodeId);
      setDragOffset({ x: pos.x - node.x, y: pos.y - node.y });
      setDragging(nodeId);
    }
  };

  const handleMinimapClick = (e: MouseEvent) => {
    e.stopPropagation();
    const data = minimapData();
    if (!data) return;
    const rect = (e.currentTarget as HTMLDivElement).getBoundingClientRect();
    const localX = e.clientX - rect.left;
    const localY = e.clientY - rect.top;
    const canvasX = clamp((localX - data.offsetX) / data.scale + data.minX, data.minX, data.minX + data.paddedWidth);
    const canvasY = clamp((localY - data.offsetY) / data.scale + data.minY, data.minY, data.minY + data.paddedHeight);
    setCenterX(canvasX);
    setCenterY(canvasY);
    setContextMenu(null);
  };

  const menuItemStyle = () => ({
    width: '100%',
    padding: '8px 12px',
    background: 'transparent',
    color: theme().textPrimary,
    border: 'none',
    'text-align': 'left' as const,
    cursor: 'pointer',
    'font-size': '13px',
    'font-family': 'inherit',
  });

  return (
    <div
      ref={containerRef}
      style={{
        position: 'relative',
        width: '100%',
        height: '100%',
        overflow: 'hidden',
        background: theme().canvasBg,
        cursor: linking() || redirecting() ? 'crosshair' : panning() ? 'grabbing' : 'grab',
        'user-select': 'none',
      }}
      onClick={handleCanvasClick}
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      onMouseLeave={handleMouseUp}
      onWheel={handleWheel}
      onDblClick={handleDoubleClick}
      onDragOver={handleFileDragOver}
      onDragLeave={handleFileDragLeave}
      onDrop={handleFileDrop}
    >
      {/* CSS for hover-reveal connector dots */}
      <style>{`.spatial-card:hover .connector-dot { opacity: 1 !important; }`}</style>

      {/* Layer 1: Background grid (Canvas2D) */}
      <canvas
        ref={canvasRef}
        style="position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;"
      />

      {/* Layer 2: Edges (SVG) */}
      <svg
        style="position:absolute;top:0;left:0;width:100%;height:100%;pointer-events:none;"
      >
        <defs>
          <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={theme().textMuted} />
          </marker>
          <marker id="arrowhead-red" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
            <polygon points="0 0, 8 3, 0 6" fill={theme().conflictBorder} />
          </marker>
        </defs>
        <g transform={svgTransform()}>
          <For each={visibleEdges()}>
            {(edge) => (
              <g>
                {/* Invisible wider hitbox for clicking */}
                <path
                  d={getEdgePath(edge)}
                  fill="none"
                  stroke="transparent"
                  stroke-width={12}
                  style="pointer-events:stroke;cursor:pointer;"
                  onClick={(e) => { e.stopPropagation(); setSelectedEdge(edge.id); setSelectedNodes(new Set<string>()); setContextMenu(null); }}
                />
                {/* Visible edge */}
                <path
                  d={getEdgePath(edge)}
                  fill="none"
                  stroke={selectedEdge() === edge.id ? theme().linkHighlight : (EDGE_COLORS[edge.edge_type] || theme().textMuted)}
                  stroke-width={selectedEdge() === edge.id ? 3 : (edge.edge_type === 'contradicts' || edge.edge_type === 'blocked_by' ? 2.5 : 2)}
                  stroke-dasharray={
                    edge.edge_type === 'references' ? '6 4'
                    : edge.edge_type === 'feedback_loop' ? '4 2 1 2'
                    : edge.edge_type === 'blocked_by' ? '8 4'
                    : edge.edge_type === 'context_share' ? '2 3'
                    : undefined
                  }
                  marker-end={edge.edge_type === 'contradicts' ? 'url(#arrowhead-red)' : 'url(#arrowhead)'}
                  opacity={edge.edge_type === 'contradicts' ? 0.9 : 1}
                  style="pointer-events:none;"
                />
              </g>
            )}
          </For>
        </g>
        <Show when={linking()}>
          {(link) => {
            const source = () => nodes().find(n => n.id === link().source_id);
            const srcScreen = () => {
              const s = source();
              return s ? canvasToScreen(s.x + s.width / 2, s.y + s.height / 2) : null;
            };
            return (
              <Show when={srcScreen()}>
                {(sp) => (
                  <line
                    x1={sp().x} y1={sp().y}
                    x2={link().cursorX} y2={link().cursorY}
                    stroke={theme().linkHighlight} stroke-width="2" stroke-dasharray="6 4" opacity="0.8"
                  />
                )}
              </Show>
            );
          }}
        </Show>
        <Show when={redirecting()}>
          {(redirect) => {
            const source = () => nodes().find(n => n.id === redirect().nodeId);
            const srcScreen = () => {
              const s = source();
              return s ? canvasToScreen(s.x + s.width / 2, s.y + s.height / 2) : null;
            };
            return (
              <Show when={srcScreen()}>
                {(sp) => (
                  <line
                    x1={sp().x} y1={sp().y}
                    x2={redirect().cursorX} y2={redirect().cursorY}
                    stroke={theme().searchMatchBorder} stroke-width="2" stroke-dasharray="6 4" opacity="0.8"
                  />
                )}
              </Show>
            );
          }}
        </Show>
      </svg>

      {/* Layer 2.5: Cluster regions */}
      <For each={clusterRegions()}>
        {(region) => {
          const screen = () => canvasToScreen(region.x, region.y);
          const z = () => zoom();
          return (
            <>
              <div
                style={{
                  position: 'absolute',
                  left: `${screen().x}px`,
                  top: `${screen().y}px`,
                  width: `${region.width * z()}px`,
                  height: `${region.height * z()}px`,
                  'background-color': theme().clusterFill,
                  border: `1px solid ${region.color}4d`,
                  'border-radius': `${8 * z()}px`,
                  'pointer-events': 'none',
                  'z-index': 0,
                  'box-sizing': 'border-box',
                }}
              />
              <span
                style={{
                  position: 'absolute',
                  left: `${screen().x + 8 * z()}px`,
                  top: `${screen().y + 4 * z()}px`,
                  'font-size': `${Math.max(9, 11 * z())}px`,
                  color: theme().clusterStroke,
                  'font-weight': '500',
                  'white-space': 'nowrap',
                  'pointer-events': 'auto',
                  cursor: 'context-menu',
                  'z-index': 1,
                }}
                onContextMenu={(e) => handleNodeContextMenu(region.clusterId, e)}
              >
                {region.label}
              </span>
            </>
          );
        }}
      </For>

      {/* Layer 3: Cards (DOM) */}
      <For each={visibleCardNodes()}>
        {(node) => {
          const screen = () => canvasToScreen(node.x, node.y);
          const colors = cardColors()[node.card_type] || cardColors().response;
          const cardIcon = CARD_ICONS[node.card_type] || CARD_ICONS.response;
          const isSelected = () => selectedNodes().has(node.id);
          const isDead = () => node.status === 'dead_end';
          const isConflicted = () => conflictedNodes().has(node.id);
          const isExpanded = () => expandedCards().has(node.id);
          const isSearchMatch = () => {
            const matches = searchMatches();
            return matches ? matches.has(node.id) : null;
          };
          const z = () => zoom();

          return (
            <div
              class="spatial-card"
              style={{
                position: 'absolute',
                left: `${screen().x}px`,
                top: `${screen().y}px`,
                width: `${node.width * z()}px`,
                'min-height': `${(isExpanded() ? 72 : 60) * z()}px`,
                background: isDead() ? theme().deadCardBg : colors.bg,
                border: `${isSelected() ? 2 : isConflicted() ? 2 : 1}px solid ${isSelected() ? theme().selectedBorder : isConflicted() ? theme().conflictBorder : isSearchMatch() === true ? theme().searchMatchBorder : colors.border}`,
                'border-style': node.status === 'archived' ? 'dashed' : 'solid',
                'border-radius': `${8 * z()}px`,
                padding: `${8 * z()}px ${12 * z()}px`,
                color: isDead() ? theme().textDead : theme().textPrimary,
                'font-size': `${Math.max(10, (props.chatFontPx ? parseInt(props.chatFontPx()) || 13 : 13) * z())}px`,
                cursor: linking() || redirecting() ? 'crosshair' : dragging() === node.id ? 'grabbing' : 'pointer',
                opacity: isDead() ? 0.5 : (node.status === 'archived' ? 0.4 : (isSearchMatch() === false ? 0.3 : 1)),
                'text-decoration': isDead() ? 'line-through' : 'none',
                'box-shadow': [
                  isSelected() ? `0 0 12px ${theme().selectedGlow}` : '',
                  isConflicted() ? `0 0 10px ${theme().conflictGlow}` : '',
                  isSearchMatch() === true ? `0 0 12px ${theme().searchMatchGlow}` : '',
                ].filter(Boolean).join(', ') || 'none',
                'z-index': isSelected() ? 10 : 1,
                overflow: 'visible',
                'pointer-events': 'auto',
                'box-sizing': 'border-box',
              }}
              onClick={(e) => handleNodeClick(node.id, e)}
              onContextMenu={(e) => handleNodeContextMenu(node.id, e)}
              onMouseDown={(e) => startNodeDrag(node.id, e)}
            >
              <div style="display:flex;align-items:flex-start;justify-content:space-between;gap:8px;">
                <div style="display:flex;align-items:flex-start;gap:6px;min-width:0;flex:1;">
                  <span style="font-size:1.05em;line-height:1;">{cardIcon()}</span>
                  <div style="min-width:0;">
                    <div style="font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                      {getNodeTitle(node)}
                    </div>
                    <div style="text-transform:capitalize;font-size:0.72em;opacity:0.75;">
                      {formatCardType(node.card_type)}
                      <Show when={node.content?.provider_id}>
                        <span style={`margin-left:4px;opacity:0.65;`}>· {node.content.provider_id as string}</span>
                      </Show>
                      <Show when={node.content?.isNotification}>
                        <span style={`margin-left:4px;opacity:0.65;`}>· notification</span>
                      </Show>
                      <Show when={typeof node.content?.text === 'string' && node.content.text.includes('denied by policy')}>
                        <span
                          style={`color:${theme().destructive};margin-left:4px;cursor:pointer;`}
                          title="Tool access denied — click to open settings"
                          onClick={(e) => { e.stopPropagation(); props.onShowSettings?.('security'); }}
                        ><Shield size={11} /></span>
                      </Show>
                      <Show when={isConflicted()}>
                        <span style={`color:${theme().conflictBorder};margin-left:4px;`} title="This card has conflicting information"><TriangleAlert size={12} /></span>
                      </Show>
                    </div>
                  </div>
                </div>
                <button
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleExpanded(node.id);
                  }}
                  style={{
                    width: `${20 * z()}px`,
                    height: `${20 * z()}px`,
                    background: theme().buttonExpandBg,
                    color: theme().textPrimary,
                    border: `1px solid ${colors.border}`,
                    'border-radius': `${5 * z()}px`,
                    cursor: 'pointer',
                    'font-size': `${Math.max(10, 11 * z())}px`,
                    display: 'flex',
                    'align-items': 'center',
                    'justify-content': 'center',
                    padding: '0',
                    'flex-shrink': '0',
                  }}
                  title={isExpanded() ? 'Collapse card' : 'Expand card'}
                >
                  {isExpanded() ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                </button>
              </div>
              <Show when={isExpanded()}>
                <div
                  ref={(el) => {
                    const saved = scrollPositions.get(node.id);
                    if (saved) requestAnimationFrame(() => { el.scrollTop = saved; });
                  }}
                  onScroll={(e) => { scrollPositions.set(node.id, e.currentTarget.scrollTop); }}
                  style={{
                    'font-size': '0.9em',
                    opacity: 0.9,
                    'max-height': `${Math.max(120, 400 * z())}px`,
                    overflow: 'auto',
                    'margin-top': `${6 * z()}px`,
                    'padding-right': `${4 * z()}px`,
                    'word-break': 'break-word',
                    'white-space': 'pre-wrap',
                  }}>
                  {typeof node.content?.text === 'string' && node.content.text.trim()
                    ? (node.card_type === 'response' || node.card_type === 'synthesis'
                      ? <div class="spatial-card-markdown" innerHTML={renderMarkdown(node.content.text)} style="white-space:normal;" />
                      : node.content.text)
                    : <span innerHTML={highlightYaml(node.content)} />}
                </div>
                {/* Copy + tool buttons on expanded card */}
                <div style={`display:flex;align-items:center;gap:${4 * z()}px;margin-top:${4 * z()}px;justify-content:flex-end;`}>
                  <Show when={props.onShowToolCall && node.card_type === 'tool_call' && node.content?.tool_id}>
                    <button
                      onMouseDown={(e) => e.stopPropagation()}
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onShowToolCall?.({
                          id: node.id,
                          tool_id: node.content.tool_id as string,
                          label: node.content.tool_id as string,
                          input: typeof node.content.input === 'string' ? node.content.input : JSON.stringify(node.content.input),
                          output: typeof node.content.output === 'string' ? node.content.output : JSON.stringify(node.content.output),
                          isError: !!node.content.is_error,
                          startedAt: (node.content.started_at as number) || 0,
                          completedAt: node.content.completed_at as number | undefined,
                        });
                      }}
                      style={`background:transparent;border:1px solid ${colors.border};color:${theme().textSecondary};border-radius:${4 * z()}px;padding:${2 * z()}px ${6 * z()}px;cursor:pointer;font-size:${Math.max(9, 10 * z())}px;display:flex;align-items:center;gap:${3 * z()}px;`}
                      title="Inspect tool call"
                    ><Wrench size={10} /> Inspect</button>
                  </Show>
                  <button
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={(e) => {
                      e.stopPropagation();
                      const text = typeof node.content?.text === 'string' ? node.content.text : JSON.stringify(node.content, null, 2);
                      navigator.clipboard.writeText(text);
                      setCopiedCardId(node.id);
                      setTimeout(() => setCopiedCardId((prev) => prev === node.id ? null : prev), 1500);
                    }}
                    style={`background:transparent;border:1px solid ${colors.border};color:${theme().textSecondary};border-radius:${4 * z()}px;padding:${2 * z()}px ${6 * z()}px;cursor:pointer;font-size:${Math.max(9, 10 * z())}px;display:flex;align-items:center;gap:${3 * z()}px;`}
                    title="Copy content"
                  >{copiedCardId() === node.id ? <><Check size={10} /> Copied</> : <><Copy size={10} /> Copy</>}</button>
                </div>
              </Show>
              {/* Connector dot — click to start linking */}
              <div
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => startLinking(node.id, e)}
                style={{
                  position: 'absolute',
                  right: `${-5 * z()}px`,
                  top: '50%',
                  transform: 'translateY(-50%)',
                  width: `${10 * z()}px`,
                  height: `${10 * z()}px`,
                  'border-radius': '50%',
                  background: linking()?.source_id === node.id ? theme().linkHighlight : colors.border,
                  border: `2px solid ${linking()?.source_id === node.id ? theme().linkHighlight : colors.bg}`,
                  cursor: 'crosshair',
                  opacity: linking() ? 1 : 0,
                  transition: 'opacity 0.15s',
                  'z-index': '5',
                }}
                title="Connect to another card"
                class="connector-dot"
              />
            </div>
          );
        }}
      </For>

      <Show when={layoutProposal()}>
        <For each={layoutProposal()!.positions}>
          {(pos) => {
            const node = () => nodes().find((entry) => entry.id === pos.node_id);
            const screen = () => canvasToScreen(pos.x, pos.y);
            const z = () => zoom();
            return (
              <Show when={node()}>
                {(ghostNode) => (
                  <div
                    style={{
                      position: 'absolute',
                      left: `${screen().x}px`,
                      top: `${screen().y}px`,
                      width: `${ghostNode().width * z()}px`,
                      height: `${Math.max(60, ghostNode().height) * z()}px`,
                      border: `2px dashed ${theme().layoutGhostBorder}`,
                      'background-color': theme().layoutGhostBg,
                      'border-radius': `${8 * z()}px`,
                      'pointer-events': 'none',
                      'z-index': 45,
                      transition: 'opacity 0.3s',
                      'box-sizing': 'border-box',
                    }}
                  />
                )}
              </Show>
            );
          }}
        </For>
      </Show>

      {/* Contextual prompt input (appears on double-click) */}
      <Show when={contextPrompt()}>
        {(pos) => {
          const screenPos = () => canvasToScreen(pos().x, pos().y);
          return (
            <div style={{
              position: 'absolute',
              left: `${screenPos().x - 150}px`,
              top: `${screenPos().y}px`,
              width: '300px',
              'z-index': '100',
            }} onMouseDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
              <textarea
                ref={promptInputRef}
                value={promptText()}
                onInput={(e) => setPromptText(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    submitContextPrompt();
                  }
                  if (e.key === 'Escape') {
                    setContextPrompt(null);
                  }
                }}
                placeholder="Ask something here..."
                style={`width:100%;min-height:60px;background:${theme().surfacePrimary};color:${theme().textPrimary};border:2px solid ${theme().accentBlue};border-radius:8px;padding:8px;font-size:13px;resize:vertical;box-sizing:border-box;font-family:inherit;`}
              />
              <div style="display:flex;gap:4px;margin-top:4px;">
                <button
                  onClick={submitContextPrompt}
                  style={`flex:1;padding:4px 8px;background:${theme().accentBlue};color:white;border:none;border-radius:4px;cursor:pointer;font-size:12px;`}
                >Send</button>
                <button
                  onClick={() => setContextPrompt(null)}
                  style={`padding:4px 8px;background:transparent;color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:4px;cursor:pointer;font-size:12px;`}
                >Cancel</button>
              </div>
            </div>
          );
        }}
      </Show>

      <Show when={synthesizeAnchor()}>
        {(anchor) => (
          <button
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              synthesizeSelectedCards();
            }}
            style={{
              position: 'absolute',
              left: `${anchor().x}px`,
              top: `${anchor().y}px`,
              transform: 'translate(-50%, -100%)',
              padding: '10px 14px',
              background: theme().accentGreen,
              color: theme().canvasBg,
              border: 'none',
              'border-radius': '999px',
              'font-size': '13px',
              'font-weight': '600',
              cursor: 'pointer',
              'box-shadow': '0 8px 18px rgba(16,185,129,0.25)',
              'z-index': '90',
            }}
          >
            <Link size={14} />{` Synthesize (${selectionCount()})`}
          </button>
        )}
      </Show>

      <Show when={contextMenu()}>
        {(menu) => (
          <Show when={contextMenuNode()}>
            {(node) => (
              <div
                style={{
                  position: 'absolute',
                  left: `${menu().x}px`,
                  top: `${menu().y}px`,
                  width: '160px',
                  background: theme().surfacePrimary,
                  border: `1px solid ${theme().surfaceBorder}`,
                  'border-radius': '8px',
                  padding: '6px 0',
                  'box-shadow': `0 12px 28px ${theme().surfaceShadow}`,
                  'z-index': '140',
                }}
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => e.stopPropagation()}
              >
                <Show
                  when={node().card_type === 'cluster'}
                  fallback={(
                    <>
                      <button
                        style={menuItemStyle()}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => forkNode(node())}
                      >
                        Fork
                      </button>
                      <button
                        style={menuItemStyle()}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => promoteNode(node())}
                      >
                        Promote to Root
                      </button>
                      <button
                        style={menuItemStyle()}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => {
                          setRedirecting({ nodeId: node().id, cursorX: menu().x, cursorY: menu().y });
                          setContextMenu(null);
                        }}
                      >
                        Redirect
                      </button>
                      <div style={`height:1px;background:${theme().surfaceBorder};margin:4px 0;`} />
                      <Show when={node().status !== 'dead_end'}>
                        <button
                          style={menuItemStyle()}
                          onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                          onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                          onClick={() => setNodeStatus(node().id, 'dead_end')}
                        >
                          Mark as Dead End
                        </button>
                      </Show>
                      <Show when={node().status === 'dead_end'}>
                        <button
                          style={menuItemStyle()}
                          onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                          onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                          onClick={() => setNodeStatus(node().id, 'active')}
                        >
                          Revive
                        </button>
                      </Show>
                      <button
                        style={menuItemStyle()}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => setNodeStatus(node().id, 'archived')}
                      >
                        Archive
                      </button>
                      <div style={`height:1px;background:${theme().surfaceBorder};margin:4px 0;`} />
                      <button
                        style={{ ...menuItemStyle(), color: theme().destructive }}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => pruneSubtree(node().id)}
                      >
                        Prune Subtree
                      </button>
                      <button
                        style={{ ...menuItemStyle(), color: theme().destructive }}
                        onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                        onClick={() => deleteNode(node().id)}
                      >
                        Delete
                      </button>
                    </>
                  )}
                >
                  <button
                    style={menuItemStyle()}
                    onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    onClick={() => renameCluster(node().id)}
                  >
                    <Pencil size={14} /> Rename Cluster
                  </button>
                  <button
                    style={menuItemStyle()}
                    onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    onClick={() => dissolveCluster(node().id)}
                  >
                    <Wind size={14} /> Dissolve Cluster
                  </button>
                  <button
                    style={menuItemStyle()}
                    onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    onClick={() => void triggerManualRecluster()}
                  >
                    <RefreshCw size={14} /> Recluster
                  </button>
                  <div style={`height:1px;background:${theme().surfaceBorder};margin:4px 0;`} />
                  <button
                    style={{ ...menuItemStyle(), color: theme().destructive }}
                    onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                    onClick={() => deleteNode(node().id)}
                  >
                    <Trash2 size={14} /> Delete
                  </button>
                </Show>
              </div>
            )}
          </Show>
        )}
      </Show>

      {/* Link type picker (appears after Alt+drag to a target card) */}
      <Show when={linkTypePicker()}>
        {(picker) => (
          <div
            style={{
              position: 'absolute',
              left: `${picker().x}px`,
              top: `${picker().y}px`,
              width: '180px',
              background: theme().surfacePrimary,
              border: `1px solid ${theme().linkHighlight}`,
              'border-radius': '8px',
              padding: '6px 0',
              'box-shadow': `0 12px 28px ${theme().surfaceShadow}`,
              'z-index': '140',
            }}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={`padding:4px 12px;font-size:11px;color:${theme().textSecondary};font-weight:600;`}>Connect as…</div>
            <For each={LINK_EDGE_TYPES}>
              {(item) => (
                <button
                  style={{ ...menuItemStyle(), display: 'flex', 'align-items': 'center', gap: '8px' }}
                  onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                  onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                  onClick={() => createEdgeLink(picker().source_id, picker().targetId, item.type)}
                >
                  <span style={`display:inline-block;width:8px;height:8px;border-radius:50%;background:${item.color};flex-shrink:0;`} />
                  {item.icon()} {item.label}
                </button>
              )}
            </For>
          </div>
        )}
      </Show>

      {/* HUD: Zoom controls + minimap */}
      <div style="position:absolute;bottom:16px;right:16px;display:flex;flex-direction:column;align-items:flex-end;gap:8px;z-index:50;">
        <Show when={minimapData()}>
          {(data) => (
            <div
              onMouseDown={(e) => e.stopPropagation()}
              onClick={handleMinimapClick}
              style={{
                position: 'relative',
                width: `${data().width}px`,
                height: `${data().height}px`,
                background: theme().minimapBg,
                border: `1px solid ${theme().surfaceBorder}`,
                'border-radius': '8px',
                cursor: 'pointer',
                overflow: 'hidden',
              }}
              title="Click to navigate"
            >
              <For each={data().items}>
                {(item) => (
                  <div
                    style={{
                      position: 'absolute',
                      left: `${item.x}px`,
                      top: `${item.y}px`,
                      width: '4px',
                      height: '3px',
                      background: item.color,
                      'border-radius': '1px',
                      opacity: 0.95,
                    }}
                  />
                )}
              </For>
              <div
                style={{
                  position: 'absolute',
                  left: `${data().viewport.x}px`,
                  top: `${data().viewport.y}px`,
                  width: `${Math.max(8, data().viewport.width)}px`,
                  height: `${Math.max(8, data().viewport.height)}px`,
                  border: `1px solid ${theme().minimapViewportStroke}`,
                  background: theme().minimapViewportFill,
                  'border-radius': '4px',
                  'box-sizing': 'border-box',
                  'pointer-events': 'none',
                }}
              />
            </div>
          )}
        </Show>
        <div style="display:flex;flex-direction:column;gap:4px;">
          <button
            onClick={() => setZoom(z => Math.min(3, z * 1.2))}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:16px;display:flex;align-items:center;justify-content:center;`}
            title="Zoom in"
          >+</button>
          <button
            onClick={() => setZoom(z => Math.max(0.05, z / 1.2))}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:16px;display:flex;align-items:center;justify-content:center;`}
            title="Zoom out"
          >−</button>
          <button
            onClick={toggleAutoLayout}
            style={`width:32px;height:32px;padding:0;background:${autoLayout() ? theme().linkHighlight : theme().surfaceOverlay};color:${autoLayout() ? theme().canvasBg : theme().textPrimary};border:1px solid ${autoLayout() ? theme().linkHighlight : theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:14px;display:flex;align-items:center;justify-content:center;`}
            title={autoLayout() ? 'Disable auto layout' : 'Enable auto layout'}
          ><Zap size={14} /></button>
          <button
            onClick={() => { setShowSearch(v => !v); if (!showSearch()) focusSearchInput(); else setSearchQuery(''); }}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:11px;display:flex;align-items:center;justify-content:center;`}
            title="Search cards (Ctrl+F)"
          ><Search size={14} /></button>
          <button
            onClick={() => void triggerLayoutProposal()}
            onContextMenu={(e) => {
              e.preventDefault();
              cycleLayoutAlgorithm();
            }}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:11px;display:flex;align-items:center;justify-content:center;`}
            title={`Propose layout (${layoutAlgorithm()} ${LAYOUT_ALGORITHM_LABELS[layoutAlgorithm()]}) — right-click to change algorithm`}
          ><Ruler size={14} /></button>
          <button
            onClick={() => void triggerManualRecluster()}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:11px;display:flex;align-items:center;justify-content:center;`}
            title="Trigger reclustering"
          ><Tag size={14} /></button>
          <button
            onClick={() => setShowArchived(v => !v)}
            style={`width:32px;height:32px;padding:0;background:${showArchived() ? theme().searchMatchGlow : theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${showArchived() ? theme().searchMatchBorder : theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:11px;display:flex;align-items:center;justify-content:center;`}
            title="Toggle archived cards"
          ><Package size={14} /></button>
          <button
            onClick={() => { setCenterX(0); setCenterY(0); setZoom(1); }}
            style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;font-size:11px;display:flex;align-items:center;justify-content:center;`}
            title="Reset view"
          >⌂</button>
        </div>
      </div>

      <Show when={showSearch()}>
        <div style="position:absolute;top:8px;left:50%;transform:translateX(-50%);z-index:60;display:flex;align-items:center;gap:8px;">
          <input
            id="spatial-search-input"
            type="text"
            placeholder="Search cards..."
            value={searchQuery()}
            onInput={(e) => setSearchQuery(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === 'Escape') { setShowSearch(false); setSearchQuery(''); } }}
            style={`width:260px;background:${theme().surfaceOverlay95};color:${theme().textPrimary};border:1px solid ${theme().linkHighlight};border-radius:6px;padding:6px 12px;font-size:13px;font-family:inherit;outline:none;`}
          />
          <Show when={searchMatches()}>
            <span style={`font-size:11px;color:${theme().textSecondary};`}>{searchMatches()!.size} match{searchMatches()!.size !== 1 ? 'es' : ''}</span>
          </Show>
          <button
            onClick={() => { setShowSearch(false); setSearchQuery(''); }}
            style={`background:none;border:none;color:${theme().textMuted};cursor:pointer;font-size:16px;padding:2px;`}
          >✕</button>
        </div>
      </Show>

      <Show when={layoutProposal()}>
        <div style={`position:absolute;top:48px;left:50%;transform:translateX(-50%);z-index:60;display:flex;align-items:center;gap:12px;background:${theme().surfaceOverlay95};border:1px solid ${theme().linkHighlight};border-radius:8px;padding:8px 16px;box-shadow:0 4px 12px ${theme().surfaceShadow};`}>
          <span style={`color:${theme().textPrimary};font-size:13px;`}>
            {layoutProposal()!.message || `Suggested ${layoutProposal()!.algorithm} layout`}
          </span>
          <button
            onClick={acceptLayout}
            style={`background:${theme().linkHighlight};color:${theme().surfacePrimary};border:none;border-radius:4px;padding:4px 12px;cursor:pointer;font-size:12px;font-weight:600;`}
          >Accept</button>
          <button
            onClick={rejectLayout}
            style={`background:transparent;color:${theme().destructive};border:1px solid ${theme().destructive};border-radius:4px;padding:4px 12px;cursor:pointer;font-size:12px;`}
          >Reject</button>
        </div>
      </Show>

      {/* HUD: Canvas info */}
      <div style={`position:absolute;top:8px;left:8px;font-size:11px;color:${theme().textMuted};z-index:50;pointer-events:none;`}>
        {visibleCardNodes().length}/{activeCardNodes().length} cards · {clusterCount()} clusters · {Math.round(zoom() * 100)}% · {zoomLevel()}
        <Show when={selectionCount() > 1}> · {selectionCount()} selected</Show>
        <Show when={linking()}> · linking…</Show>
        <Show when={redirecting()}> · redirecting… click target card</Show>
        <Show when={selectedEdge()}> · edge selected (Delete to remove)</Show>
        <Show when={props.daemonOnline && !props.daemonOnline()}> · offline</Show>
      </div>
      {/* Activities / diagnostics HUD */}
      <Show when={props.activities && props.activities()?.length}>
        <div style={`position:absolute;top:24px;left:8px;font-size:10px;color:${theme().textMuted};z-index:50;pointer-events:none;max-width:300px;`}>
          <For each={props.activities!().slice(-3)}>
            {(act) => (
              <div style={`white-space:nowrap;overflow:hidden;text-overflow:ellipsis;opacity:0.7;`}>
                {act.label || act.kind}
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={clusterStatus()}>
        <div style={`position:absolute;top:28px;left:50%;transform:translateX(-50%);z-index:60;background:${theme().surfaceOverlay95};color:${theme().textPrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;padding:6px 16px;font-size:12px;pointer-events:none;`}>
          {clusterStatus()}
        </div>
      </Show>
      <Show when={activeNodes().length > 0 && !linking() && !redirecting()}>
        <div style={`position:absolute;top:8px;right:220px;font-size:10px;color:${theme().textMuted};z-index:50;pointer-events:none;`}>
          Hover card edge to connect
        </div>
      </Show>

      {/* Empty state welcome overlay */}
      <Show when={activeNodes().length === 0}>
        <div style="position:absolute;top:40%;left:50%;transform:translate(-50%,-50%);text-align:center;z-index:20;pointer-events:none;">
          <div style="margin-bottom:16px;"><Brain size={48} /></div>
          <h2 style={`color:${theme().textPrimary};margin:0 0 8px;font-size:20px;`}>Spatial Canvas</h2>
          <p style={`color:${theme().textSecondary};margin:0 0 4px;font-size:14px;`}>Type your message below to get started</p>
          <p style={`color:${theme().textMuted};margin:0;font-size:12px;`}>Or double-click anywhere to place a contextual prompt</p>
        </div>
      </Show>

      {/* File drag overlay */}
      <Show when={isDraggingFile()}>
        <div style={`position:absolute;inset:0;z-index:80;background:${theme().canvasBg}cc;display:flex;align-items:center;justify-content:center;pointer-events:none;`}>
          <div style={`border:3px dashed ${theme().accentBlue};border-radius:16px;padding:32px 48px;text-align:center;`}>
            <Paperclip size={32} style={`color:${theme().accentBlue};margin-bottom:8px;`} />
            <div style={`font-size:16px;font-weight:600;color:${theme().textPrimary};`}>Drop images here</div>
            <div style={`font-size:12px;color:${theme().textSecondary};margin-top:4px;`}>PNG, JPEG, GIF, WebP · max 10 MB</div>
          </div>
        </div>
      </Show>

      {/* Daemon offline banner */}
      <Show when={props.daemonOnline && !props.daemonOnline()}>
        <div style={`position:absolute;top:40px;left:50%;transform:translateX(-50%);z-index:70;background:${theme().destructive};color:white;border-radius:8px;padding:8px 20px;font-size:13px;font-weight:600;display:flex;align-items:center;gap:8px;box-shadow:0 4px 12px ${theme().surfaceShadow};`}>
          <TriangleAlert size={16} /> Daemon offline — messages will be queued
        </div>
      </Show>

      {/* Session controls — interrupt/resume */}
      <Show when={props.activeSessionState}>
        <Show when={props.activeSessionState!() === 'running' || props.activeSessionState!() === 'paused' || props.activeSessionState!() === 'interrupted'}>
          <div style={`position:absolute;top:8px;right:16px;z-index:60;display:flex;align-items:center;gap:6px;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:8px;padding:4px 8px;box-shadow:0 2px 8px ${theme().surfaceShadow};`}>
            <Show when={props.activeSessionState!() === 'running'}>
              <div style={`width:8px;height:8px;border-radius:50%;background:#22c55e;animation:pulse 1.5s infinite;flex-shrink:0;`} />
              <span style={`font-size:12px;color:${theme().textSecondary};margin-right:4px;`}>Running</span>
              <Show when={props.interrupt}>
                <button
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => { e.stopPropagation(); props.interrupt?.('soft'); }}
                  style={`background:transparent;border:1px solid ${theme().surfaceBorder};color:${theme().textPrimary};border-radius:4px;padding:2px 8px;cursor:pointer;font-size:11px;display:flex;align-items:center;gap:4px;`}
                  title="Soft interrupt"
                ><Pause size={12} /> Pause</button>
                <button
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => { e.stopPropagation(); props.interrupt?.('hard'); }}
                  style={`background:transparent;border:1px solid ${theme().destructive};color:${theme().destructive};border-radius:4px;padding:2px 8px;cursor:pointer;font-size:11px;display:flex;align-items:center;gap:4px;`}
                  title="Hard stop"
                ><Square size={12} /> Stop</button>
              </Show>
            </Show>
            <Show when={props.activeSessionState!() === 'paused' || props.activeSessionState!() === 'interrupted'}>
              <div style={`width:8px;height:8px;border-radius:50%;background:#f59e0b;flex-shrink:0;`} />
              <span style={`font-size:12px;color:${theme().textSecondary};margin-right:4px;`}>
                {props.activeSessionState!() === 'paused' ? 'Paused' : 'Interrupted'}
              </span>
              <Show when={props.resume}>
                <button
                  onMouseDown={(e) => e.stopPropagation()}
                  onClick={(e) => { e.stopPropagation(); props.resume?.(); }}
                  style={`background:${theme().accentBlue};border:none;color:white;border-radius:4px;padding:2px 8px;cursor:pointer;font-size:11px;display:flex;align-items:center;gap:4px;`}
                  title="Resume session"
                ><Play size={12} /> Resume</button>
              </Show>
            </Show>
          </div>
        </Show>
      </Show>

      {/* Streaming / thinking indicator */}
      <Show when={props.isStreaming?.() && !props.streamingContent}>
        <div style={`position:absolute;bottom:80px;left:50%;transform:translateX(-50%);z-index:55;display:flex;align-items:center;gap:8px;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:20px;padding:6px 16px;box-shadow:0 2px 8px ${theme().surfaceShadow};`}>
          <Loader2 size={14} class="animate-spin" style={`color:${theme().accentBlue};`} />
          <span style={`font-size:12px;color:${theme().textSecondary};`}>
            {props.activities?.()?.length ? props.activities!()[props.activities!().length - 1].label || 'Thinking...' : 'Thinking...'}
          </span>
        </div>
      </Show>

      {/* Inline questions overlay */}
      <Show when={props.allQuestions && props.allQuestions()?.some(q => !q.answer)}>
        <div style={`position:absolute;bottom:80px;right:16px;z-index:60;max-width:400px;display:flex;flex-direction:column;gap:8px;`}>
          <For each={props.allQuestions!().filter(q => !q.answer)}>
            {(q) => (
              <div style={`background:${theme().surfaceOverlay95};border:1px solid ${theme().accentBlue};border-radius:8px;padding:12px;box-shadow:0 4px 12px ${theme().surfaceShadow};`}>
                <InlineQuestion
                  question={q}
                  session_id={props.session_id}
                  onAnswered={(request_id, answer) => props.onQuestionAnswered?.(request_id, answer)}
                />
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* Prompt injection review overlay */}
      <Show when={props.pendingReview?.()}>
        {(review) => (
          <div style={`position:absolute;top:50%;left:50%;transform:translate(-50%,-50%);z-index:80;background:${theme().surfaceOverlay95};border:2px solid ${theme().destructive};border-radius:12px;padding:20px;max-width:500px;box-shadow:0 8px 24px ${theme().surfaceShadow};`}>
            <div style={`display:flex;align-items:center;gap:8px;margin-bottom:12px;`}>
              <Shield size={20} style={`color:${theme().destructive};`} />
              <span style={`font-weight:600;font-size:15px;color:${theme().textPrimary};`}>Prompt Injection Review</span>
            </div>
            <div style={`font-size:13px;color:${theme().textSecondary};margin-bottom:8px;`}>
              Source: <strong>{review().source}</strong> · Threat: {review().threat_type || 'unknown'} · Confidence: {Math.round(review().confidence * 100)}%
            </div>
            <div style={`background:${theme().surfacePrimary};border:1px solid ${theme().surfaceBorder};border-radius:6px;padding:10px;font-size:12px;color:${theme().textPrimary};max-height:200px;overflow:auto;margin-bottom:12px;white-space:pre-wrap;word-break:break-word;`}>
              {review().preview}
            </div>
            <div style="display:flex;gap:8px;justify-content:flex-end;">
              <button
                onClick={() => props.sendDecision?.({ type: 'scan_decision', decision: 'block' })}
                style={`background:${theme().destructive};color:white;border:none;border-radius:6px;padding:6px 16px;cursor:pointer;font-size:12px;font-weight:600;`}
              >Block</button>
              <button
                onClick={() => props.sendDecision?.({ type: 'scan_decision', decision: 'redact' })}
                style={`background:transparent;border:1px solid ${theme().surfaceBorder};color:${theme().textPrimary};border-radius:6px;padding:6px 16px;cursor:pointer;font-size:12px;`}
              >Redact</button>
              <button
                onClick={() => props.sendDecision?.({ type: 'scan_decision', decision: 'allow' })}
                style={`background:transparent;border:1px solid ${theme().surfaceBorder};color:${theme().textPrimary};border-radius:6px;padding:6px 16px;cursor:pointer;font-size:12px;`}
              >Allow</button>
            </div>
          </div>
        )}
      </Show>

      {/* Workflow status overlay */}
      <Show when={props.activeChatWorkflows && props.activeChatWorkflows()?.length}>
        <div style={`position:absolute;top:40px;left:8px;z-index:55;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:8px;padding:8px;max-width:260px;box-shadow:0 2px 8px ${theme().surfaceShadow};`}>
          <div style={`font-size:11px;font-weight:600;color:${theme().textSecondary};margin-bottom:4px;display:flex;align-items:center;gap:4px;`}>
            <Workflow size={12} /> Active Workflows
          </div>
          <For each={props.activeChatWorkflows!()}>
            {(wf) => (
              <div style={`display:flex;align-items:center;gap:6px;padding:4px 0;font-size:11px;color:${theme().textPrimary};`}>
                <Loader2 size={10} class="animate-spin" style={`color:${theme().accentBlue};flex-shrink:0;`} />
                <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{wf.instance?.name || wf.instanceId}</span>
                <Show when={props.onPauseChatWorkflow}>
                  <button
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={(e) => { e.stopPropagation(); props.onPauseChatWorkflow?.(wf.instanceId); }}
                    style={`background:transparent;border:none;color:${theme().textMuted};cursor:pointer;padding:1px;`}
                    title="Pause workflow"
                  ><Pause size={10} /></button>
                </Show>
                <Show when={props.onKillChatWorkflow}>
                  <button
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={(e) => { e.stopPropagation(); props.onKillChatWorkflow?.(wf.instanceId); }}
                    style={`background:transparent;border:none;color:${theme().destructive};cursor:pointer;padding:1px;`}
                    title="Kill workflow"
                  ><Square size={10} /></button>
                </Show>
              </div>
            )}
          </For>
        </div>
      </Show>

      {/* Terminal workflow results */}
      <Show when={props.terminalChatWorkflows && props.terminalChatWorkflows()?.length}>
        <div style={`position:absolute;top:${props.activeChatWorkflows?.()?.length ? '140' : '40'}px;left:8px;z-index:55;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:8px;padding:8px;max-width:260px;box-shadow:0 2px 8px ${theme().surfaceShadow};`}>
          <div style={`font-size:11px;font-weight:600;color:${theme().textSecondary};margin-bottom:4px;display:flex;align-items:center;gap:4px;`}>
            <Workflow size={12} /> Workflow Results
          </div>
          <For each={props.terminalChatWorkflows!().slice(-5)}>
            {(wf) => {
              const status = wf.instance?.status ?? 'unknown';
              const isSuccess = status === 'completed' || status === 'succeeded';
              return (
                <div style={`display:flex;align-items:center;gap:6px;padding:4px 0;font-size:11px;color:${theme().textPrimary};`}>
                  <span style={`color:${isSuccess ? '#22c55e' : theme().destructive};flex-shrink:0;`}>
                    {isSuccess ? <Check size={10} /> : <TriangleAlert size={10} />}
                  </span>
                  <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{wf.instance?.name || wf.instanceId}</span>
                  <span style={`font-size:9px;color:${theme().textMuted};`}>{status}</span>
                </div>
              );
            }}
          </For>
        </div>
      </Show>

      {/* Global prompt bar at bottom */}
      <div style={`position:absolute;bottom:16px;left:50%;transform:translateX(-50%);width:min(${activeNodes().length === 0 ? '700px' : '600px'},${activeNodes().length === 0 ? '90%' : '80%'});z-index:50;`}>
        {/* Persona/prompt template picker */}
        <Show when={showPersonaPicker() && props.personas && props.personas()?.length}>
          <div style={`margin-bottom:6px;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:8px;padding:6px;max-height:200px;overflow:auto;box-shadow:0 4px 12px ${theme().surfaceShadow};`}>
            <div style={`font-size:11px;color:${theme().textMuted};padding:2px 6px 4px;`}>Prompt Templates</div>
            <For each={props.personas!()}>
              {(persona) => (
                <button
                  onClick={() => {
                    setGlobalPromptText(`/prompt ${persona.name} `);
                    setShowPersonaPicker(false);
                    globalInputRef?.focus();
                  }}
                  style={`display:block;width:100%;text-align:left;background:transparent;border:none;color:${theme().textPrimary};padding:6px 8px;border-radius:4px;cursor:pointer;font-size:12px;`}
                  onMouseEnter={(e) => { e.currentTarget.style.background = theme().surfaceHover; }}
                  onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                >
                  <div style="font-weight:600;">{persona.name}</div>
                  <Show when={persona.description}>
                    <div style={`font-size:10px;color:${theme().textSecondary};margin-top:2px;`}>{persona.description}</div>
                  </Show>
                </button>
              )}
            </For>
          </div>
        </Show>
        {/* Attachment preview strip */}
        <Show when={props.pendingAttachments && props.pendingAttachments()?.length}>
          <div style={`display:flex;gap:6px;margin-bottom:6px;padding:6px 8px;background:${theme().surfaceOverlay95};border:1px solid ${theme().surfaceBorder};border-radius:6px;flex-wrap:wrap;`}>
            <For each={props.pendingAttachments!()}>
              {(att, idx) => (
                <div style={`display:flex;align-items:center;gap:4px;background:${theme().surfacePrimary};border:1px solid ${theme().surfaceBorder};border-radius:4px;padding:2px 8px;font-size:11px;color:${theme().textSecondary};`}>
                  <Paperclip size={10} />
                  <span style="max-width:120px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{att.filename || 'attachment'}</span>
                  <button
                    onClick={() => {
                      const current = props.pendingAttachments!();
                      props.setPendingAttachments?.(current.filter((_, i) => i !== idx()) as any);
                    }}
                    style={`background:none;border:none;color:${theme().textMuted};cursor:pointer;padding:0;font-size:14px;line-height:1;`}
                  >×</button>
                </div>
              )}
            </For>
          </div>
        </Show>
        <div style="display:flex;gap:8px;align-items:center;">
          {/* Action buttons before input */}
          <div style={`display:flex;gap:2px;`}>
            <Show when={props.onUploadFiles}>
              <button
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => { e.stopPropagation(); props.onUploadFiles?.(); }}
                style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;display:flex;align-items:center;justify-content:center;`}
                title="Attach files"
              ><Paperclip size={14} /></button>
            </Show>
            <Show when={props.onShowConfig}>
              <button
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => { e.stopPropagation(); props.onShowConfig?.(); }}
                style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;display:flex;align-items:center;justify-content:center;`}
                title="Session config"
              ><Settings size={14} /></button>
            </Show>
            <Show when={props.onShowWorkflowLauncher}>
              <button
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => { e.stopPropagation(); props.onShowWorkflowLauncher?.(); }}
                style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;display:flex;align-items:center;justify-content:center;`}
                title="Launch workflow"
              ><Workflow size={14} /></button>
            </Show>
            <Show when={props.onShowMemories}>
              <button
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => { e.stopPropagation(); props.onShowMemories?.(); }}
                style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;display:flex;align-items:center;justify-content:center;`}
                title="Memories"
              ><Brain size={14} /></button>
            </Show>
            <Show when={props.onShowPermissions}>
              <button
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => { e.stopPropagation(); props.onShowPermissions?.(); }}
                style={`width:32px;height:32px;padding:0;background:${theme().surfaceOverlay};color:${theme().textSecondary};border:1px solid ${theme().surfaceBorder};border-radius:6px;cursor:pointer;display:flex;align-items:center;justify-content:center;`}
                title="Session permissions"
              ><Shield size={14} /></button>
            </Show>
          </div>
          <input
            ref={globalInputRef}
            type="text"
            value={globalPromptText()}
            onInput={(e) => {
              const val = e.currentTarget.value;
              setGlobalPromptText(val);
              setShowPersonaPicker(val === '/' || val.startsWith('/p'));
            }}
            placeholder={activeNodes().length === 0 ? 'Type your first message to get started...' : props.setPendingAttachments ? 'Ask HiveMind something, or drop/paste images…' : 'Ask HiveMind something...'}
            disabled={props.daemonOnline ? !props.daemonOnline() : false}
            style={`flex:1;background:${theme().surfaceOverlay95};color:${theme().textPrimary};border:${activeNodes().length === 0 ? `2px solid ${theme().accentBlue}` : `1px solid ${theme().surfaceBorder}`};border-radius:8px;padding:${activeNodes().length === 0 ? '12px 18px' : '10px 16px'};font-size:${activeNodes().length === 0 ? '15px' : '14px'};font-family:inherit;${activeNodes().length === 0 ? `box-shadow:0 0 12px ${theme().selectedGlow};` : ''}${props.chatFontPx ? `font-size:${props.chatFontPx()}` : ''}`}
            onPaste={handleInputPaste}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && globalPromptText().trim()) {
                e.preventDefault();
                submitGlobalPrompt();
              }
            }}
          />
          <button
            onClick={submitGlobalPrompt}
            disabled={!globalPromptText().trim() || (props.daemonOnline ? !props.daemonOnline() : false)}
            style={`padding:10px 16px;background:${theme().accentBlue};color:white;border:none;border-radius:8px;cursor:pointer;font-size:14px;white-space:nowrap;`}
          >Send</button>
        </div>
      </div>
    </div>
  );
};

export default SpatialCanvas;
