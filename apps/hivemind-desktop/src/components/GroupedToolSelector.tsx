import { For, createEffect, createMemo } from 'solid-js';
import type { ToolDefinition } from '../types';

export interface GroupedToolSelectorProps {
  tools: ToolDefinition[];
  /** Currently selected tool keys (tool.id or tool.name depending on toolKey) */
  selected: string[];
  /** Called with the updated selection array when the user toggles tools */
  onChange: (selected: string[]) => void;
  /** Extract the selection key from a ToolDefinition. Defaults to tool.id */
  toolKey?: (tool: ToolDefinition) => string;
  /** Max height of the scrollable area, e.g. "200px" */
  maxHeight?: string;
}

export default function GroupedToolSelector(props: GroupedToolSelectorProps) {
  const getKey = (tool: ToolDefinition) => (props.toolKey ?? ((t) => t.id))(tool);

  const selectedSet = createMemo(() => new Set(props.selected));

  const toolGroups = createMemo(() => {
    const groups = new Map<string, ToolDefinition[]>();
    for (const tool of props.tools) {
      const dotIdx = tool.id.indexOf('.');
      const prefix = dotIdx > 0 ? tool.id.substring(0, dotIdx) : 'other';
      if (!groups.has(prefix)) groups.set(prefix, []);
      groups.get(prefix)!.push(tool);
    }
    return Array.from(groups.entries()).sort((a, b) => a[0].localeCompare(b[0]));
  });

  const isSelected = (tool: ToolDefinition) => selectedSet().has(getKey(tool));

  const toggleTool = (tool: ToolDefinition) => {
    const key = getKey(tool);
    if (selectedSet().has(key)) {
      props.onChange(props.selected.filter((k) => k !== key));
    } else {
      props.onChange([...props.selected, key]);
    }
  };

  const toggleGroup = (tools: ToolDefinition[]) => {
    const allSelected = tools.every((t) => isSelected(t));
    if (allSelected) {
      const groupKeys = new Set(tools.map(getKey));
      props.onChange(props.selected.filter((k) => !groupKeys.has(k)));
    } else {
      const current = new Set(props.selected);
      for (const t of tools) current.add(getKey(t));
      props.onChange(Array.from(current));
    }
  };

  return (
    <div
      class="space-y-2 overflow-y-auto rounded-md border border-input p-2"
      style={props.maxHeight ? `max-height:${props.maxHeight}` : undefined}
    >
      <For each={toolGroups()}>
        {([prefix, tools]) => {
          const allSelected = () => tools.every((t) => isSelected(t));
          const someSelected = () => tools.some((t) => isSelected(t));
          return (
            <div>
              <label class="flex items-center gap-2 text-sm font-medium text-foreground">
                <input
                  type="checkbox"
                  checked={allSelected()}
                  ref={(el) => {
                    createEffect(() => {
                      el.indeterminate = someSelected() && !allSelected();
                    });
                  }}
                  onChange={() => toggleGroup(tools)}
                  class="accent-primary rounded"
                />
                {prefix}
              </label>
              <div class="ml-5 mt-1 space-y-0.5">
                <For each={tools}>
                  {(tool) => (
                    <label class="flex items-center gap-2 text-xs text-foreground/70" title={tool.description}>
                      <input
                        type="checkbox"
                        checked={isSelected(tool)}
                        onChange={() => toggleTool(tool)}
                        class="accent-primary rounded"
                      />
                      {tool.name}
                    </label>
                  )}
                </For>
              </div>
            </div>
          );
        }}
      </For>
    </div>
  );
}
