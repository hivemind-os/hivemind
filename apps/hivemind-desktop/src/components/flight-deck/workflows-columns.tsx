import { Show } from 'solid-js';
import type { ColumnDef } from '@tanstack/solid-table';
import type { WorkflowInstanceSummary, WorkflowStatus } from '~/types';
import { HelpCircle, Lock, Hand, Bell } from 'lucide-solid';

// ---- Helpers ----

function wfDurationStr(startMs: number, endMs: number | null | undefined): string {
  if (!startMs) return '—';
  const end = endMs || Date.now();
  const secs = Math.floor((end - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

// ---- Enriched row type ----

export interface WorkflowRow {
  workflow: WorkflowInstanceSummary;
  questionCount: number;
  approvalCount: number;
}

export interface WorkflowColumnCallbacks {
  onApprovalClick?: (workflowId: number) => void;
  onQuestionClick?: (workflowId: number) => void;
  /** Reactive tick signal — read it in runtime cells to force re-render every second */
  tick?: () => number;
}

// ---- Column definitions ----

export function createWorkflowColumns(callbacks?: WorkflowColumnCallbacks): ColumnDef<WorkflowRow, any>[] {
  return [
    {
      id: 'gates',
      header: 'Gates',
      size: 110,
      minSize: 60,
      cell: (info) => {
        const wfRow = info.row.original;
        const wf = wfRow.workflow;
        return (
          <div class="flex items-center gap-1 flex-wrap">
            <Show when={wf.status === 'waiting_on_input'}>
              <span class="fd-badge fd-badge-question" style="font-size:0.75em;">
                <Hand size={12} /> feedback
              </span>
            </Show>
            <Show when={wf.status === 'waiting_on_event'}>
              <span class="fd-badge fd-badge-approval" style="font-size:0.75em;">
                <Bell size={12} /> event
              </span>
            </Show>
            <Show when={(wf.pending_agent_approvals ?? 0) > 0}>
              <span
                class="fd-badge fd-badge-approval"
                style="font-size:0.75em;animation:pulse 2s infinite;cursor:pointer;"
                onClick={(e) => {
                  e.stopPropagation();
                  callbacks?.onApprovalClick?.(wf.id);
                }}
              >
                <Lock size={12} /> {wf.pending_agent_approvals}
              </span>
            </Show>
            <Show when={(wf.pending_agent_questions ?? 0) > 0}>
              <span
                class="fd-badge fd-badge-question"
                style="font-size:0.75em;animation:pulse 2s infinite;cursor:pointer;"
                onClick={(e) => {
                  e.stopPropagation();
                  callbacks?.onQuestionClick?.(wf.id);
                }}
              >
                <HelpCircle size={12} /> {wf.pending_agent_questions}
              </span>
            </Show>
            <Show
              when={
                wf.status !== 'waiting_on_input' &&
                wf.status !== 'waiting_on_event' &&
                (wf.pending_agent_approvals ?? 0) === 0 &&
                (wf.pending_agent_questions ?? 0) === 0
              }
            >
              <span class="text-muted-foreground text-xs">—</span>
            </Show>
          </div>
        );
      },
      enableSorting: false,
    },
    {
      id: 'id',
      accessorFn: (row) => row.workflow.id,
      header: 'ID',
      size: 90,
      minSize: 60,
      cell: (info) => (
        <code class="text-xs">{info.getValue()}</code>
      ),
      enableSorting: false,
    },
    {
      id: 'definition',
      accessorFn: (row) =>
        `${row.workflow.definition_name} v${row.workflow.definition_version}`,
      header: 'Definition',
      size: 150,
      minSize: 80,
      cell: (info) => {
        const wf = info.row.original.workflow;
        return (
          <span>
            {wf.definition_name}{' '}
            <span class="text-muted-foreground text-xs">v{wf.definition_version}</span>
          </span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'status',
      accessorFn: (row) => row.workflow.status,
      header: 'Status',
      size: 100,
      minSize: 60,
      cell: (info) => {
        const status = info.getValue() as WorkflowStatus;
        const wf = info.row.original.workflow;
        return (
          <span class={`flight-deck-status ${status}`}>
            {status.replace(/_/g, ' ')}
            {wf.execution_mode === 'shadow' && (
              <span class="wf-test-badge" style="margin-left:4px;">TEST</span>
            )}
          </span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'parent',
      accessorFn: (row) =>
        row.workflow.parent_agent_id ?? row.workflow.parent_session_id,
      header: 'Parent',
      size: 140,
      minSize: 80,
      cell: (info) => {
        const wf = info.row.original.workflow;
        if (wf.parent_agent_id) {
          return (
            <span class="text-xs" title={`agent: ${wf.parent_agent_id}`}>
              <span class="text-muted-foreground">agent:</span>{' '}
              <code>{wf.parent_agent_id}</code>
            </span>
          );
        }
        return (
          <span class="text-xs" title={`session: ${wf.parent_session_id}`}>
            <span class="text-muted-foreground">session:</span>{' '}
            <code>{wf.parent_session_id}</code>
          </span>
        );
      },
      enableSorting: false,
    },
    {
      id: 'trigger',
      accessorFn: (row) => row.workflow.trigger_step_id ?? '',
      header: 'Trigger',
      size: 90,
      minSize: 60,
      cell: (info) => {
        const val = info.getValue() as string;
        return val ? (
          <span class="text-xs">{val}</span>
        ) : (
          <span class="text-muted-foreground text-xs">manual</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'active_agents',
      accessorFn: (row) => row.workflow.child_agent_ids?.length ?? 0,
      header: 'Agents',
      size: 70,
      minSize: 50,
      cell: (info) => {
        const val = info.getValue() as number;
        return val > 0 ? val : (
          <span class="text-muted-foreground">—</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'runtime',
      accessorFn: (row) => row.workflow.created_at_ms,
      header: 'Runtime',
      size: 90,
      minSize: 60,
      cell: (info) => {
        callbacks?.tick?.();
        const wf = info.row.original.workflow;
        return wfDurationStr(wf.created_at_ms, wf.completed_at_ms);
      },
      enableSorting: true,
    },
  ];
}
