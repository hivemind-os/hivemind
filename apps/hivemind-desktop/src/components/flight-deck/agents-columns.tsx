import { Show } from 'solid-js';
import type { ColumnDef } from '@tanstack/solid-table';
import type { GlobalAgentEntry, TokenUsage, AgentStatus } from '~/types';
import { HelpCircle, Lock } from 'lucide-solid';

// ---- Helpers ----

function agentStatusColor(status: AgentStatus): string {
  switch (status) {
    case 'spawning': return 'spawning';
    case 'active': return 'active';
    case 'waiting': return 'waiting';
    case 'paused': return 'paused';
    case 'blocked': return 'blocked';
    case 'done': return 'done';
    case 'error': return 'error';
    default: return '';
  }
}

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${tokens}`;
}

function durationStr(startMs: number | null | undefined): string {
  if (!startMs) return '—';
  const secs = Math.floor((Date.now() - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

// ---- Enriched row type ----

export interface AgentRow {
  agent: GlobalAgentEntry;
  usage: TokenUsage | undefined;
  questionCount: number;
  approvalCount: number;
}

export interface AgentColumnCallbacks {
  onQuestionClick?: (agent_id: string) => void;
  onApprovalClick?: (agent_id: string) => void;
  /** Reactive tick signal — read it in runtime cells to force re-render every second */
  tick?: () => number;
}

// ---- Column definitions ----

export function createAgentColumns(callbacks?: AgentColumnCallbacks): ColumnDef<AgentRow, any>[] {
  return [
    {
      id: 'gates',
      header: 'Gates',
      size: 90,
      minSize: 60,
      cell: (info) => {
        const row = info.row.original;
        return (
          <div class="flex items-center gap-1">
            <Show when={row.questionCount > 0}>
              <span
                class="fd-badge fd-badge-question"
                style="font-size:0.75em;cursor:pointer;"
                onClick={(e) => {
                  e.stopPropagation();
                  callbacks?.onQuestionClick?.(row.agent.agent_id);
                }}
              >
                <HelpCircle size={12} /> {row.questionCount}
              </span>
            </Show>
            <Show when={row.approvalCount > 0}>
              <span
                class="fd-badge fd-badge-approval"
                style="font-size:0.75em;cursor:pointer;"
                onClick={(e) => {
                  e.stopPropagation();
                  callbacks?.onApprovalClick?.(row.agent.agent_id);
                }}
              >
                <Lock size={12} /> {row.approvalCount}
              </span>
            </Show>
            <Show when={row.questionCount === 0 && row.approvalCount === 0}>
              <span class="text-muted-foreground text-xs">—</span>
            </Show>
          </div>
        );
      },
      enableSorting: false,
    },
    {
      id: 'id',
      accessorFn: (row) => row.agent.agent_id,
      header: 'ID',
      size: 140,
      minSize: 80,
      cell: (info) => {
        const val = info.getValue() as string;
        return (
          <code class="text-xs" title={val}>{val}</code>
        );
      },
      enableSorting: false,
    },
    {
      id: 'name',
      accessorFn: (row) =>
        row.agent.spec.friendly_name || row.agent.spec.name,
      header: 'Name',
      size: 150,
      minSize: 80,
      enableSorting: true,
    },
    {
      id: 'persona',
      accessorFn: (row) => row.agent.spec.persona_id ?? '',
      header: 'Persona',
      size: 110,
      minSize: 60,
      cell: (info) => {
        const val = info.getValue() as string;
        return val ? (
          <span class="text-xs" title={val}>{val}</span>
        ) : (
          <span class="text-muted-foreground text-xs">—</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'status',
      accessorFn: (row) => row.agent.status,
      header: 'Status',
      size: 90,
      minSize: 60,
      cell: (info) => {
        const status = info.getValue() as AgentStatus;
        return (
          <span class={`flight-deck-status ${agentStatusColor(status)}`}>
            {status}
          </span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'model',
      accessorFn: (row) =>
        row.agent.active_model || row.agent.spec.model || '',
      header: 'Model',
      size: 120,
      minSize: 60,
      cell: (info) => {
        const val = info.getValue() as string;
        return val ? (
          <span class="text-xs">{val}</span>
        ) : (
          <span class="text-muted-foreground text-xs">—</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'parent',
      accessorFn: (row) => row.agent.parent_id ?? '',
      header: 'Parent',
      size: 90,
      minSize: 60,
      cell: (info) => {
        const val = info.getValue() as string;
        return val ? (
          <code class="text-xs" title={val}>{val.slice(0, 8)}…</code>
        ) : (
          <span class="text-muted-foreground text-xs">—</span>
        );
      },
      enableSorting: false,
    },
    {
      id: 'tokens',
      accessorFn: (row) => {
        const u = row.usage;
        return u ? (u.input_tokens + u.output_tokens) : 0;
      },
      header: 'Tokens',
      size: 80,
      minSize: 50,
      cell: (info) => {
        const val = info.getValue() as number;
        return val > 0 ? formatTokens(val) : (
          <span class="text-muted-foreground">—</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'llm_calls',
      accessorFn: (row) => row.usage?.model_calls ?? 0,
      header: 'LLM Calls',
      size: 80,
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
      id: 'tool_calls',
      accessorFn: (row) => row.usage?.tool_calls ?? 0,
      header: 'Tool Calls',
      size: 80,
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
      accessorFn: (row) => row.agent.started_at_ms ?? null,
      header: 'Runtime',
      size: 90,
      minSize: 60,
      cell: (info) => {
        callbacks?.tick?.();
        return durationStr(info.getValue() as number | null);
      },
      enableSorting: true,
      sortingFn: (a, b) => {
        const aMs = a.original.agent.started_at_ms ?? 0;
        const bMs = b.original.agent.started_at_ms ?? 0;
        return aMs - bMs;
      },
    },
  ];
}
