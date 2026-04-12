import { For, Show, type JSX, createSignal } from 'solid-js';
import {
  type ColumnDef,
  type SortingState,
  type ColumnSizingState,
  createSolidTable,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
} from '@tanstack/solid-table';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '~/ui/table';
import { ArrowUpDown, ArrowUp, ArrowDown } from 'lucide-solid';

interface DataTableProps<TData, TValue> {
  columns: ColumnDef<TData, TValue>[];
  data: TData[];
  /** Currently selected row id (for highlight) */
  selectedRowId?: string | null;
  /** Callback when a row is clicked — receives the row data */
  onRowClick?: (row: TData) => void;
  /** Extract a unique id from a row */
  getRowId?: (row: TData) => string;
  /** Optional content rendered below the table when a row is selected */
  detailPanel?: () => JSX.Element;
  /** Optional empty state message */
  emptyMessage?: string;
}

export function DataTable<TData, TValue>(props: DataTableProps<TData, TValue>) {
  const [sorting, setSorting] = createSignal<SortingState>([]);
  const [columnSizing, setColumnSizing] = createSignal<ColumnSizingState>({});

  const table = createSolidTable({
    get data() {
      return props.data;
    },
    get columns() {
      return props.columns;
    },
    get getRowId() {
      return props.getRowId
        ? (row: TData) => props.getRowId!(row)
        : undefined;
    },
    state: {
      get sorting() {
        return sorting();
      },
      get columnSizing() {
        return columnSizing();
      },
    },
    onSortingChange: setSorting,
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: 'onChange',
    enableColumnResizing: true,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  return (
    <div class="fd-data-table">
      <div class="rounded-md border">
        <Table style={{ width: `${table.getCenterTotalSize()}px`, 'table-layout': 'fixed' }}>
          <TableHeader>
            <For each={table.getHeaderGroups()}>
              {(headerGroup) => (
                <TableRow>
                  <For each={headerGroup.headers}>
                    {(header) => (
                      <TableHead
                        colSpan={header.colSpan}
                        class={header.column.getCanSort() ? 'cursor-pointer select-none' : ''}
                        style={{ width: `${header.getSize()}px`, position: 'relative' }}
                        onClick={header.column.getToggleSortingHandler()}
                      >
                        <Show when={!header.isPlaceholder}>
                          <div class="flex items-center gap-1">
                            {flexRender(header.column.columnDef.header, header.getContext())}
                            <Show when={header.column.getCanSort()}>
                              {(() => {
                                const sorted = header.column.getIsSorted();
                                if (sorted === 'asc') return <ArrowUp class="h-3 w-3" />;
                                if (sorted === 'desc') return <ArrowDown class="h-3 w-3" />;
                                return <ArrowUpDown class="h-3 w-3 opacity-40" />;
                              })()}
                            </Show>
                          </div>
                        </Show>
                        <div
                          onMouseDown={header.getResizeHandler()}
                          onTouchStart={header.getResizeHandler()}
                          onClick={(e) => e.stopPropagation()}
                          class={`fd-col-resizer ${header.column.getIsResizing() ? 'fd-col-resizing' : ''}`}
                        />
                      </TableHead>
                    )}
                  </For>
                </TableRow>
              )}
            </For>
          </TableHeader>
          <TableBody>
            <Show
              when={table.getRowModel().rows?.length}
              fallback={
                <TableRow>
                  <TableCell colSpan={props.columns.length} class="h-24 text-center text-muted-foreground">
                    {props.emptyMessage ?? 'No results.'}
                  </TableCell>
                </TableRow>
              }
            >
              <For each={table.getRowModel().rows}>
                {(row) => {
                  const isSelected = () =>
                    props.selectedRowId != null && row.id === props.selectedRowId;
                  return (
                    <TableRow
                      data-state={isSelected() ? 'selected' : undefined}
                      class={props.onRowClick ? 'cursor-pointer' : ''}
                      onClick={() => props.onRowClick?.(row.original)}
                    >
                      <For each={row.getVisibleCells()}>
                        {(cell) => (
                          <TableCell style={{ width: `${cell.column.getSize()}px` }}>
                            {flexRender(cell.column.columnDef.cell, cell.getContext())}
                          </TableCell>
                        )}
                      </For>
                    </TableRow>
                  );
                }}
              </For>
            </Show>
          </TableBody>
        </Table>
      </div>
      <Show when={props.detailPanel}>
        {props.detailPanel!()}
      </Show>
    </div>
  );
}
