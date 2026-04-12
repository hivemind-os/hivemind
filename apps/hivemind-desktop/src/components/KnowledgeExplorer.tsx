import { createSignal, createEffect, onCleanup, onMount, For, Show, batch } from 'solid-js';
import type { Accessor } from 'solid-js';
import { PenLine, Search, Trash2, Brain } from 'lucide-solid';
import { authFetch } from '~/lib/authFetch';
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '~/ui/collapsible';
import { invoke } from '@tauri-apps/api/core';
import cytoscape from 'cytoscape';
import type { Core, EventObject, NodeSingular } from 'cytoscape';
import type { KgNode, KgEdge, KgNodeWithEdges, KgStats } from '../types';
import { getThemeFamily } from '../stores/themeStore';

// ── Types ──────────────────────────────────────────────────────────

type Neighborhood = KgNode & { edges: KgEdge[]; neighbors: KgNode[] };
type VectorResult = KgNode & { distance: number };

export interface KnowledgeExplorerProps {
  daemonOnline: Accessor<boolean>;
  daemon_url: Accessor<string | undefined>;
  kgStats: Accessor<KgStats | null>;
  loadKgStats: () => Promise<void>;
}

// ── Color palette ──────────────────────────────────────────────────

type NodeColorSet = Record<string, { bg: string; border: string }>;

const NODE_COLORS_DARK: NodeColorSet = {
  session: { bg: 'hsl(215, 28%, 17%)', border: 'hsl(217, 91%, 60%)' },
  session_message: { bg: 'hsl(150, 25%, 16%)', border: 'hsl(142, 71%, 45%)' },
  workspace_file: { bg: 'hsl(30, 30%, 16%)', border: 'hsl(38, 92%, 50%)' },
  workspace_dir: { bg: 'hsl(270, 30%, 16%)', border: 'hsl(271, 81%, 56%)' },
  memory: { bg: 'hsl(180, 20%, 13%)', border: 'hsl(173, 80%, 40%)' },
  file_chunk: { bg: 'hsl(0, 0%, 16%)', border: 'hsl(220, 9%, 46%)' },
  session_workspace: { bg: 'hsl(0, 25%, 13%)', border: 'hsl(0, 84%, 60%)' },
  session_agent: { bg: 'hsl(210, 30%, 16%)', border: 'hsl(199, 89%, 48%)' },
};

const NODE_COLORS_LIGHT: NodeColorSet = {
  session: { bg: 'hsl(215, 60%, 95%)', border: 'hsl(217, 80%, 50%)' },
  session_message: { bg: 'hsl(150, 50%, 93%)', border: 'hsl(142, 60%, 38%)' },
  workspace_file: { bg: 'hsl(30, 60%, 93%)', border: 'hsl(38, 80%, 42%)' },
  workspace_dir: { bg: 'hsl(270, 50%, 94%)', border: 'hsl(271, 65%, 48%)' },
  memory: { bg: 'hsl(180, 40%, 93%)', border: 'hsl(173, 65%, 34%)' },
  file_chunk: { bg: 'hsl(0, 0%, 93%)', border: 'hsl(220, 9%, 50%)' },
  session_workspace: { bg: 'hsl(0, 50%, 95%)', border: 'hsl(0, 72%, 50%)' },
  session_agent: { bg: 'hsl(210, 55%, 94%)', border: 'hsl(199, 75%, 42%)' },
};

const DEFAULT_NODE_COLOR_DARK = { bg: 'hsl(215, 28%, 17%)', border: 'hsl(215, 16%, 47%)' };
const DEFAULT_NODE_COLOR_LIGHT = { bg: 'hsl(215, 30%, 94%)', border: 'hsl(215, 20%, 58%)' };

const EDGE_COLORS_DARK: Record<string, string> = {
  session_message: 'hsl(142, 71%, 45%)',
  session_workspace: 'hsl(38, 92%, 50%)',
  contains_file: 'hsl(271, 81%, 56%)',
  contains_dir: 'hsl(271, 81%, 56%)',
  file_chunk: 'hsl(220, 9%, 46%)',
  child_of: 'hsl(215, 16%, 47%)',
  session_agent: 'hsl(199, 89%, 48%)',
  related_to: 'hsl(199, 89%, 65%)',
};

const EDGE_COLORS_LIGHT: Record<string, string> = {
  session_message: 'hsl(142, 55%, 35%)',
  session_workspace: 'hsl(38, 70%, 40%)',
  contains_file: 'hsl(271, 60%, 45%)',
  contains_dir: 'hsl(271, 60%, 45%)',
  file_chunk: 'hsl(220, 12%, 50%)',
  child_of: 'hsl(215, 18%, 52%)',
  session_agent: 'hsl(199, 70%, 40%)',
  related_to: 'hsl(199, 65%, 42%)',
};

const DEFAULT_EDGE_COLOR_DARK = 'hsl(215, 20%, 35%)';
const DEFAULT_EDGE_COLOR_LIGHT = 'hsl(215, 20%, 65%)';

function nodeColor(type: string, family: 'dark' | 'light') {
  const palette = family === 'light' ? NODE_COLORS_LIGHT : NODE_COLORS_DARK;
  const fallback = family === 'light' ? DEFAULT_NODE_COLOR_LIGHT : DEFAULT_NODE_COLOR_DARK;
  return palette[type] ?? fallback;
}
function edgeColor(type: string, family: 'dark' | 'light') {
  const palette = family === 'light' ? EDGE_COLORS_LIGHT : EDGE_COLORS_DARK;
  const fallback = family === 'light' ? DEFAULT_EDGE_COLOR_LIGHT : DEFAULT_EDGE_COLOR_DARK;
  return palette[type] ?? fallback;
}

// Cytoscape graph styling — theme-dependent values that can't use CSS variables
// Uses comma-separated hsl() for Cytoscape's CSS parser compatibility
function cyGraphStyles(family: 'dark' | 'light') {
  const labelColor = family === 'light' ? 'hsl(222, 47%, 11%)' : 'hsl(226, 64%, 88%)';
  const edgeLabelColor = family === 'light' ? 'hsl(215, 25%, 35%)' : 'hsl(215, 25%, 63%)';
  const edgeLabelBg = family === 'light' ? 'hsl(0, 0%, 97%)' : 'hsl(222, 47%, 11%)';
  const selectionBorder = family === 'light' ? 'hsl(199, 75%, 42%)' : 'hsl(199, 89%, 65%)';
  return { labelColor, edgeLabelColor, edgeLabelBg, selectionBorder };
}

// ── Component ──────────────────────────────────────────────────────

const KnowledgeExplorer = (props: KnowledgeExplorerProps) => {
  let cyRef: Core | undefined;
  let cyContainer: HTMLDivElement | undefined;
  let layoutTimer: ReturnType<typeof setTimeout> | undefined;

  // Track what's in the graph and expanded
  const [expandedIds, setExpandedIds] = createSignal<Set<number>>(new Set());
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  const [selectedDetail, setSelectedDetail] = createSignal<KgNodeWithEdges | null>(null);
  const [graphEmpty, setGraphEmpty] = createSignal(true);

  // Search state
  const [searchMode, setSearchMode] = createSignal<'fts' | 'vector'>('fts');
  const [searchQuery, setSearchQuery] = createSignal('');
  const [searchResults, setSearchResults] = createSignal<(KgNode & { distance?: number })[]>([]);
  const [searching, setSearching] = createSignal(false);

  // Embedding models for vector search
  const [embeddingModels, setEmbeddingModels] = createSignal<{ modelId: string; dimensions: number }[]>([]);
  const [selectedModel, setSelectedModel] = createSignal<string>('');

  // Filters
  const [typeFilters, setTypeFilters] = createSignal<Set<string>>(new Set());
  const [showFilters, setShowFilters] = createSignal(false);
  const [allNodeTypes, setAllNodeTypes] = createSignal<string[]>([]);

  // ── Cytoscape initialization ─────────────────────────────────────

  onMount(() => {
    const family = getThemeFamily();
    const gs = cyGraphStyles(family);
    cyRef = cytoscape({
      container: cyContainer,
      style: [
        {
          selector: 'node',
          style: {
            label: 'data(label)',
            'text-valign': 'center',
            'text-halign': 'center',
            'font-size': '10px',
            color: gs.labelColor,
            'text-outline-width': 2,
            'text-outline-color': 'data(bgColor)',
            'background-color': 'data(bgColor)',
            'border-width': 2,
            'border-color': 'data(borderColor)',
            width: 'label',
            height: 30,
            shape: 'round-rectangle',
            'padding-left': '12px',
            'padding-right': '12px',
            'text-max-width': '140px',
            'text-overflow-wrap': 'ellipsis' as any,
          },
        },
        {
          selector: 'node:selected',
          style: {
            'border-width': 3,
            'border-color': gs.selectionBorder,
            'background-color': 'data(bgColor)',
          },
        },
        {
          selector: 'node.expanded',
          style: {
            'border-style': 'double',
            'border-width': 4,
          },
        },
        {
          selector: 'node:active',
          style: {
            'overlay-opacity': 0.08,
          },
        },
        {
          selector: 'edge',
          style: {
            width: 1.5,
            'line-color': 'data(color)',
            'target-arrow-color': 'data(color)',
            'target-arrow-shape': 'triangle',
            'curve-style': 'bezier',
            'arrow-scale': 0.8,
            opacity: 0.6,
            label: '',
          },
        },
        {
          selector: 'edge:selected',
          style: {
            width: 3,
            opacity: 1,
            label: 'data(edgeType)',
            'font-size': '9px',
            color: gs.edgeLabelColor,
            'text-background-color': gs.edgeLabelBg,
            'text-background-opacity': 0.85,
            'text-background-padding': '2px',
          },
        },
        {
          selector: 'node.filtered',
          style: {
            display: 'none',
          },
        },
        {
          selector: 'edge.filtered',
          style: {
            display: 'none',
          },
        },
      ],
      layout: { name: 'preset' },
      wheelSensitivity: 0.3,
      minZoom: 0.1,
      maxZoom: 5,
    });

    // Event: node tap → select + fetch detail
    cyRef.on('tap', 'node', (evt: EventObject) => {
      const nodeId = parseInt(evt.target.id(), 10);
      setSelectedId(nodeId);
      void fetchNodeDetail(nodeId);
    });

    // Event: double-tap node → expand neighbors
    cyRef.on('dbltap', 'node', (evt: EventObject) => {
      const nodeId = parseInt(evt.target.id(), 10);
      void fetchNeighbors(nodeId);
    });

    // Event: tap background → deselect
    cyRef.on('tap', (evt: EventObject) => {
      if (evt.target === cyRef) {
        setSelectedId(null);
        setSelectedDetail(null);
      }
    });

    // Edge hover → show label
    cyRef.on('mouseover', 'edge', (evt: EventObject) => {
      evt.target.select();
    });
    cyRef.on('mouseout', 'edge', (evt: EventObject) => {
      evt.target.unselect();
    });

    // Node hover → hint that double-click expands
    cyRef.on('mouseover', 'node', (evt: EventObject) => {
      const node = evt.target as NodeSingular;
      if (!node.hasClass('expanded')) {
        (cyContainer as HTMLDivElement).style.cursor = 'pointer';
        node.scratch('_tipBackup', node.style('label'));
      }
    });
    cyRef.on('mouseout', 'node', () => {
      (cyContainer as HTMLDivElement).style.cursor = '';
    });

    // Resize the Cytoscape canvas when the container size changes
    const resizeObs = new ResizeObserver(() => cyRef?.resize());
    if (cyContainer) resizeObs.observe(cyContainer);
    onCleanup(() => resizeObs.disconnect());
  });

  onCleanup(() => {
    if (layoutTimer) clearTimeout(layoutTimer);
    cyRef?.destroy();
  });

  // Re-apply graph styles + node/edge data colors when theme changes
  createEffect(() => {
    const family = getThemeFamily();
    if (!cyRef) return;
    const gs = cyGraphStyles(family);
    cyRef.style()
      .selector('node').style({ color: gs.labelColor })
      .selector('node:selected').style({ 'border-color': gs.selectionBorder })
      .selector('edge:selected').style({ color: gs.edgeLabelColor, 'text-background-color': gs.edgeLabelBg })
      .update();
    // Update each node's data-driven colors
    cyRef.nodes().forEach((n) => {
      const colors = nodeColor(n.data('node_type'), family);
      n.data({ bgColor: colors.bg, borderColor: colors.border });
    });
    // Update each edge's data-driven color
    cyRef.edges().forEach((e) => {
      e.data({ color: edgeColor(e.data('edgeType'), family) });
    });
  });

  // ── Load embedding models when vector mode is selected ──────────
  createEffect(() => {
    if (searchMode() === 'vector' && embeddingModels().length === 0) {
      invoke<{ modelId: string; dimensions: number }[]>('kg_list_embedding_models')
        .then((models) => {
          setEmbeddingModels(models);
          if (models.length > 0 && !selectedModel()) {
            setSelectedModel(models[0].modelId);
          }
        })
        .catch(e => console.error('Failed to load embedding models:', e));
    }
  });

  // ── Update node type list when graph changes ─────────────────────

  const refreshNodeTypes = () => {
    if (!cyRef) return;
    const types = new Set<string>();
    cyRef.nodes().forEach((n: NodeSingular) => {
      types.add(n.data('node_type') as string);
    });
    setAllNodeTypes(Array.from(types).sort());
    setGraphEmpty(cyRef.nodes().length === 0);
  };

  // ── Apply type filters to cytoscape ──────────────────────────────

  createEffect(() => {
    if (!cyRef) return;
    const filters = typeFilters();
    cyRef.nodes().forEach((n: NodeSingular) => {
      const node_type = n.data('node_type') as string;
      if (filters.size > 0 && !filters.has(node_type)) {
        n.addClass('filtered');
      } else {
        n.removeClass('filtered');
      }
    });
    // Hide edges where either endpoint is filtered
    cyRef.edges().forEach((e) => {
      const src = e.source();
      const tgt = e.target();
      if (src.hasClass('filtered') || tgt.hasClass('filtered')) {
        e.addClass('filtered');
      } else {
        e.removeClass('filtered');
      }
    });
  });

  // ── Graph manipulation ───────────────────────────────────────────

  const addNodesToGraph = (newNodes: KgNode[]) => {
    if (!cyRef) return;
    const added: cytoscape.ElementDefinition[] = [];
    for (const n of newNodes) {
      const id = String(n.id);
      if (cyRef.getElementById(id).length > 0) continue;
      const colors = nodeColor(n.node_type, getThemeFamily());
      added.push({
        data: {
          id,
          label: n.name.length > 22 ? n.name.slice(0, 20) + '…' : n.name,
          fullName: n.name,
          node_type: n.node_type,
          data_class: n.data_class,
          content: n.content,
          bgColor: colors.bg,
          borderColor: colors.border,
        },
      });
    }
    if (added.length > 0) {
      cyRef.add(added);
      refreshNodeTypes();
    }
  };

  const addEdgesToGraph = (newEdges: KgEdge[]) => {
    if (!cyRef) return;
    const added: cytoscape.ElementDefinition[] = [];
    for (const e of newEdges) {
      const id = `e${e.id}`;
      if (cyRef.getElementById(id).length > 0) continue;
      // Only add edge if both endpoints are in the graph
      if (cyRef.getElementById(String(e.source_id)).length === 0) continue;
      if (cyRef.getElementById(String(e.target_id)).length === 0) continue;
      added.push({
        data: {
          id,
          source: String(e.source_id),
          target: String(e.target_id),
          edgeType: e.edge_type,
          weight: e.weight,
          color: edgeColor(e.edge_type, getThemeFamily()),
          kgEdgeId: e.id,
        },
      });
    }
    if (added.length > 0) {
      cyRef.add(added);
    }
  };

  const runLayout = (centerNodeId?: string) => {
    if (!cyRef || cyRef.nodes().length === 0) return;
    const layoutOpts: cytoscape.LayoutOptions = {
      name: 'cose',
      animate: true,
      animationDuration: 600,
      nodeRepulsion: () => 8000,
      idealEdgeLength: () => 120,
      gravity: 0.25,
      numIter: 200,
      randomize: cyRef.nodes().length <= 3,
      nodeDimensionsIncludeLabels: true,
    } as any;

    const layout = cyRef.layout(layoutOpts);
    layout.run();

    if (centerNodeId) {
      // Center on the node after layout finishes
      layoutTimer = setTimeout(() => {
        const node = cyRef?.getElementById(centerNodeId);
        if (node && node.length > 0) {
          cyRef?.animate({ center: { eles: node }, duration: 300 });
        }
      }, 700);
    }
  };

  // ── API calls ────────────────────────────────────────────────────

  const inFlightExpansions = new Set<number>();

  const fetchNeighbors = async (nodeId: number) => {
    if (expandedIds().has(nodeId) || inFlightExpansions.has(nodeId)) return;
    inFlightExpansions.add(nodeId);
    try {
      const resp = await invoke<Neighborhood>('kg_get_neighbors', { node_id: nodeId, limit: 20 });
      addNodesToGraph(resp.neighbors);
      addEdgesToGraph(resp.edges);
      setExpandedIds((prev) => { const next = new Set(prev); next.add(nodeId); return next; });

      // Mark the node as expanded visually
      const cyNode = cyRef?.getElementById(String(nodeId));
      cyNode?.addClass('expanded');

      runLayout(String(nodeId));
    } catch (e) {
      console.error('Failed to fetch neighbors:', e);
    } finally {
      inFlightExpansions.delete(nodeId);
    }
  };

  const fetchNodeDetail = async (nodeId: number) => {
    const base = props.daemon_url();
    if (!base) return;
    try {
      const resp = await authFetch(`${base}/api/v1/knowledge/nodes/${nodeId}`);
      if (!resp.ok) return;
      const detail = await resp.json() as KgNodeWithEdges;
      setSelectedDetail(detail);
    } catch {
      // Fallback: build from cytoscape data
      const cyNode = cyRef?.getElementById(String(nodeId));
      if (cyNode && cyNode.length > 0) {
        const d = cyNode.data();
        setSelectedDetail({
          id: nodeId,
          node_type: d.node_type,
          name: d.fullName,
          data_class: d.data_class,
          content: d.content,
          edges: [],
        } as KgNodeWithEdges);
      }
    }
  };

  let searchSeq = 0;

  const doSearch = async () => {
    const mySeq = ++searchSeq;
    const q = searchQuery().trim();
    if (!q || !props.daemonOnline()) { setSearchResults([]); return; }
    const base = props.daemon_url();
    if (!base) return;
    setSearching(true);
    try {
      if (searchMode() === 'vector') {
        const model = selectedModel() || undefined;
        const results = await invoke<VectorResult[]>('kg_vector_search', { q, limit: 20, model });
        if (mySeq !== searchSeq) return;
        setSearchResults(results);
      } else {
        const resp = await authFetch(`${base}/api/v1/knowledge/search?${new URLSearchParams({ q, limit: '20' })}`);
        if (!resp.ok) throw new Error(`Search failed: ${resp.status} ${resp.statusText}`);
        const results = await resp.json() as KgNode[];
        if (mySeq !== searchSeq) return;
        setSearchResults(results);
      }
    } catch (e) {
      if (mySeq !== searchSeq) return;
      console.error('Search failed:', e);
      setSearchResults([]);
    } finally {
      if (mySeq === searchSeq) setSearching(false);
    }
  };

  const addSearchResultToGraph = (node: KgNode) => {
    addNodesToGraph([node]);
    runLayout(String(node.id));

    // Select and center on the node
    const cyNode = cyRef?.getElementById(String(node.id));
    if (cyNode && cyNode.length > 0) {
      cyRef?.nodes().unselect();
      cyNode.select();
      cyRef?.animate({ center: { eles: cyNode }, duration: 300 });
    }

    setSelectedId(node.id);
    void fetchNodeDetail(node.id);
    void fetchNeighbors(node.id);
  };


  const clearGraph = () => {
    batch(() => {
      setExpandedIds(new Set<number>());
      setSelectedId(null);
      setSelectedDetail(null);
      setAllNodeTypes([]);
    });
    cyRef?.elements().remove();
    setGraphEmpty(true);
  };

  const fitGraph = () => {
    cyRef?.fit(undefined, 40);
  };

  // Navigate to a neighbor node from the detail panel
  const navigateToNode = async (nodeId: number, node_type?: string, nodeName?: string) => {
    // Add node if not already in graph
    if (cyRef && cyRef.getElementById(String(nodeId)).length === 0) {
      addNodesToGraph([{
        id: nodeId,
        node_type: node_type ?? '',
        name: nodeName ?? `#${nodeId}`,
        data_class: 'INTERNAL' as any,
        content: null,
      }]);
    }

    const cyNode = cyRef?.getElementById(String(nodeId));
    if (cyNode && cyNode.length > 0) {
      cyRef?.nodes().unselect();
      cyNode.select();
      cyRef?.animate({ center: { eles: cyNode }, duration: 300 });
    }

    setSelectedId(nodeId);
    void fetchNodeDetail(nodeId);
  };

  return (
    <div class="kg-explorer">
      {/* ── Left panel: search ────────────────────────── */}
      <div class="kg-search-panel">
        <div class="kg-search-header">
          <h3><Brain size={16} /> Knowledge Graph</h3>
          <Show when={props.kgStats()}>
            {(stats) => (
              <div class="kg-stats-row">
                <span class="pill neutral">{stats().node_count} nodes</span>
                <span class="pill neutral">{stats().edge_count} edges</span>
              </div>
            )}
          </Show>
        </div>

        <div class="kg-search-mode">
          <button
            class={searchMode() === 'fts' ? 'active' : ''}
            onClick={() => setSearchMode('fts')}
          ><PenLine size={14} /> Text</button>
          <button
            class={searchMode() === 'vector' ? 'active' : ''}
            onClick={() => setSearchMode('vector')}
          >🧲 Semantic</button>
        </div>

        <Show when={searchMode() === 'vector' && embeddingModels().length > 1}>
          <div class="kg-model-selector">
            <label>Model:</label>
            <select
              value={selectedModel()}
              onInput={(e) => setSelectedModel(e.currentTarget.value)}
            >
              <For each={embeddingModels()}>
                {(m) => <option value={m.modelId}>{m.modelId} ({m.dimensions}d)</option>}
              </For>
            </select>
          </div>
        </Show>

        <div class="kg-search-input">
          <input
            type="text"
            placeholder={searchMode() === 'fts' ? 'Full-text search…' : 'Semantic search…'}
            value={searchQuery()}
            onInput={(e) => setSearchQuery(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void doSearch(); }}
          />
          <button disabled={searching()} onClick={() => void doSearch()}>
            {searching() ? '…' : <Search size={14} />}
          </button>
        </div>

        <div class="kg-search-results">
          <For each={searchResults()}>
            {(node) => (
              <div
                class={`kg-result-card ${selectedId() === node.id ? 'selected' : ''}`}
                onClick={() => addSearchResultToGraph(node)}
              >
                <div class="kg-result-header">
                  <span
                    class="kg-type-dot"
                    style={`background: ${nodeColor(node.node_type, getThemeFamily()).border}`}
                  />
                  <span class="kg-result-name">{node.name}</span>
                </div>
                <div class="kg-result-meta">
                  <span class="kg-result-type">{node.node_type}</span>
                  <span class="kg-result-class">{node.data_class}</span>
                  {'distance' in node && typeof (node as any).distance === 'number' && (
                    <span class="kg-result-distance" title={`Distance: ${((node as any).distance as number).toFixed(4)}`}>
                      📐 {((node as any).distance as number).toFixed(3)}
                    </span>
                  )}
                </div>
              </div>
            )}
          </For>
        </div>

        <Collapsible open={showFilters()} onOpenChange={setShowFilters}>
        <div class="kg-search-actions">
          <button onClick={clearGraph} title="Clear graph"><Trash2 size={14} /> Clear</button>
          <button onClick={fitGraph} title="Fit to view">⊙ Fit</button>
          <CollapsibleTrigger as="button" title="Filters">
            🔽 Filters {typeFilters().size > 0 ? `(${typeFilters().size})` : ''}
          </CollapsibleTrigger>
        </div>

        <CollapsibleContent>
          <div class="kg-filters">
            <For each={allNodeTypes()}>
              {(type) => (
                <label class="kg-filter-item">
                  <input
                    type="checkbox"
                    checked={typeFilters().size === 0 || typeFilters().has(type)}
                    onChange={(e) => {
                      setTypeFilters((prev) => {
                        const next = new Set<string>(prev);
                        if (e.currentTarget.checked) {
                          next.add(type);
                        } else {
                          if (next.size === 0) {
                            for (const t of allNodeTypes()) {
                              if (t !== type) next.add(t);
                            }
                          } else {
                            next.delete(type);
                            if (next.size === 0) return new Set<string>();
                          }
                        }
                        return next;
                      });
                    }}
                  />
                  <span class="kg-type-dot" style={`background: ${nodeColor(type, getThemeFamily()).border}`} />
                  {type}
                </label>
              )}
            </For>
          </div>
        </CollapsibleContent>
        </Collapsible>
      </div>

      {/* ── Center: Cytoscape graph ───────────────────── */}
      <div class="kg-graph-area">
        <div ref={cyContainer} class="kg-cy-container" />
        <Show when={graphEmpty()}>
          <div class="kg-empty-overlay">
            <p>Search for nodes to explore the knowledge graph.</p>
            <p class="muted">Double-click a node to expand its connections.</p>
          </div>
        </Show>
        <Show when={!graphEmpty()}>
          <div class="kg-hint-bar">
            💡 Double-click a node to expand its connections
          </div>
        </Show>
      </div>

      {/* ── Right panel: detail ───────────────────────── */}
      <Show when={selectedDetail()}>
        {(detail) => (
          <div class="kg-detail-panel">
            <div class="kg-detail-header">
              <h3>Node #{detail().id}</h3>
              <button class="kg-close-btn" onClick={() => { setSelectedId(null); setSelectedDetail(null); }}>✕</button>
            </div>

            <Show when={!expandedIds().has(detail().id)}>
              <button
                class="kg-expand-btn"
                onClick={() => void fetchNeighbors(detail().id)}
              >
                🔗 Expand connections
              </button>
            </Show>
            <Show when={expandedIds().has(detail().id)}>
              <span class="kg-expanded-badge">✓ Connections expanded</span>
            </Show>

            <div class="kg-detail-field">
              <label>Name</label>
              <span class="kg-detail-readonly">{detail().name}</span>
            </div>

            <div class="kg-detail-field">
              <label>Type</label>
              <span class="kg-detail-readonly">{detail().node_type}</span>
            </div>

            <div class="kg-detail-field">
              <label>Classification</label>
              <span class="kg-detail-readonly">{detail().data_class}</span>
            </div>

            <div class="kg-detail-field">
              <label>Content</label>
              <pre class="kg-detail-content">{detail().content || '(no content)'}</pre>
            </div>

            <Show when={detail().edges.length > 0}>
              <div class="kg-detail-edges">
                <h4>Edges ({detail().edges.length})</h4>
                <For each={detail().edges}>
                  {(edge) => {
                    const otherId = edge.source_id === detail().id ? edge.target_id : edge.source_id;
                    const direction = edge.source_id === detail().id ? '→' : '←';
                    // Try to get name from cytoscape
                    const otherName = () => {
                      const cyNode = cyRef?.getElementById(String(otherId));
                      return (cyNode && cyNode.length > 0) ? (cyNode.data('fullName') as string) : `#${otherId}`;
                    };
                    const otherType = () => {
                      const cyNode = cyRef?.getElementById(String(otherId));
                      return (cyNode && cyNode.length > 0) ? (cyNode.data('node_type') as string) : undefined;
                    };
                    return (
                      <div class="kg-edge-row">
                        <span
                          class="kg-edge-type"
                          style={`color: ${edgeColor(edge.edge_type, getThemeFamily())}`}
                        >{edge.edge_type}</span>
                        <span class="kg-edge-dir">{direction}</span>
                        <button
                          class="kg-edge-target"
                          onClick={() => void navigateToNode(otherId, otherType(), otherName())}
                        >
                          {otherName()}
                        </button>
                      </div>
                    );
                  }}
                </For>
              </div>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
};

export default KnowledgeExplorer;
