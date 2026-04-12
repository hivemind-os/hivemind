import { createMemo, For, Show } from 'solid-js';
import { Lock, HelpCircle, Bell, Hand, Filter, Archive } from 'lucide-solid';
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuBadge,
  SidebarMenuAction,
} from '~/ui';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import type { WorkflowStore } from '../stores/workflowStore';
import type { InteractionStore } from '../stores/interactionStore';

import { buildNamespaceTree, flattenNamespaceTree } from '~/lib/workflowGrouping';

interface SidebarWorkflowListProps {
  store: WorkflowStore;
  interactionStore: InteractionStore;
}

const ALL_STATUSES = ['pending','running','paused','waiting_on_input','waiting_on_event','completed','failed','killed'] as const;
const ACTIVE_STATUSES = ['pending','running','paused','waiting_on_input','waiting_on_event'] as const;
const TERMINAL_STATUSES = ['completed','failed','killed'] as const;

const statusDotColors: Record<string, string> = {
  running: '#34d399', completed: '#60a5fa', failed: '#f87171', killed: '#f87171',
  paused: '#fbbf24', waiting_on_input: '#fbbf24', waiting_on_event: '#fbbf24', pending: '#94a3b8',
};

function statusLabel(s: string) { return s.replace(/_/g, ' '); }

const SidebarWorkflowList = (props: SidebarWorkflowListProps) => {
  const hasBadge = (inst: { id: number; pending_agent_approvals?: number; pending_agent_questions?: number; status: string }) => {
    const c = props.interactionStore.badgeCountForEntity(`workflow/${inst.id}`);
    return (inst.pending_agent_approvals ?? 0) > 0 ||
      (inst.pending_agent_questions ?? 0) > 0 ||
      c.questions > 0 || c.approvals > 0 || c.gates > 0 ||
      inst.status === 'waiting_on_input' ||
      inst.status === 'waiting_on_event';
  };

  const activeFilterCount = createMemo(() => {
    const sf = props.store.statusFilter();
    const unchecked = ALL_STATUSES.filter(s => !sf[s]).length;
    const df = props.store.definitionFilter();
    const uncheckedDefs = Object.entries(df).filter(([_, v]) => !v).length;
    return unchecked + uncheckedDefs;
  });

  return (
    <SidebarGroup>
      <SidebarGroupContent>
        {/* Toolbar: filter + definitions */}
        <div class="flex justify-end gap-1 px-2 pt-2 pb-0.5">
          <Popover>
            <PopoverTrigger
              class="inline-flex items-center justify-center rounded-md p-1 text-sidebar-foreground hover:bg-sidebar-accent cursor-pointer transition-colors relative"
              aria-label="Filter workflows"
            >
              <Filter size={14} />
              <Show when={activeFilterCount() > 0}>
                <span class="wf-filter-badge" style="position:absolute;top:-4px;right:-4px;">{activeFilterCount()}</span>
              </Show>
            </PopoverTrigger>
            <PopoverContent class="wf-filter-popover">
              <div class="wf-filter-popover-section">
                <div class="wf-filter-popover-label">Active</div>
                <For each={[...ACTIVE_STATUSES]}>
                  {(status) => (
                    <label class="wf-filter-popover-item">
                      <input type="checkbox" checked={props.store.statusFilter()[status] ?? false} onChange={() => props.store.toggleStatus(status)} />
                      <span class="wf-filter-popover-dot" style={`background:${statusDotColors[status] ?? '#94a3b8'}`} />
                      {statusLabel(status)}
                    </label>
                  )}
                </For>
              </div>
              <div class="wf-filter-popover-section">
                <div class="wf-filter-popover-label">Terminal</div>
                <For each={[...TERMINAL_STATUSES]}>
                  {(status) => (
                    <label class="wf-filter-popover-item">
                      <input type="checkbox" checked={props.store.statusFilter()[status] ?? false} onChange={() => props.store.toggleStatus(status)} />
                      <span class="wf-filter-popover-dot" style={`background:${statusDotColors[status] ?? '#94a3b8'}`} />
                      {statusLabel(status)}
                    </label>
                  )}
                </For>
              </div>
              <Show when={props.store.definitions().length > 0}>
                <div class="wf-filter-popover-section">
                  <div class="wf-filter-popover-label">Workflow</div>
                  <For each={flattenNamespaceTree(buildNamespaceTree(props.store.definitions()))}>
                    {([ns, defs]) => (
                      <div style="margin-bottom: 4px;">
                        <div style="font-size: 0.78em; font-weight: 600; color: var(--muted-foreground, #888); padding: 2px 0; text-transform: uppercase; letter-spacing: 0.04em;">{ns}</div>
                        <For each={defs}>
                          {(def) => (
                            <label class="wf-filter-popover-item">
                              <input type="checkbox" checked={props.store.definitionFilter()[def.name] ?? true} onChange={() => props.store.toggleDefinition(def.name)} />
                              {def.name}
                            </label>
                          )}
                        </For>
                      </div>
                    )}
                  </For>
                </div>
              </Show>
              <div class="wf-filter-popover-section">
                <label class="wf-filter-popover-item">
                  <input type="checkbox" checked={props.store.showArchived()} onChange={() => props.store.toggleShowArchived()} />
                  Show archived
                </label>
              </div>
            </PopoverContent>
          </Popover>
        </div>

        {/* Search input */}
        <div class="flex flex-col gap-1.5 px-2 py-2">
          <input
            type="text"
            placeholder="Search workflows..."
            class="bg-sidebar-accent text-sidebar-foreground border border-sidebar-border rounded-md px-2 py-1 text-xs w-full"
            value={props.store.sidebarSearchQuery()}
            onInput={(e) => props.store.setSidebarSearchQuery(e.currentTarget.value)}
          />
        </div>

        {/* Workflow instance list */}
        <Show
          when={props.store.sidebarFilteredInstances().length > 0}
          fallback={<p class="px-2 text-xs text-muted-foreground">No workflow instances found.</p>}
        >
          <SidebarMenu>
            <For each={props.store.sidebarFilteredInstances()}>
              {(inst) => (
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={inst.id === props.store.sidebarSelectedInstanceId()}
                    onClick={() => props.store.setSidebarSelectedInstanceId(inst.id)}
                    size="sm"
                    class="gap-1.5"
                  >
                    <span
                      class="inline-block size-2 shrink-0 rounded-full"
                      classList={{
                        'bg-green-400': inst.status === 'running',
                        'bg-yellow-400':
                          inst.status === 'paused' ||
                          inst.status === 'waiting_on_input' ||
                          inst.status === 'waiting_on_event' ||
                          inst.status === 'pending',
                        'bg-red-400': inst.status === 'failed' || inst.status === 'killed',
                        'bg-slate-600': inst.status === 'completed',
                      }}
                      title={inst.status}
                    />
                    <span class="truncate">{inst.definition_name}</span>
                    <span class="text-[10px] text-muted-foreground shrink-0">
                      {inst.id}
                    </span>
                  </SidebarMenuButton>

                  <Show when={inst.status === 'completed' || inst.status === 'failed' || inst.status === 'killed'}>
                    <SidebarMenuAction
                      showOnHover
                      aria-label="Archive workflow"
                      title="Archive"
                      onClick={(e: MouseEvent) => {
                        e.stopPropagation();
                        void props.store.archiveInstance(inst.id);
                      }}
                    >
                      <Archive size={14} />
                    </SidebarMenuAction>
                  </Show>

                  <Show when={hasBadge(inst)}>
                    <SidebarMenuBadge>
                      {(() => {
                        const c = props.interactionStore.badgeCountForEntity(`workflow/${inst.id}`);
                        const approvals = Math.max(inst.pending_agent_approvals ?? 0, c.approvals);
                        const questions = Math.max(inst.pending_agent_questions ?? 0, c.questions);
                        return <>
                          <Show when={approvals > 0}>
                            <span
                              class="flex items-center gap-0.5 text-amber-400 cursor-pointer"
                              title="Pending approvals — click to review"
                              onClick={(e: MouseEvent) => {
                                e.stopPropagation();
                                props.store.setSidebarSelectedInstanceId(inst.id);
                              }}
                            >
                              <Lock size={12} />
                              <span class="text-[10px]">{approvals}</span>
                            </span>
                          </Show>
                          <Show when={questions > 0}>
                            <span
                              class="flex items-center gap-0.5 text-blue-400 cursor-pointer"
                              title="Pending questions — click to answer"
                              onClick={(e: MouseEvent) => {
                                e.stopPropagation();
                                props.store.setSidebarSelectedInstanceId(inst.id);
                              }}
                            >
                              <HelpCircle size={12} />
                              <span class="text-[10px]">{questions}</span>
                            </span>
                          </Show>
                        </>;
                      })()}
                      <Show when={inst.status === 'waiting_on_input'}>
                        <span
                          class="flex items-center gap-0.5 text-yellow-400 cursor-pointer"
                          title="Waiting on input — click to view"
                          onClick={(e: MouseEvent) => {
                            e.stopPropagation();
                            props.store.setSidebarSelectedInstanceId(inst.id);
                          }}
                        >
                          <Hand size={12} />
                        </span>
                      </Show>
                      <Show when={inst.status === 'waiting_on_event'}>
                        <span class="flex items-center gap-0.5 text-yellow-400">
                          <Bell size={12} />
                        </span>
                      </Show>
                    </SidebarMenuBadge>
                  </Show>
                </SidebarMenuItem>
              )}
            </For>
          </SidebarMenu>
        </Show>
      </SidebarGroupContent>
    </SidebarGroup>
  );
};

export default SidebarWorkflowList;
