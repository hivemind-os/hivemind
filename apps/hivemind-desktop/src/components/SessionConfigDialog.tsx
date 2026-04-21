import { For, Show, createMemo } from 'solid-js';
import type { Accessor, Setter } from 'solid-js';
import type { Persona, ToolDefinition, InstalledSkill } from '../types';
import { Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle, DialogFooter, Button } from '~/ui';
import { invoke } from '@tauri-apps/api/core';
import GroupedToolSelector from './GroupedToolSelector';
import { buildNamespaceTree, flattenNamespaceTree } from '~/lib/workflowGrouping';

export interface SessionConfigDialogProps {
  open: boolean;
  session_id: Accessor<string | null>;
  personas: Accessor<Persona[]>;
  tools: Accessor<ToolDefinition[]>;
  installedSkills: Accessor<InstalledSkill[]>;
  selectedAgentId: Accessor<string>;
  setSelectedAgentId: Setter<string>;
  selectedDataClass: Accessor<string>;
  setSelectedDataClass: Setter<string>;
  excludedTools: Accessor<string[]>;
  setExcludedTools: Setter<string[]>;
  excludedSkills: Accessor<string[]>;
  setExcludedSkills: Setter<string[]>;
  onClose: () => void;
}

const DATA_CLASSES = [
  { value: 'PUBLIC', label: 'Public', desc: 'No restrictions' },
  { value: 'INTERNAL', label: 'Internal', desc: 'Organization internal' },
  { value: 'CONFIDENTIAL', label: 'Confidential', desc: 'Restricted access' },
  { value: 'RESTRICTED', label: 'Restricted', desc: 'Highly sensitive' },
];

export default function SessionConfigDialog(props: SessionConfigDialogProps) {
  // Resolve the selected agent ID: App.tsx uses 'general' as shorthand for 'system/general'
  const resolvedAgentId = createMemo(() => {
    const raw = props.selectedAgentId();
    const personas = props.personas();
    if (personas.some((p) => p.id === raw)) return raw;
    // Try 'system/{raw}' mapping
    const mapped = `system/${raw}`;
    if (personas.some((p) => p.id === mapped)) return mapped;
    return personas[0]?.id ?? raw;
  });

  const currentPersona = createMemo(() =>
    props.personas().find((p) => p.id === resolvedAgentId()) ?? props.personas()[0]
  );

  const selectedToolIds = createMemo(() => {
    const excluded = new Set(props.excludedTools());
    return props.tools().map(t => t.id).filter(id => !excluded.has(id));
  });

  const enabledSkills = createMemo(() =>
    props.installedSkills().filter((s) => s.enabled)
  );

  const onToolSelectionChange = (selected: string[]) => {
    const selectedSet = new Set(selected);
    props.setExcludedTools(props.tools().map(t => t.id).filter(id => !selectedSet.has(id)));
  };

  const isSkillExcluded = (name: string) => props.excludedSkills().includes(name);

  const toggleSkill = (name: string) => {
    const excluded = props.excludedSkills();
    if (excluded.includes(name)) {
      props.setExcludedSkills(excluded.filter((n) => n !== name));
    } else {
      props.setExcludedSkills([...excluded, name]);
    }
  };

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => { if (!open) props.onClose(); }}
    >
      <DialogContent class="max-w-[600px] max-h-[80vh] flex flex-col overflow-hidden p-0" data-testid="session-config-dialog">
        <DialogHeader class="px-6 pt-6 pb-2">
          <DialogTitle class="flex items-center gap-2">
            <span>{currentPersona()?.avatar || '🤖'}</span>
            Session Configuration
          </DialogTitle>
        </DialogHeader>

        <DialogBody class="px-6 pb-2">
          <div class="space-y-4">
            {/* Persona Section */}
            <section>
              <h4 class="mb-2 text-sm font-medium text-foreground">Persona</h4>
              <select
                class="w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm text-foreground"
                value={resolvedAgentId()}
                onChange={(e) => {
                  const newPersonaId = e.currentTarget.value;
                  props.setSelectedAgentId(newPersonaId);
                  // Notify the backend so MCP servers are reloaded for the new persona.
                  const sid = props.session_id();
                  if (sid) {
                    invoke('chat_set_session_persona', { session_id: sid, persona_id: newPersonaId }).catch((err: any) =>
                      console.warn('Failed to set session persona:', err),
                    );
                  }
                }}
              >
                <For each={flattenNamespaceTree(buildNamespaceTree(props.personas(), (p) => p.id, (p) => p.name))}>
                  {([ns, personas]) => (
                    <optgroup label={ns}>
                      <For each={personas}>
                        {(persona) => (
                          <option value={persona.id}>
                            {persona.avatar || '🤖'} {persona.name}
                          </option>
                        )}
                      </For>
                    </optgroup>
                  )}
                </For>
              </select>
              <Show when={currentPersona()?.description}>
                <p class="mt-1 text-xs text-muted-foreground">{currentPersona()!.description}</p>
              </Show>
            </section>

            {/* Data Classification Section */}
            <section>
              <h4 class="mb-2 text-sm font-medium text-foreground">Data Classification</h4>
              <div class="flex flex-wrap gap-2">
                <For each={DATA_CLASSES}>
                  {(dc) => (
                    <label class={`flex cursor-pointer items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm transition-colors ${props.selectedDataClass() === dc.value ? 'border-primary bg-primary/10 text-foreground' : 'border-input text-muted-foreground hover:bg-accent'}`}>
                      <input
                        type="radio"
                        name="data_class"
                        value={dc.value}
                        checked={props.selectedDataClass() === dc.value}
                        onChange={() => props.setSelectedDataClass(dc.value)}
                        class="sr-only"
                      />
                      {dc.label}
                    </label>
                  )}
                </For>
              </div>
            </section>

            {/* Tools Section */}
            <section>
              <h4 class="mb-2 text-sm font-medium text-foreground">
                Tools
                <span class="ml-1 text-xs text-muted-foreground">
                  ({props.tools().length - props.excludedTools().length}/{props.tools().length} enabled)
                </span>
              </h4>
              <GroupedToolSelector
                tools={props.tools()}
                selected={selectedToolIds()}
                onChange={onToolSelectionChange}
                maxHeight="200px"
              />
            </section>

            {/* Skills Section */}
            <Show when={enabledSkills().length > 0}>
              <section>
                <h4 class="mb-2 text-sm font-medium text-foreground">
                  Skills
                  <span class="ml-1 text-xs text-muted-foreground">
                    ({enabledSkills().length - props.excludedSkills().length}/{enabledSkills().length} enabled)
                  </span>
                </h4>
                <div class="max-h-[150px] space-y-1 overflow-y-auto rounded-md border border-input p-2">
                  <For each={enabledSkills()}>
                    {(skill) => (
                      <label class="flex items-center gap-2 text-sm" title={skill.manifest.description}>
                        <input
                          type="checkbox"
                          checked={!isSkillExcluded(skill.manifest.name)}
                          onChange={() => toggleSkill(skill.manifest.name)}
                          class="rounded"
                        />
                        <strong class="text-foreground">{skill.manifest.name}</strong>
                        <span class="text-xs text-muted-foreground">{skill.manifest.description}</span>
                      </label>
                    )}
                  </For>
                </div>
              </section>
            </Show>
          </div>
        </DialogBody>

        <DialogFooter class="px-6 pb-6 pt-2">
          <Button variant="outline" onClick={props.onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
