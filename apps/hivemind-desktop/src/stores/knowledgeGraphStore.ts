import { createSignal, type Accessor } from 'solid-js';
import { authFetch } from '~/lib/authFetch';
import type {
  AppContext,
  DataClass,
  KgNode,
  KgNodeWithEdges,
  KgStats,
} from '../types';

export interface KnowledgeGraphStoreDeps {
  context: Accessor<AppContext | null>;
  daemonOnline: Accessor<boolean>;
  runAction: (name: string, action: () => Promise<void>) => Promise<void>;
}

export function createKnowledgeGraphStore(deps: KnowledgeGraphStoreDeps) {
  const { context, daemonOnline, runAction } = deps;

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
  const [kgError, setKgError] = createSignal<string | null>(null);
  const kgFetch = async <T,>(path: string, init?: RequestInit): Promise<T> => {
    const url = context()?.daemon_url;
    if (!url) throw new Error('daemon offline');
    const resp = await authFetch(`${url}${path}`, init);
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(text || `${resp.status}`);
    }
    if (resp.status === 204) return undefined as unknown as T;
    return resp.json() as Promise<T>;
  };

  const loadKgStats = async () => {
    if (!daemonOnline()) { setKgStats(null); return; }
    try {
      setKgError(null);
      setKgStats(await kgFetch<KgStats>('/api/v1/knowledge/stats'));
    } catch (e: any) {
      const msg = e?.message ?? String(e);
      console.warn('[knowledgeGraphStore] loadKgStats failed:', msg);
      setKgError(msg);
      setKgStats(null);
    }
  };

  const loadKgNodes = async () => {
    if (!daemonOnline()) { setKgNodes([]); return; }
    try {
      setKgError(null);
      const typeFilter = kgNodeTypeFilter().trim();
      const params = new URLSearchParams();
      if (typeFilter) params.set('node_type', typeFilter);
      params.set('limit', '50');
      setKgNodes(await kgFetch<KgNode[]>(`/api/v1/knowledge/nodes?${params}`));
    } catch (e: any) {
      const msg = e?.message ?? String(e);
      console.warn('[knowledgeGraphStore] loadKgNodes failed:', msg);
      setKgError(msg);
      setKgNodes([]);
    }
  };

  const loadKgNode = async (nodeId: number) => {
    if (!daemonOnline()) { setKgSelectedNode(null); return; }
    try {
      setKgError(null);
      setKgSelectedNode(await kgFetch<KgNodeWithEdges>(`/api/v1/knowledge/nodes/${nodeId}`));
    } catch (e: any) {
      const msg = e?.message ?? String(e);
      console.warn('[knowledgeGraphStore] loadKgNode failed:', msg);
      setKgError(msg);
      setKgSelectedNode(null);
    }
  };

  const runKgSearch = async () => {
    const q = kgSearchQuery().trim();
    if (!daemonOnline() || !q) { setKgSearchResults([]); return; }
    try {
      setKgError(null);
      const params = new URLSearchParams({ q, limit: '20' });
      setKgSearchResults(await kgFetch<KgNode[]>(`/api/v1/knowledge/search?${params}`));
    } catch (e: any) {
      const msg = e?.message ?? String(e);
      console.warn('[knowledgeGraphStore] runKgSearch failed:', msg);
      setKgError(msg);
      setKgSearchResults([]);
    }
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

  return {
    kgStats, kgView, setKgView,
    kgNodes, kgSelectedNode, setKgSelectedNode,
    kgSearchQuery, setKgSearchQuery, kgSearchResults,
    kgNodeTypeFilter, setKgNodeTypeFilter,
    kgNewNodeType, setKgNewNodeType,
    kgNewNodeName, setKgNewNodeName,
    kgNewNodeContent, setKgNewNodeContent,
    kgNewNodeDataClass, setKgNewNodeDataClass,
    kgNewEdgeTargetId, setKgNewEdgeTargetId,
    kgNewEdgeType, setKgNewEdgeType,
    kgError,
    loadKgStats, loadKgNodes, loadKgNode,
    runKgSearch, kgCreateNode, kgDeleteNode, kgCreateEdge, kgDeleteEdge,
  };
}

export type KnowledgeGraphStore = ReturnType<typeof createKnowledgeGraphStore>;
