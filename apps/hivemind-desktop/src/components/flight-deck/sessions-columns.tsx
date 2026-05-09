import type { ColumnDef } from '@tanstack/solid-table';
import type { SessionTelemetryEntry, ChatRunState } from '~/types';
import { ClipboardList, Map as MapIcon } from 'lucide-solid';

// ---- Helpers ----

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${tokens}`;
}

function stateColor(state: ChatRunState): string {
  switch (state) {
    case 'running': return 'active';
    case 'paused': return 'paused';
    case 'interrupted': return 'error';
    case 'idle': return 'done';
    default: return '';
  }
}

// ---- Enriched row type ----

export interface SessionRow {
  entry: SessionTelemetryEntry;
  modality?: 'linear' | 'spatial';
}

// ---- Column definitions ----

export function createSessionColumns(): ColumnDef<SessionRow, any>[] {
  return [
    {
      id: 'title',
      accessorFn: (row) => row.entry.title || 'Untitled',
      header: 'Session',
      size: 200,
      minSize: 120,
      cell: (info) => {
        const val = info.getValue() as string;
        return (
          <span class="text-xs font-medium" title={val}>
            {val.length > 40 ? val.slice(0, 37) + '…' : val}
          </span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'persona',
      accessorFn: (row) => row.entry.persona_id ?? '',
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
      id: 'state',
      accessorFn: (row) => row.entry.state,
      header: 'State',
      size: 80,
      minSize: 60,
      cell: (info) => {
        const state = info.getValue() as ChatRunState;
        return (
          <span class={`flight-deck-status ${stateColor(state)}`}>
            {state}
          </span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'model_calls',
      accessorFn: (row) => row.entry.telemetry.total.model_calls,
      header: 'Model Calls',
      size: 90,
      minSize: 60,
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
      accessorFn: (row) => row.entry.telemetry.total.tool_calls,
      header: 'Tool Calls',
      size: 80,
      minSize: 60,
      cell: (info) => {
        const val = info.getValue() as number;
        return val > 0 ? val : (
          <span class="text-muted-foreground">—</span>
        );
      },
      enableSorting: true,
    },
    {
      id: 'input_tokens',
      accessorFn: (row) => row.entry.telemetry.total.input_tokens,
      header: 'Input ↑',
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
      id: 'output_tokens',
      accessorFn: (row) => row.entry.telemetry.total.output_tokens,
      header: 'Output ↓',
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
      id: 'cached_tokens',
      accessorFn: (row) => row.entry.telemetry.total.cached_input_tokens ?? 0,
      header: 'Cached',
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
  ];
}
