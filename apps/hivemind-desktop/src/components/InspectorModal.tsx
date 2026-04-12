import { For, Show, type Accessor, type Setter } from 'solid-js';
import { Search } from 'lucide-solid';
import type { ChatMemoryItem, DaemonStatus, DataClass, KgNode, KgNodeWithEdges, KgStats, RiskScanRecord, ToolDefinition } from '../types';
import { dataClassBadge, riskClass } from '../utils';
import { highlightYaml } from './YamlHighlight';
import { Dialog, DialogContent, DialogHeader, DialogTitle, Button, Badge } from '~/ui';

type KgView = 'browse' | 'search' | 'create';

export interface InspectorModalProps {
  daemonStatus: Accessor<DaemonStatus | null>;
  tools: Accessor<ToolDefinition[]>;
  sessionMemory: Accessor<ChatMemoryItem[]>;
  memoryQuery: Accessor<string>;
  setMemoryQuery: Setter<string>;
  daemonOnline: Accessor<boolean>;
  busyAction: Accessor<string | null>;
  runAction: (name: string, action: () => Promise<void>) => Promise<void>;
  runMemorySearch: () => Promise<void>;
  riskScans: Accessor<RiskScanRecord[]>;
  kgStats: Accessor<KgStats | null>;
  kgView: Accessor<KgView>;
  setKgView: Setter<KgView>;
  loadKgNodes: () => Promise<void>;
  loadKgStats: () => Promise<void>;
  kgNodeTypeFilter: Accessor<string>;
  setKgNodeTypeFilter: Setter<string>;
  kgNodes: Accessor<KgNode[]>;
  loadKgNode: (nodeId: number) => Promise<void>;
  kgDeleteNode: (nodeId: number) => Promise<void>;
  kgSearchQuery: Accessor<string>;
  setKgSearchQuery: Setter<string>;
  runKgSearch: () => Promise<void>;
  kgSearchResults: Accessor<KgNode[]>;
  kgNewNodeType: Accessor<string>;
  setKgNewNodeType: Setter<string>;
  kgNewNodeName: Accessor<string>;
  setKgNewNodeName: Setter<string>;
  kgNewNodeContent: Accessor<string>;
  setKgNewNodeContent: Setter<string>;
  kgNewNodeDataClass: Accessor<DataClass>;
  setKgNewNodeDataClass: Setter<DataClass>;
  kgCreateNode: () => Promise<void>;
  kgSelectedNode: Accessor<KgNodeWithEdges | null>;
  setKgSelectedNode: Setter<KgNodeWithEdges | null>;
  kgNewEdgeTargetId: Accessor<string>;
  setKgNewEdgeTargetId: Setter<string>;
  kgNewEdgeType: Accessor<string>;
  setKgNewEdgeType: Setter<string>;
  kgCreateEdge: (source_id: number) => Promise<void>;
  kgDeleteEdge: (edgeId: number) => Promise<void>;
  memoryResults: Accessor<ChatMemoryItem[]>;
  onClose: () => void;
}

const InspectorModal = (props: InspectorModalProps) => {
  const daemonStatus = props.daemonStatus;
  const tools = props.tools;
  const sessionMemory = props.sessionMemory;
  const memoryQuery = props.memoryQuery;
  const setMemoryQuery = props.setMemoryQuery;
  const daemonOnline = props.daemonOnline;
  const busyAction = props.busyAction;
  const runAction = props.runAction;
  const runMemorySearch = props.runMemorySearch;
  const riskScans = props.riskScans;
  const kgStats = props.kgStats;
  const kgView = props.kgView;
  const setKgView = props.setKgView;
  const loadKgNodes = props.loadKgNodes;
  const loadKgStats = props.loadKgStats;
  const kgNodeTypeFilter = props.kgNodeTypeFilter;
  const setKgNodeTypeFilter = props.setKgNodeTypeFilter;
  const kgNodes = props.kgNodes;
  const loadKgNode = props.loadKgNode;
  const kgDeleteNode = props.kgDeleteNode;
  const kgSearchQuery = props.kgSearchQuery;
  const setKgSearchQuery = props.setKgSearchQuery;
  const runKgSearch = props.runKgSearch;
  const kgSearchResults = props.kgSearchResults;
  const kgNewNodeType = props.kgNewNodeType;
  const setKgNewNodeType = props.setKgNewNodeType;
  const kgNewNodeName = props.kgNewNodeName;
  const setKgNewNodeName = props.setKgNewNodeName;
  const kgNewNodeContent = props.kgNewNodeContent;
  const setKgNewNodeContent = props.setKgNewNodeContent;
  const kgNewNodeDataClass = props.kgNewNodeDataClass;
  const setKgNewNodeDataClass = props.setKgNewNodeDataClass;
  const kgCreateNode = props.kgCreateNode;
  const kgSelectedNode = props.kgSelectedNode;
  const setKgSelectedNode = props.setKgSelectedNode;
  const kgNewEdgeTargetId = props.kgNewEdgeTargetId;
  const setKgNewEdgeTargetId = props.setKgNewEdgeTargetId;
  const kgNewEdgeType = props.kgNewEdgeType;
  const setKgNewEdgeType = props.setKgNewEdgeType;
  const kgCreateEdge = props.kgCreateEdge;
  const kgDeleteEdge = props.kgDeleteEdge;
  const memoryResults = props.memoryResults;
  const setInspectorOpen = (open: boolean) => {
    if (!open) props.onClose();
  };

  return (
        <Dialog
          open={true}
          onOpenChange={(open) => { if (!open) setInspectorOpen(false); }}
        >
          <DialogContent class="max-w-[700px] max-h-[85vh] flex flex-col" data-testid="inspector-modal">
            <DialogHeader class="flex items-center justify-between border-b border-input pb-3">
              <DialogTitle class="flex items-center gap-1.5 text-base"><Search size={14} /> Inspector</DialogTitle>
              <div class="flex items-center gap-2">
                <Badge variant={daemonStatus() ? 'default' : 'secondary'}>
                  {daemonStatus() ? 'online' : 'offline'}
                </Badge>
              </div>
            </DialogHeader>
            <div class="flex-1 overflow-y-auto p-4 space-y-5">
              <Show
                when={daemonStatus()}
                fallback={<p class="text-sm text-muted-foreground">The daemon is offline. Start it to enable the local API.</p>}
              >
                {(status) => (
                  <dl class="grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
                    <div class="flex justify-between"><dt class="text-muted-foreground">Version</dt><dd>{status().version}</dd></div>
                    <div class="flex justify-between"><dt class="text-muted-foreground">Uptime</dt><dd>{status().uptime_secs.toFixed(0)}s</dd></div>
                  </dl>
                )}
              </Show>

              <section>
                <header class="mb-2 flex items-center justify-between">
                  <h3 class="text-sm font-semibold">Tools</h3>
                  <Badge variant="secondary">{tools().length} available</Badge>
                </header>
                <Show when={tools().length > 0} fallback={<p class="text-sm text-muted-foreground">No tools registered yet.</p>}>
                  <div class="space-y-2">
                    <For each={tools()}>
                      {(tool) => (
                        <article class="rounded-md border border-input bg-card p-2.5">
                          <header class="flex items-center justify-between"><strong class="text-sm">{tool.id}</strong><Badge variant="outline">{tool.approval}</Badge></header>
                          <p class="mt-1 text-xs text-muted-foreground">{tool.description}</p>
                          <dl class="mt-1.5 grid grid-cols-2 gap-x-3 text-xs">
                            <div class="flex justify-between"><dt class="text-muted-foreground">Channel</dt><dd>{tool.channel_class}</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">Side effects</dt><dd>{tool.side_effects ? 'yes' : 'no'}</dd></div>
                          </dl>
                        </article>
                      )}
                    </For>
                  </div>
                </Show>
              </section>

              <section>
                <header class="mb-2 flex items-center justify-between">
                  <h3 class="text-sm font-semibold">Session memory</h3>
                  <Badge variant="secondary">{sessionMemory().length} stored</Badge>
                </header>
                <Show when={sessionMemory().length > 0} fallback={<p class="text-sm text-muted-foreground">Session memory will appear here after messages are persisted.</p>}>
                  <div class="space-y-2">
                    <For each={sessionMemory()}>
                      {(memory) => (
                        <article class="rounded-md border border-input bg-card p-2.5">
                          <header class="flex items-center justify-between"><strong class="text-sm">{memory.name}</strong><Badge variant="outline">{memory.data_class}</Badge></header>
                          <p class="mt-1 text-xs text-muted-foreground">{memory.content ?? 'No stored content.'}</p>
                        </article>
                      )}
                    </For>
                  </div>
                </Show>
              </section>

              <section>
                <header class="mb-2"><h3 class="text-sm font-semibold">Knowledge search</h3></header>
                <div class="flex gap-2">
                  <input class="flex-1 rounded border border-input bg-transparent px-2 py-1 text-sm" value={memoryQuery()} placeholder="Search stored memory…" disabled={!daemonOnline()} onInput={(event) => setMemoryQuery(event.currentTarget.value)} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void runAction('memory-search', async () => { await runMemorySearch(); }); } }} />
                  <Button size="sm" disabled={!daemonOnline() || !memoryQuery().trim() || busyAction() === 'memory-search'} onClick={() => void runAction('memory-search', async () => { await runMemorySearch(); })}>{busyAction() === 'memory-search' ? 'Searching…' : 'Search'}</Button>
                </div>
                <Show when={memoryResults().length > 0}>
                  <div class="mt-2 space-y-2">
                    <For each={memoryResults()}>
                      {(memory) => (
                        <article class="rounded-md border border-input bg-card p-2.5">
                          <header class="flex items-center justify-between"><strong class="text-sm">{memory.name}</strong><Badge variant="outline">{memory.data_class}</Badge></header>
                          <p class="mt-1 text-xs text-muted-foreground">{memory.content ?? 'No stored content.'}</p>
                        </article>
                      )}
                    </For>
                  </div>
                </Show>
              </section>

              <section>
                <header class="mb-2 flex items-center justify-between">
                  <h3 class="text-sm font-semibold">Risk ledger</h3>
                  <Badge variant="secondary">{riskScans().length} scans</Badge>
                </header>
                <Show when={riskScans().length > 0} fallback={<p class="text-sm text-muted-foreground">Risk scans for this session will appear here.</p>}>
                  <div class="space-y-2">
                    <For each={riskScans()}>
                      {(scan) => (
                        <article class="rounded-md border border-input bg-card p-2.5">
                          <header class="flex items-center justify-between"><strong class="text-sm">{scan.source}</strong><Badge variant={scan.verdict === 'clean' ? 'default' : 'destructive'}>{scan.verdict}</Badge></header>
                          <p class="mt-1 text-xs text-muted-foreground">{scan.payload_preview}</p>
                          <dl class="mt-1.5 grid grid-cols-3 gap-x-3 text-xs">
                            <div class="flex justify-between"><dt class="text-muted-foreground">Action</dt><dd>{scan.action_taken}</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">Confidence</dt><dd>{Math.round(scan.confidence * 100)}%</dd></div>
                            <div class="flex justify-between"><dt class="text-muted-foreground">Class</dt><dd>{scan.data_class}</dd></div>
                          </dl>
                        </article>
                      )}
                    </For>
                  </div>
                </Show>
              </section>

              <section>
                <header class="mb-2 flex items-center gap-2">
                  <h3 class="text-sm font-semibold">Knowledge graph</h3>
                  <Badge variant="secondary">{kgStats()?.node_count ?? 0} nodes</Badge>
                  <Badge variant="secondary">{kgStats()?.edge_count ?? 0} edges</Badge>
                </header>
                <div class="mb-2 flex gap-1.5">
                  <Button variant={kgView() === 'browse' ? 'default' : 'secondary'} size="sm" onClick={() => { setKgView('browse'); void runAction('kg-load', async () => { await Promise.all([loadKgNodes(), loadKgStats()]); }); }}>Browse</Button>
                  <Button variant={kgView() === 'search' ? 'default' : 'secondary'} size="sm" onClick={() => setKgView('search')}>Search</Button>
                  <Button variant={kgView() === 'create' ? 'default' : 'secondary'} size="sm" onClick={() => setKgView('create')}>+ Node</Button>
                </div>
                <Show when={kgStats()}>
                  {(stats) => (
                    <Show when={stats().nodes_by_type.length > 0}>
                      <dl class="mb-2 grid grid-cols-2 gap-x-4 gap-y-0.5 text-xs">
                        <For each={stats().nodes_by_type}>{(tc) => (<div class="flex justify-between"><dt class="text-muted-foreground">{tc.name}</dt><dd>{tc.count}</dd></div>)}</For>
                      </dl>
                    </Show>
                  )}
                </Show>
                <Show when={kgView() === 'browse'}>
                  <div class="mb-2 flex gap-2">
                    <input class="flex-1 rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Filter by node type…" value={kgNodeTypeFilter()} onInput={(e) => setKgNodeTypeFilter(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === 'Enter') void runAction('kg-filter', loadKgNodes); }} />
                    <Button size="sm" disabled={busyAction() !== null} onClick={() => void runAction('kg-filter', loadKgNodes)}>Filter</Button>
                  </div>
                  <Show when={kgNodes().length > 0} fallback={<p class="text-sm text-muted-foreground">No nodes found. Load the graph or create a node.</p>}>
                    <div class="space-y-2">
                      <For each={kgNodes()}>
                        {(node) => (
                          <article class="rounded-md border border-input bg-card p-2.5">
                            <header class="flex items-center justify-between"><strong class="text-sm">{node.name}</strong><Badge variant="outline">{node.data_class}</Badge></header>
                            <p class="text-xs text-muted-foreground">{node.node_type} · #{node.id}</p>
                            <div class="mt-1.5 flex gap-1.5">
                              <Button variant="secondary" size="sm" onClick={() => void runAction('kg-inspect', () => loadKgNode(node.id))}>Inspect</Button>
                              <Button variant="destructive" size="sm" onClick={() => void kgDeleteNode(node.id)}>Delete</Button>
                            </div>
                          </article>
                        )}
                      </For>
                    </div>
                  </Show>
                </Show>
                <Show when={kgView() === 'search'}>
                  <div class="mb-2 flex gap-2">
                    <input class="flex-1 rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Search nodes…" value={kgSearchQuery()} onInput={(e) => setKgSearchQuery(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === 'Enter') void runAction('kg-search', runKgSearch); }} />
                    <Button size="sm" disabled={busyAction() !== null} onClick={() => void runAction('kg-search', runKgSearch)}>Search</Button>
                  </div>
                  <Show when={kgSearchResults().length > 0} fallback={<p class="text-sm text-muted-foreground">Enter a query to search the knowledge graph.</p>}>
                    <div class="space-y-2">
                      <For each={kgSearchResults()}>
                        {(node) => (
                          <article class="rounded-md border border-input bg-card p-2.5">
                            <header class="flex items-center justify-between"><strong class="text-sm">{node.name}</strong><Badge variant="outline">{node.data_class}</Badge></header>
                            <p class="text-xs text-muted-foreground">{node.node_type} · #{node.id}</p>
                            <Show when={node.content}>{(c) => <p class="mt-1 text-xs">{c().length > 120 ? c().slice(0, 120) + '…' : c()}</p>}</Show>
                            <div class="mt-1.5 flex gap-1.5">
                              <Button variant="secondary" size="sm" onClick={() => void runAction('kg-inspect', () => loadKgNode(node.id))}>Inspect</Button>
                            </div>
                          </article>
                        )}
                      </For>
                    </div>
                  </Show>
                </Show>
                <Show when={kgView() === 'create'}>
                  <div class="space-y-2">
                    <article class="rounded-md border border-input bg-card p-3 space-y-2">
                      <header><strong class="text-sm">New node</strong></header>
                      <input class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Node type (e.g. concept)" value={kgNewNodeType()} onInput={(e) => setKgNewNodeType(e.currentTarget.value)} />
                      <input class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Name" value={kgNewNodeName()} onInput={(e) => setKgNewNodeName(e.currentTarget.value)} />
                      <input class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Content (optional)" value={kgNewNodeContent()} onInput={(e) => setKgNewNodeContent(e.currentTarget.value)} />
                      <select class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" value={kgNewNodeDataClass()} onChange={(e) => setKgNewNodeDataClass(e.currentTarget.value as DataClass)}>
                        <option value="PUBLIC">PUBLIC</option><option value="INTERNAL">INTERNAL</option><option value="CONFIDENTIAL">CONFIDENTIAL</option><option value="RESTRICTED">RESTRICTED</option>
                      </select>
                      <Button size="sm" disabled={busyAction() !== null} onClick={() => void kgCreateNode()}>Create node</Button>
                    </article>
                  </div>
                </Show>
                <Show when={kgSelectedNode()}>
                  {(detail) => (
                    <div class="mt-2 space-y-2">
                      <article class="rounded-md border border-input bg-card p-3 space-y-2">
                        <header class="flex items-center justify-between"><strong class="text-sm">{detail().name}</strong><Badge variant="outline">{detail().data_class}</Badge></header>
                        <dl class="grid grid-cols-2 gap-x-3 text-xs">
                          <div class="flex justify-between"><dt class="text-muted-foreground">ID</dt><dd>{detail().id}</dd></div>
                          <div class="flex justify-between"><dt class="text-muted-foreground">Type</dt><dd>{detail().node_type}</dd></div>
                        </dl>
                        <Show when={detail().content}>{(c) => <pre class="code-block compact yaml-block" innerHTML={highlightYaml(c())} />}</Show>
                        <Show when={(detail().edges ?? []).length > 0}>
                          <h4 class="text-xs font-semibold">Edges ({(detail().edges ?? []).length})</h4>
                          <div class="space-y-1.5">
                            <For each={detail().edges ?? []}>
                              {(edge) => (
                                <article class="rounded border border-input bg-card/50 p-2">
                                  <header class="flex items-center justify-between text-xs"><strong>{edge.edge_type}</strong><Badge variant="outline">{edge.source_id} → {edge.target_id}</Badge></header>
                                  <div class="mt-1 flex gap-1.5">
                                    <Button variant="secondary" size="sm" onClick={() => { const other = edge.source_id === detail().id ? edge.target_id : edge.source_id; void runAction('kg-inspect', () => loadKgNode(other)); }}>Go to other</Button>
                                    <Button variant="destructive" size="sm" onClick={() => void kgDeleteEdge(edge.id)}>Delete</Button>
                                  </div>
                                </article>
                              )}
                            </For>
                          </div>
                        </Show>
                        <h4 class="text-xs font-semibold">Add edge</h4>
                        <input class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Target node ID" value={kgNewEdgeTargetId()} onInput={(e) => setKgNewEdgeTargetId(e.currentTarget.value)} />
                        <input class="w-full rounded border border-input bg-transparent px-2 py-1 text-sm" type="text" placeholder="Edge type (e.g. related_to)" value={kgNewEdgeType()} onInput={(e) => setKgNewEdgeType(e.currentTarget.value)} />
                        <div class="flex gap-1.5">
                          <Button size="sm" disabled={busyAction() !== null} onClick={() => void kgCreateEdge(detail().id)}>Add edge</Button>
                          <Button variant="secondary" size="sm" onClick={() => setKgSelectedNode(null)}>Close</Button>
                        </div>
                      </article>
                    </div>
                  )}
                </Show>
              </section>
            </div>
          </DialogContent>
        </Dialog>
  );
};

export default InspectorModal;
