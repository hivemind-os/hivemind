// WorkflowCanvas.ts — Canvas-based workflow graph renderer
// Pure TypeScript, completely decoupled from SolidJS reactivity.
// All rendering happens in a RAF loop; no DOM elements per node/edge.

// ── Types ──

export interface CanvasNode {
  id: string;
  type: 'trigger' | 'task' | 'control_flow';
  subtype: string;
  x: number;
  y: number;
  config: Record<string, any>;
  outputs: Record<string, string>;
  onError?: { strategy: string; max_retries: number; delay_secs: number } | null;
}

export interface CanvasEdge {
  id: string;
  source: string;
  target: string;
  edgeType: string;
}

export interface GraphCallbacks {
  onNodeClick(nodeId: string, shiftKey: boolean): void;
  onNodeDoubleClick(nodeId: string): void;
  onBackgroundClick(): void;
  onNodesMove(updates: { id: string; x: number; y: number }[]): void;
  onEdgeCreate(source_id: string, targetId: string, edgeType?: string): void;
  onEdgeDoubleClick(edgeId: string): void;
  onSelectionRect(nodeIds: string[]): void;
  onViewChange(panX: number, panY: number, zoom: number): void;
}

type HitResult =
  | { type: 'node'; nodeId: string }
  | { type: 'port'; nodeId: string; portType: 'input' | 'output'; edgeType?: string }
  | { type: 'edge'; edgeId: string }
  | { type: 'background' };

interface DragState {
  nodeId: string;
  offsetX: number;
  offsetY: number;
  movedNodeIds: Set<string>;
  startPositions: Map<string, { x: number; y: number }>;
}

interface ConnectState {
  source_id: string;
  mx: number;
  my: number;
  edgeType?: string;
}

interface SelBoxState {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

// ── Constants ──

const NODE_H = 46;
const NODE_MIN_W = 120;
const PORT_R = 5;
const PORT_HIT_R = 16;
const GRID_SIZE = 20;
const MIN_ZOOM = 0.15;
const MAX_ZOOM = 3;

const ICON_MAP: Record<string, string> = {
  manual: '▶', event: '🔔', call_tool: '🔧', invoke_agent: '🤖', invoke_prompt: '📝',
  feedback_gate: '✋', delay: '⏱️', signal_agent: '📡', launch_workflow: '🔄',
  schedule_task: '📅', branch: '🔀', for_each: '🔁', while: '🔃', end_workflow: '🏁',
};

// ── Theme ──

interface CanvasTheme {
  bg: string;
  gridLine: string;
  gridLineMajor: string;
  text: string;
  textMuted: string;
  accent: string;
  border: string;
  borderSubtle: string;
  card: string;
  // Semantic colors
  green: string;
  red: string;
  amber: string;
  // Node type fills
  nodeTrigger: { bg: string; border: string };
  nodeTask: { bg: string; border: string };
  nodeControl: { bg: string; border: string };
  nodeFallback: { bg: string; border: string };
  // Status
  statusPending: string;
  statusActive: string;
  statusCompleted: string;
  statusFailed: string;
  statusSkipped: string;
  statusWaiting: string;
  // Selection box
  selFill: string;
  selStroke: string;
  // Shadows (for zoom badge, etc.)
  overlayBg: string;
}

function resolveTheme(root: HTMLElement): CanvasTheme {
  const cs = getComputedStyle(root);
  const v = (name: string) => cs.getPropertyValue(name).trim();

  // Detect dark vs light from background lightness
  const bgParts = v('--background').split(/[\s,]+/);
  const bgL = parseFloat(bgParts[2]) || 0;
  const isDark = bgL < 50;

  const hsl = (token: string) => `hsl(${v(token)})`;
  const hslA = (token: string, alpha: number) => `hsl(${v(token)} / ${alpha})`;

  return {
    bg: hsl('--background'),
    gridLine: hslA('--border', 0.25),
    gridLineMajor: hslA('--border', 0.5),
    text: hsl('--foreground'),
    textMuted: hsl('--muted-foreground'),
    accent: hsl('--primary'),
    border: hsl('--border'),
    borderSubtle: hslA('--border', 0.6),
    card: hsl('--card'),
    green: isDark ? '#22c55e' : '#16a34a',
    red: isDark ? '#ef4444' : '#dc2626',
    amber: isDark ? '#f59e0b' : '#d97706',
    nodeTrigger: {
      bg: isDark ? 'hsl(217 40% 16%)' : 'hsl(217 30% 94%)',
      border: isDark ? '#3b82f6' : '#2563eb',
    },
    nodeTask: {
      bg: isDark ? 'hsl(142 30% 14%)' : 'hsl(142 25% 93%)',
      border: isDark ? '#22c55e' : '#16a34a',
    },
    nodeControl: {
      bg: isDark ? 'hsl(38 40% 14%)' : 'hsl(38 30% 93%)',
      border: isDark ? '#f59e0b' : '#d97706',
    },
    nodeFallback: {
      bg: hsl('--card'),
      border: hsl('--border'),
    },
    statusPending: hsl('--muted-foreground'),
    statusActive: hsl('--primary'),
    statusCompleted: isDark ? '#a6e3a1' : '#16a34a',
    statusFailed: isDark ? '#f38ba8' : '#dc2626',
    statusSkipped: isDark ? '#6c7086' : '#9ca3af',
    statusWaiting: isDark ? '#fab387' : '#d97706',
    selFill: hslA('--primary', 0.08),
    selStroke: hslA('--primary', 0.4),
    overlayBg: hslA('--card', 0.9),
  };
}

function statusColor(theme: CanvasTheme, status: string): string {
  switch (status) {
    case 'pending': return theme.statusPending;
    case 'ready': case 'running': return theme.statusActive;
    case 'completed': return theme.statusCompleted;
    case 'failed': return theme.statusFailed;
    case 'skipped': return theme.statusSkipped;
    case 'waiting_on_input': case 'waiting_on_event': return theme.statusWaiting;
    default: return theme.textMuted;
  }
}

function nodeColors(theme: CanvasTheme, type: string): { bg: string; border: string } {
  switch (type) {
    case 'trigger': return theme.nodeTrigger;
    case 'task': return theme.nodeTask;
    case 'control_flow': return theme.nodeControl;
    default: return theme.nodeFallback;
  }
}

// ── Helpers ──

function nodeWidth(id: string): number {
  const textLen = id.length * 7 + 44;
  return Math.max(NODE_MIN_W, Math.min(textLen, 240));
}

function snapToGrid(val: number): number {
  return Math.round(val / GRID_SIZE) * GRID_SIZE;
}

function dist(x1: number, y1: number, x2: number, y2: number): number {
  const dx = x1 - x2;
  const dy = y1 - y2;
  return Math.sqrt(dx * dx + dy * dy);
}

function requiredFields(subtype: string): string[] {
  switch (subtype) {
    case 'call_tool': return ['tool_id'];
    case 'invoke_agent': return ['persona_id', 'task'];
    case 'feedback_gate': return ['prompt'];
    case 'signal_agent': return ['content'];
    case 'launch_workflow': return ['workflow_name'];
    case 'schedule_task': return ['task_name'];
    case 'branch': return ['condition'];
    case 'for_each': return ['collection'];
    case 'while': return ['condition'];
    default: return [];
  }
}

function hasValidationErrors(node: CanvasNode): boolean {
  const fields = requiredFields(node.subtype);
  return fields.some(f => {
    const val = node.config[f];
    return val === undefined || val === null || val === '';
  });
}

function isLoopSubtype(subtype: string): boolean {
  return subtype === 'for_each' || subtype === 'while';
}

function outputPort(node: CanvasNode, portType?: string): { x: number; y: number } {
  const w = nodeWidth(node.id);
  if (node.subtype === 'branch') {
    if (portType === 'else') return { x: node.x + w * 0.75, y: node.y + NODE_H };
    return { x: node.x + w * 0.25, y: node.y + NODE_H };
  }
  if (isLoopSubtype(node.subtype)) {
    if (portType === 'next') return { x: node.x + w * 0.75, y: node.y + NODE_H };
    return { x: node.x + w * 0.25, y: node.y + NODE_H }; // 'body' or default
  }
  return { x: node.x + w / 2, y: node.y + NODE_H };
}

function inputPort(node: CanvasNode): { x: number; y: number } {
  const w = nodeWidth(node.id);
  return { x: node.x + w / 2, y: node.y };
}

function roundedRect(
  ctx: CanvasRenderingContext2D,
  x: number, y: number, w: number, h: number, r: number,
) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.arcTo(x + w, y, x + w, y + r, r);
  ctx.lineTo(x + w, y + h - r);
  ctx.arcTo(x + w, y + h, x + w - r, y + h, r);
  ctx.lineTo(x + r, y + h);
  ctx.arcTo(x, y + h, x, y + h - r, r);
  ctx.lineTo(x, y + r);
  ctx.arcTo(x, y, x + r, y, r);
  ctx.closePath();
}

function truncateText(ctx: CanvasRenderingContext2D, text: string, maxW: number): string {
  if (ctx.measureText(text).width <= maxW) return text;
  let t = text;
  while (t.length > 1 && ctx.measureText(t + '…').width > maxW) t = t.slice(0, -1);
  return t + '…';
}

// ── GraphCanvas Class ──

export class GraphCanvas {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private dpr = 1;
  private cssW = 0;
  private cssH = 0;

  // Theme (resolved from CSS variables)
  private theme: CanvasTheme;
  private themeObserver: MutationObserver;

  // Data (plain arrays — no signals, no subscriptions)
  private nodes: CanvasNode[] = [];
  private edges: CanvasEdge[] = [];
  private nodeMap = new Map<string, CanvasNode>();
  private selectedNodes = new Set<string>();
  private stepStates: Record<string, { status: string; error?: string | null }> = {};
  private readOnly = false;
  private snapEnabled = true;

  // View state
  private panX = 0;
  private panY = 0;
  private zoom = 1;

  // Interaction state (all internal, no signals)
  private dragState: DragState | null = null;
  private connectState: ConnectState | null = null;
  private panState: { startX: number; startY: number } | null = null;
  private selBox: SelBoxState | null = null;
  private hoveredPort: string | null = null;
  private hoveredEdge: string | null = null;
  private pendingNodes: CanvasNode[] | null = null;

  // RAF rendering
  private dirty = true;
  private rafId = 0;
  private runningDashOffset = 0;
  private lastAnimTime = 0;

  // Resize
  private resizeObserver: ResizeObserver;

  // Callbacks
  private cb: GraphCallbacks;

  // Double-click detection
  private lastClickTime = 0;
  private lastClickTarget: string | null = null;

  constructor(container: HTMLElement, callbacks: GraphCallbacks) {
    this.cb = callbacks;
    this.canvas = document.createElement('canvas');
    this.canvas.style.width = '100%';
    this.canvas.style.height = '100%';
    this.canvas.style.display = 'block';
    this.canvas.style.cursor = 'default';
    container.appendChild(this.canvas);
    this.ctx = this.canvas.getContext('2d')!;

    // Resolve theme from CSS variables
    this.theme = resolveTheme(document.documentElement);

    // Watch for theme class changes on <html>
    this.themeObserver = new MutationObserver(() => {
      this.theme = resolveTheme(document.documentElement);
      this.dirty = true;
    });
    this.themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-theme', 'style'] });

    // Bind event handlers
    this.canvas.addEventListener('mousedown', this.onMouseDown);
    this.canvas.addEventListener('mousemove', this.onMouseMove);
    this.canvas.addEventListener('mouseup', this.onMouseUp);
    this.canvas.addEventListener('wheel', this.onWheel, { passive: false });
    this.canvas.addEventListener('contextmenu', this.onContextMenu);

    // Resize observer
    this.resizeObserver = new ResizeObserver(() => this.handleResize());
    this.resizeObserver.observe(container);
    this.handleResize();

    // Start RAF loop
    this.rafId = requestAnimationFrame(this.renderLoop);
  }

  destroy(): void {
    cancelAnimationFrame(this.rafId);
    this.resizeObserver.disconnect();
    this.themeObserver.disconnect();
    this.canvas.removeEventListener('mousedown', this.onMouseDown);
    this.canvas.removeEventListener('mousemove', this.onMouseMove);
    this.canvas.removeEventListener('mouseup', this.onMouseUp);
    this.canvas.removeEventListener('wheel', this.onWheel);
    this.canvas.removeEventListener('contextmenu', this.onContextMenu);
    this.canvas.remove();
  }

  updateTheme(): void {
    this.theme = resolveTheme(document.documentElement);
    this.dirty = true;
  }

  // ── Data setters (called from SolidJS effects) ──

  setNodes(nodes: CanvasNode[]): void {
    // During drag, queue external updates to apply when drag ends
    if (this.dragState) {
      this.pendingNodes = nodes;
      return;
    }
    this.nodes = nodes;
    this.rebuildNodeMap();
    this.dirty = true;
  }

  setEdges(edges: CanvasEdge[]): void {
    this.edges = edges;
    this.dirty = true;
  }

  setSelectedNodes(sel: Set<string>): void {
    this.selectedNodes = sel;
    this.dirty = true;
  }

  setStepStates(states: Record<string, { status: string; error?: string | null }>): void {
    this.stepStates = states;
    this.dirty = true;
  }

  setReadOnly(ro: boolean): void {
    this.readOnly = ro;
  }

  setSnapEnabled(enabled: boolean): void {
    this.snapEnabled = enabled;
  }

  setPanZoom(px: number, py: number, z: number): void {
    this.panX = px;
    this.panY = py;
    this.zoom = z;
    this.dirty = true;
  }

  getZoom(): number { return this.zoom; }
  getPanX(): number { return this.panX; }
  getPanY(): number { return this.panY; }
  getCanvas(): HTMLCanvasElement { return this.canvas; }

  // Convert screen (client) coordinates to graph coordinates
  screenToGraph(clientX: number, clientY: number): { x: number; y: number } {
    const rect = this.canvas.getBoundingClientRect();
    const cx = clientX - rect.left - rect.width / 2;
    const cy = clientY - rect.top - rect.height / 2;
    return { x: cx / this.zoom + this.panX, y: cy / this.zoom + this.panY };
  }

  fitToView(): void {
    if (this.nodes.length === 0) return;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const n of this.nodes) {
      const w = nodeWidth(n.id);
      minX = Math.min(minX, n.x);
      minY = Math.min(minY, n.y);
      maxX = Math.max(maxX, n.x + w);
      maxY = Math.max(maxY, n.y + NODE_H);
    }
    const pad = 40;
    const gw = maxX - minX + pad * 2;
    const gh = maxY - minY + pad * 2;
    const z = Math.min(this.cssW / gw, this.cssH / gh, 2);
    this.zoom = Math.max(MIN_ZOOM, z);
    this.panX = (minX + maxX) / 2;
    this.panY = (minY + maxY) / 2;
    this.dirty = true;
    this.cb.onViewChange(this.panX, this.panY, this.zoom);
  }

  // ── Private helpers ──

  private rebuildNodeMap(): void {
    this.nodeMap.clear();
    for (const n of this.nodes) this.nodeMap.set(n.id, n);
  }

  private markDirty(): void { this.dirty = true; }

  private handleResize(): void {
    const rect = this.canvas.parentElement!.getBoundingClientRect();
    this.dpr = window.devicePixelRatio || 1;
    this.cssW = rect.width;
    this.cssH = rect.height;
    this.canvas.width = rect.width * this.dpr;
    this.canvas.height = rect.height * this.dpr;
    this.canvas.style.width = `${rect.width}px`;
    this.canvas.style.height = `${rect.height}px`;
    this.dirty = true;
  }

  // ── Hit Testing ──

  private hitTest(clientX: number, clientY: number): HitResult {
    const g = this.screenToGraph(clientX, clientY);

    // Check ports first (smallest targets)
    for (let i = this.nodes.length - 1; i >= 0; i--) {
      const node = this.nodes[i];
      const w = nodeWidth(node.id);
      const ip = inputPort(node);
      if (dist(g.x, g.y, ip.x, ip.y) < PORT_HIT_R / this.zoom) {
        return { type: 'port', nodeId: node.id, portType: 'input' };
      }
      if (node.subtype === 'branch') {
        const tp = outputPort(node, 'then');
        if (dist(g.x, g.y, tp.x, tp.y) < PORT_HIT_R / this.zoom) {
          return { type: 'port', nodeId: node.id, portType: 'output', edgeType: 'then' };
        }
        const ep = outputPort(node, 'else');
        if (dist(g.x, g.y, ep.x, ep.y) < PORT_HIT_R / this.zoom) {
          return { type: 'port', nodeId: node.id, portType: 'output', edgeType: 'else' };
        }
      } else if (isLoopSubtype(node.subtype)) {
        const bp = outputPort(node, 'body');
        if (dist(g.x, g.y, bp.x, bp.y) < PORT_HIT_R / this.zoom) {
          return { type: 'port', nodeId: node.id, portType: 'output', edgeType: 'body' };
        }
        const np = outputPort(node, 'next');
        if (dist(g.x, g.y, np.x, np.y) < PORT_HIT_R / this.zoom) {
          return { type: 'port', nodeId: node.id, portType: 'output' };
        }
      } else {
        const op = outputPort(node);
        if (dist(g.x, g.y, op.x, op.y) < PORT_HIT_R / this.zoom) {
          return { type: 'port', nodeId: node.id, portType: 'output' };
        }
      }
    }

    // Check nodes
    for (let i = this.nodes.length - 1; i >= 0; i--) {
      const node = this.nodes[i];
      const w = nodeWidth(node.id);
      if (g.x >= node.x && g.x <= node.x + w && g.y >= node.y && g.y <= node.y + NODE_H) {
        return { type: 'node', nodeId: node.id };
      }
    }

    // Check edges
    for (const edge of this.edges) {
      if (this.hitTestEdge(edge, g.x, g.y)) {
        return { type: 'edge', edgeId: edge.id };
      }
    }

    return { type: 'background' };
  }

  private hitTestEdge(edge: CanvasEdge, gx: number, gy: number): boolean {
    const src = this.nodeMap.get(edge.source);
    const tgt = this.nodeMap.get(edge.target);
    if (!src || !tgt) return false;
    const s = outputPort(src, edge.edgeType === 'then' ? 'then' : edge.edgeType === 'else' ? 'else' : edge.edgeType === 'body' ? 'body' : isLoopSubtype(src.subtype) ? 'next' : undefined);
    const t = inputPort(tgt);
    const dy = Math.abs(t.y - s.y) / 2;
    const cp = Math.max(30, dy);
    const threshold = 8 / this.zoom;
    // Sample bezier at 24 points
    for (let i = 0; i <= 24; i++) {
      const u = i / 24;
      const u1 = 1 - u;
      // Control points: (s.x, s.y), (s.x, s.y+cp), (t.x, t.y-cp), (t.x, t.y)
      const bx = u1*u1*u1*s.x + 3*u1*u1*u*s.x + 3*u1*u*u*t.x + u*u*u*t.x;
      const by = u1*u1*u1*s.y + 3*u1*u1*u*(s.y+cp) + 3*u1*u*u*(t.y-cp) + u*u*u*t.y;
      if (dist(gx, gy, bx, by) < threshold) return true;
    }
    return false;
  }

  // ── Mouse Events ──

  private onMouseDown = (e: MouseEvent): void => {
    if (e.button === 2 || e.button === 1) {
      // Right or middle click: pan
      this.panState = { startX: e.clientX, startY: e.clientY };
      this.canvas.style.cursor = 'grabbing';
      e.preventDefault();
      return;
    }
    if (e.button !== 0) return;

    const hit = this.hitTest(e.clientX, e.clientY);

    if (hit.type === 'port' && hit.portType === 'output' && !this.readOnly) {
      // Start connecting
      const g = this.screenToGraph(e.clientX, e.clientY);
      this.connectState = { source_id: hit.nodeId, mx: g.x, my: g.y, edgeType: hit.edgeType };
      this.canvas.style.cursor = 'crosshair';
      e.preventDefault();
      return;
    }

    if (hit.type === 'node') {
      // Check double-click
      const now = Date.now();
      if (now - this.lastClickTime < 400 && this.lastClickTarget === hit.nodeId) {
        this.cb.onNodeDoubleClick(hit.nodeId);
        this.lastClickTime = 0;
        this.lastClickTarget = null;
        return;
      }
      this.lastClickTime = now;
      this.lastClickTarget = hit.nodeId;

      // Select
      this.cb.onNodeClick(hit.nodeId, e.shiftKey);

      // Start drag (if not read-only)
      if (!this.readOnly) {
        const node = this.nodeMap.get(hit.nodeId);
        if (node) {
          const g = this.screenToGraph(e.clientX, e.clientY);
          const movedIds = new Set(this.selectedNodes);
          if (!movedIds.has(hit.nodeId)) movedIds.clear();
          movedIds.add(hit.nodeId);
          const startPos = new Map<string, { x: number; y: number }>();
          for (const id of movedIds) {
            const n = this.nodeMap.get(id);
            if (n) startPos.set(id, { x: n.x, y: n.y });
          }
          this.dragState = {
            nodeId: hit.nodeId,
            offsetX: g.x - node.x,
            offsetY: g.y - node.y,
            movedNodeIds: movedIds,
            startPositions: startPos,
          };
          this.canvas.style.cursor = 'move';
        }
      }
      e.preventDefault();
      return;
    }

    if (hit.type === 'edge') {
      const now = Date.now();
      if (now - this.lastClickTime < 400 && this.lastClickTarget === hit.edgeId) {
        this.cb.onEdgeDoubleClick(hit.edgeId);
        this.lastClickTime = 0;
        this.lastClickTarget = null;
      } else {
        this.lastClickTime = now;
        this.lastClickTarget = hit.edgeId;
      }
      e.preventDefault();
      return;
    }

    // Background click
    if (e.shiftKey && !this.readOnly) {
      // Start selection box
      const g = this.screenToGraph(e.clientX, e.clientY);
      this.selBox = { x1: g.x, y1: g.y, x2: g.x, y2: g.y };
      this.canvas.style.cursor = 'crosshair';
    } else {
      this.panState = { startX: e.clientX, startY: e.clientY };
      this.canvas.style.cursor = 'grabbing';
      if (!e.shiftKey) this.cb.onBackgroundClick();
    }
    e.preventDefault();
  };

  private onMouseMove = (e: MouseEvent): void => {
    if (this.panState) {
      const dx = (e.clientX - this.panState.startX) / this.zoom;
      const dy = (e.clientY - this.panState.startY) / this.zoom;
      this.panX -= dx;
      this.panY -= dy;
      this.panState.startX = e.clientX;
      this.panState.startY = e.clientY;
      this.dirty = true;
      return;
    }

    if (this.dragState) {
      const g = this.screenToGraph(e.clientX, e.clientY);
      const ds = this.dragState;
      const baseNode = this.nodeMap.get(ds.nodeId);
      if (!baseNode) return;

      let newX = g.x - ds.offsetX;
      let newY = g.y - ds.offsetY;
      if (this.snapEnabled) { newX = snapToGrid(newX); newY = snapToGrid(newY); }

      const dx = newX - baseNode.x;
      const dy = newY - baseNode.y;
      if (dx === 0 && dy === 0) return;

      // Update node positions directly (no signals!)
      for (const node of this.nodes) {
        if (ds.movedNodeIds.has(node.id)) {
          node.x += dx;
          node.y += dy;
        }
      }
      this.rebuildNodeMap();
      this.dirty = true;
      return;
    }

    if (this.connectState) {
      const g = this.screenToGraph(e.clientX, e.clientY);
      this.connectState.mx = g.x;
      this.connectState.my = g.y;

      // Check for port hover
      let found: string | null = null;
      for (const n of this.nodes) {
        if (n.id === this.connectState.source_id) continue;
        const ip = inputPort(n);
        if (dist(g.x, g.y, ip.x, ip.y) < PORT_HIT_R / this.zoom) {
          found = n.id;
          break;
        }
      }
      this.hoveredPort = found;
      this.dirty = true;
      return;
    }

    if (this.selBox) {
      const g = this.screenToGraph(e.clientX, e.clientY);
      this.selBox.x2 = g.x;
      this.selBox.y2 = g.y;
      this.dirty = true;
      return;
    }

    // Hover detection (cursor changes)
    const hit = this.hitTest(e.clientX, e.clientY);
    if (hit.type === 'port') {
      this.canvas.style.cursor = 'crosshair';
      this.hoveredPort = hit.nodeId + ':' + hit.portType + (hit.edgeType ? ':' + hit.edgeType : '');
    } else if (hit.type === 'node') {
      this.canvas.style.cursor = this.readOnly ? 'default' : 'move';
      this.hoveredPort = null;
    } else if (hit.type === 'edge') {
      this.canvas.style.cursor = 'pointer';
      this.hoveredEdge = hit.edgeId;
      this.hoveredPort = null;
    } else {
      this.canvas.style.cursor = 'default';
      this.hoveredPort = null;
      this.hoveredEdge = null;
    }
    this.dirty = true;
  };

  private onMouseUp = (e: MouseEvent): void => {
    if (this.selBox) {
      const x1 = Math.min(this.selBox.x1, this.selBox.x2);
      const y1 = Math.min(this.selBox.y1, this.selBox.y2);
      const x2 = Math.max(this.selBox.x1, this.selBox.x2);
      const y2 = Math.max(this.selBox.y1, this.selBox.y2);
      const ids: string[] = [];
      for (const n of this.nodes) {
        const w = nodeWidth(n.id);
        if (n.x + w > x1 && n.x < x2 && n.y + NODE_H > y1 && n.y < y2) ids.push(n.id);
      }
      this.selBox = null;
      this.canvas.style.cursor = 'default';
      this.dirty = true;
      this.cb.onSelectionRect(ids);
      return;
    }

    if (this.panState) {
      this.panState = null;
      this.canvas.style.cursor = 'default';
      this.cb.onViewChange(this.panX, this.panY, this.zoom);
      return;
    }

    if (this.dragState) {
      const ds = this.dragState;
      const updates: { id: string; x: number; y: number }[] = [];
      for (const id of ds.movedNodeIds) {
        const n = this.nodeMap.get(id);
        if (n) updates.push({ id: n.id, x: n.x, y: n.y });
      }
      this.dragState = null;
      if (this.pendingNodes) {
        // Merge drag positions into pending nodes so they don't snap back
        const dragMap = new Map(updates.map(u => [u.id, u]));
        for (const pn of this.pendingNodes) {
          const dragged = dragMap.get(pn.id);
          if (dragged) { pn.x = dragged.x; pn.y = dragged.y; }
        }
        this.nodes = this.pendingNodes;
        this.pendingNodes = null;
        this.rebuildNodeMap();
        this.dirty = true;
      }
      this.canvas.style.cursor = 'default';
      if (updates.length > 0) this.cb.onNodesMove(updates);
      return;
    }

    if (this.connectState) {
      const g = this.screenToGraph(e.clientX, e.clientY);
      const conn = this.connectState;
      let targetId: string | null = null;
      for (const n of this.nodes) {
        if (n.id === conn.source_id) continue;
        const ip = inputPort(n);
        if (dist(g.x, g.y, ip.x, ip.y) < PORT_HIT_R / this.zoom) {
          targetId = n.id;
          break;
        }
      }
      this.connectState = null;
      this.hoveredPort = null;
      this.canvas.style.cursor = 'default';
      this.dirty = true;
      if (targetId) {
        this.cb.onEdgeCreate(conn.source_id, targetId, conn.edgeType);
      }
      return;
    }
  };

  private onWheel = (e: WheelEvent): void => {
    e.preventDefault();
    const factor = e.deltaY > 0 ? 0.92 : 1.08;
    this.zoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, this.zoom * factor));
    this.dirty = true;
    this.cb.onViewChange(this.panX, this.panY, this.zoom);
  };

  private onContextMenu = (e: MouseEvent): void => {
    e.preventDefault();
  };

  // ── RAF Render Loop ──

  private renderLoop = (time: number): void => {
    // Animate running step dash offset
    if (time - this.lastAnimTime > 40) {
      this.runningDashOffset = (this.runningDashOffset + 1) % 24;
      this.lastAnimTime = time;
      // Only mark dirty if there are running steps
      for (const nodeId in this.stepStates) {
        if (this.stepStates[nodeId]?.status === 'running') { this.dirty = true; break; }
      }
    }

    if (this.dirty) {
      this.dirty = false;
      this.render();
    }
    this.rafId = requestAnimationFrame(this.renderLoop);
  };

  // ── Main Render ──

  private render(): void {
    const ctx = this.ctx;
    const W = this.cssW;
    const H = this.cssH;

    // Set up for retina
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);

    // Clear
    ctx.fillStyle = this.theme.bg;
    ctx.fillRect(0, 0, W, H);

    // Apply camera transform
    ctx.save();
    ctx.translate(W / 2, H / 2);
    ctx.scale(this.zoom, this.zoom);
    ctx.translate(-this.panX, -this.panY);

    this.drawGrid(ctx, W, H);
    this.drawEdges(ctx);
    if (this.connectState) this.drawConnectionLine(ctx);
    if (this.selBox) this.drawSelectionBox(ctx);
    this.drawNodes(ctx);

    ctx.restore();

    // Screen-space overlays
    this.drawZoomIndicator(ctx, W, H);
  }

  // ── Drawing Methods ──

  private drawGrid(ctx: CanvasRenderingContext2D, W: number, H: number): void {
    const hw = W / 2 / this.zoom;
    const hh = H / 2 / this.zoom;
    const left = this.panX - hw - GRID_SIZE;
    const top = this.panY - hh - GRID_SIZE;
    const right = this.panX + hw + GRID_SIZE;
    const bottom = this.panY + hh + GRID_SIZE;

    // Small grid
    ctx.strokeStyle = this.theme.gridLine;
    ctx.lineWidth = 0.5 / this.zoom;
    const sX = Math.floor(left / GRID_SIZE) * GRID_SIZE;
    const sY = Math.floor(top / GRID_SIZE) * GRID_SIZE;
    ctx.beginPath();
    for (let x = sX; x <= right; x += GRID_SIZE) { ctx.moveTo(x, top); ctx.lineTo(x, bottom); }
    for (let y = sY; y <= bottom; y += GRID_SIZE) { ctx.moveTo(left, y); ctx.lineTo(right, y); }
    ctx.stroke();

    // Large grid
    const bigStep = 100;
    ctx.strokeStyle = this.theme.gridLineMajor;
    ctx.lineWidth = 1 / this.zoom;
    const lX = Math.floor(left / bigStep) * bigStep;
    const lY = Math.floor(top / bigStep) * bigStep;
    ctx.beginPath();
    for (let x = lX; x <= right; x += bigStep) { ctx.moveTo(x, top); ctx.lineTo(x, bottom); }
    for (let y = lY; y <= bottom; y += bigStep) { ctx.moveTo(left, y); ctx.lineTo(right, y); }
    ctx.stroke();
  }

  private drawEdges(ctx: CanvasRenderingContext2D): void {
    for (const edge of this.edges) {
      const src = this.nodeMap.get(edge.source);
      const tgt = this.nodeMap.get(edge.target);
      if (!src || !tgt) continue;

      const s = outputPort(src, edge.edgeType === 'then' ? 'then' : edge.edgeType === 'else' ? 'else' : edge.edgeType === 'body' ? 'body' : isLoopSubtype(src.subtype) ? 'next' : undefined);
      const t = inputPort(tgt);
      const dy = Math.abs(t.y - s.y) / 2;
      const cp = Math.max(30, dy);

      // Draw bezier
      ctx.beginPath();
      ctx.moveTo(s.x, s.y);
      ctx.bezierCurveTo(s.x, s.y + cp, t.x, t.y - cp, t.x, t.y);

      // Hit highlight
      const isHovered = this.hoveredEdge === edge.id;
      let color = this.theme.borderSubtle;
      if (edge.edgeType === 'then') color = this.theme.green;
      else if (edge.edgeType === 'else') color = this.theme.red;
      else if (edge.edgeType === 'body') color = this.theme.amber;

      ctx.strokeStyle = color;
      ctx.lineWidth = (isHovered ? 3 : 2) / this.zoom;
      ctx.stroke();

      // Arrowhead at target
      const aSize = 6 / this.zoom;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.moveTo(t.x, t.y);
      ctx.lineTo(t.x - aSize, t.y - aSize * 1.5);
      ctx.lineTo(t.x + aSize, t.y - aSize * 1.5);
      ctx.closePath();
      ctx.fill();

      // Edge type label
      if (edge.edgeType && edge.edgeType !== 'default') {
        const mx = (s.x + t.x) / 2;
        const my = (s.y + t.y) / 2;
        ctx.fillStyle = this.theme.textMuted;
        ctx.font = `${10 / this.zoom}px system-ui, -apple-system, sans-serif`;
        ctx.textAlign = 'center';
        ctx.textBaseline = 'bottom';
        ctx.fillText(edge.edgeType, mx, my - 3 / this.zoom);
      }
    }
  }

  private drawConnectionLine(ctx: CanvasRenderingContext2D): void {
    if (!this.connectState) return;
    const src = this.nodeMap.get(this.connectState.source_id);
    if (!src) return;
    const s = outputPort(src, this.connectState.edgeType);
    const mx = this.connectState.mx;
    const my = this.connectState.my;

    ctx.beginPath();
    const dy = Math.abs(my - s.y) / 2;
    const cp = Math.max(30, dy);
    ctx.moveTo(s.x, s.y);
    ctx.bezierCurveTo(s.x, s.y + cp, mx, my - cp, mx, my);
    ctx.strokeStyle = this.theme.accent;
    ctx.lineWidth = 2 / this.zoom;
    ctx.setLineDash([6 / this.zoom, 3 / this.zoom]);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  private drawSelectionBox(ctx: CanvasRenderingContext2D): void {
    if (!this.selBox) return;
    const x = Math.min(this.selBox.x1, this.selBox.x2);
    const y = Math.min(this.selBox.y1, this.selBox.y2);
    const w = Math.abs(this.selBox.x2 - this.selBox.x1);
    const h = Math.abs(this.selBox.y2 - this.selBox.y1);
    ctx.fillStyle = this.theme.selFill;
    ctx.fillRect(x, y, w, h);
    ctx.strokeStyle = this.theme.selStroke;
    ctx.lineWidth = 1 / this.zoom;
    ctx.strokeRect(x, y, w, h);
  }

  private drawNodes(ctx: CanvasRenderingContext2D): void {
    for (const node of this.nodes) {
      this.drawNode(ctx, node);
    }
  }

  private drawNode(ctx: CanvasRenderingContext2D, node: CanvasNode): void {
    const w = nodeWidth(node.id);
    const selected = this.selectedNodes.has(node.id);
    const cat = nodeColors(this.theme, node.type);
    const stepState = this.stepStates[node.id];

    // Running animation border
    if (stepState?.status === 'running') {
      ctx.save();
      roundedRect(ctx, node.x - 2, node.y - 2, w + 4, NODE_H + 4, 10);
      ctx.strokeStyle = this.theme.accent;
      ctx.lineWidth = 1.5 / this.zoom;
      ctx.setLineDash([8 / this.zoom, 4 / this.zoom]);
      ctx.lineDashOffset = -this.runningDashOffset / this.zoom;
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.restore();
    }

    // Main body
    roundedRect(ctx, node.x, node.y, w, NODE_H, 8);
    ctx.fillStyle = cat.bg;
    ctx.fill();
    ctx.strokeStyle = selected ? this.theme.accent : cat.border;
    ctx.lineWidth = (selected ? 2 : 1) / this.zoom;
    ctx.stroke();

    // Status border overlay (if step state exists and not running)
    if (stepState && stepState.status !== 'running') {
      const sColor = statusColor(this.theme, stepState.status);
      if (sColor) {
        roundedRect(ctx, node.x, node.y, w, NODE_H, 8);
        ctx.strokeStyle = sColor;
        ctx.lineWidth = 2 / this.zoom;
        ctx.stroke();
      }
    }

    // Category stripe
    const stripeW = 4;
    ctx.save();
    ctx.beginPath();
    ctx.moveTo(node.x + 8, node.y);
    ctx.lineTo(node.x + stripeW, node.y);
    ctx.arcTo(node.x, node.y, node.x, node.y + 8, 8);
    ctx.lineTo(node.x, node.y + NODE_H - 8);
    ctx.arcTo(node.x, node.y + NODE_H, node.x + 8, node.y + NODE_H, 8);
    ctx.lineTo(node.x + stripeW, node.y + NODE_H);
    ctx.closePath();
    ctx.fillStyle = cat.border;
    ctx.fill();
    ctx.restore();

    // Icon
    const iconSize = 14 / this.zoom;
    const icon = ICON_MAP[node.subtype] ?? '⬜';
    ctx.font = `${iconSize}px system-ui, -apple-system, sans-serif`;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = this.theme.text;
    ctx.fillText(icon, node.x + 10, node.y + NODE_H / 2);

    // Label
    const labelFontSize = 11 / this.zoom;
    ctx.font = `${labelFontSize}px system-ui, -apple-system, sans-serif`;
    const label = truncateText(ctx, node.id, w - 36);
    ctx.fillStyle = this.theme.text;
    ctx.textAlign = 'left';
    ctx.fillText(label, node.x + 26, node.y + NODE_H / 2);

    // Validation warning
    if (hasValidationErrors(node)) {
      ctx.beginPath();
      ctx.arc(node.x + w - 8, node.y + 8, 5 / this.zoom, 0, Math.PI * 2);
      ctx.fillStyle = this.theme.amber;
      ctx.fill();
      ctx.font = `bold ${8 / this.zoom}px system-ui`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillStyle = this.theme.bg;
      ctx.fillText('!', node.x + w - 8, node.y + 8);
    }

    // Ports
    this.drawPorts(ctx, node, w);
  }

  private drawPorts(ctx: CanvasRenderingContext2D, node: CanvasNode, w: number): void {
    const r = PORT_R / this.zoom;
    const hoverR = PORT_R * 1.6 / this.zoom;

    // Input port (top center)
    if (node.type !== 'trigger') {
      const ip = inputPort(node);
      const isHov = this.hoveredPort === node.id + ':input'
        || (this.connectState && this.hoveredPort === node.id);
      ctx.beginPath();
      ctx.arc(ip.x, ip.y, isHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = isHov ? this.theme.accent : this.theme.border;
      ctx.fill();
      ctx.strokeStyle = this.theme.borderSubtle;
      ctx.lineWidth = 1 / this.zoom;
      ctx.stroke();
    }

    // Output port(s) (bottom)
    if (node.subtype === 'branch') {
      // Then port
      const tp = outputPort(node, 'then');
      const tHov = this.hoveredPort === node.id + ':output:then';
      ctx.beginPath();
      ctx.arc(tp.x, tp.y, tHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = this.theme.green;
      ctx.fill();
      ctx.font = `${8 / this.zoom}px system-ui`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      ctx.fillStyle = this.theme.green;
      ctx.fillText('then', tp.x, tp.y + r + 2 / this.zoom);

      // Else port
      const ep = outputPort(node, 'else');
      const eHov = this.hoveredPort === node.id + ':output:else';
      ctx.beginPath();
      ctx.arc(ep.x, ep.y, eHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = this.theme.red;
      ctx.fill();
      ctx.font = `${8 / this.zoom}px system-ui`;
      ctx.fillStyle = this.theme.red;
      ctx.fillText('else', ep.x, ep.y + r + 2 / this.zoom);
    } else if (isLoopSubtype(node.subtype)) {
      // Body port (left)
      const bp = outputPort(node, 'body');
      const bHov = this.hoveredPort === node.id + ':output:body';
      ctx.beginPath();
      ctx.arc(bp.x, bp.y, bHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = this.theme.amber;
      ctx.fill();
      ctx.font = `${8 / this.zoom}px system-ui`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'top';
      ctx.fillStyle = this.theme.amber;
      ctx.fillText('body', bp.x, bp.y + r + 2 / this.zoom);

      // Next port (right)
      const np = outputPort(node, 'next');
      const nHov = this.hoveredPort === node.id + ':output:next';
      ctx.beginPath();
      ctx.arc(np.x, np.y, nHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = this.theme.accent;
      ctx.fill();
      ctx.font = `${8 / this.zoom}px system-ui`;
      ctx.fillStyle = this.theme.accent;
      ctx.fillText('next', np.x, np.y + r + 2 / this.zoom);
    } else if (node.subtype !== 'end_workflow') {
      const op = outputPort(node);
      const isHov = this.hoveredPort === node.id + ':output';
      ctx.beginPath();
      ctx.arc(op.x, op.y, isHov ? hoverR : r, 0, Math.PI * 2);
      ctx.fillStyle = isHov ? this.theme.accent : this.theme.border;
      ctx.fill();
      ctx.strokeStyle = this.theme.borderSubtle;
      ctx.lineWidth = 1 / this.zoom;
      ctx.stroke();
    }
  }

  private drawZoomIndicator(ctx: CanvasRenderingContext2D, W: number, H: number): void {
    const text = `${Math.round(this.zoom * 100)}%`;
    ctx.font = '11px system-ui, -apple-system, sans-serif';
    ctx.textAlign = 'right';
    ctx.textBaseline = 'bottom';
    const tw = ctx.measureText(text).width;
    const px = W - 12;
    const py = H - 8;
    ctx.fillStyle = this.theme.overlayBg;
    ctx.fillRect(px - tw - 8, py - 16, tw + 16, 20);
    ctx.strokeStyle = this.theme.border;
    ctx.lineWidth = 1;
    ctx.strokeRect(px - tw - 8, py - 16, tw + 16, 20);
    ctx.fillStyle = this.theme.textMuted;
    ctx.fillText(text, px, py);
  }
}
