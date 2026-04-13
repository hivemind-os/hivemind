import { Component, createSignal, createEffect, createMemo, For, Index, Show, untrack, type JSX } from 'solid-js';
import { Pencil, Trash2, AlertTriangle, CheckCircle, XCircle, ClipboardList, Play, Bell, Inbox, Calendar, Wrench, Bot, Hand, Timer, Radio, RotateCcw, PenLine, GitBranch, Repeat, RotateCw, Flag, Loader2 } from 'lucide-solid';
import PermissionRulesEditor, { type PermissionRule as WfPermissionRule } from '../PermissionRulesEditor';
import { CronBuilder, TopicSelector, PersonaSelector, payloadKeysForTopic as sharedPayloadKeysForTopic } from '../shared';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import { invoke } from '@tauri-apps/api/core';
import {
  type DesignerNode,
  type DesignerEdge,
  type WorkflowVariable,
  type WorkflowAttachment,
  type ToolDefinitionProp,
  type PersonaProp,
  type PromptTemplateProp,
  type ChannelProp,
  NODE_CATEGORY_COLORS,
  SubtypeIcon,
  EnumEditor,
  inputStyle,
  labelStyle,
} from './types';

// Module-level counter for stable expression helper IDs across re-renders
let _exprHelperNextId = 0;

const TYPE_LABELS: Record<string, string> = {
  'string': 'Text',
  'number': 'Number',
  'boolean': 'Boolean',
  'object': 'Object',
  'array': 'List',
};
const TYPE_VALUES = ['string', 'number', 'boolean', 'object', 'array'] as const;

// ── StepConfigFields ───────────────────────────────────────────────────

export interface StepConfigFieldsProps {
  nodeId: string;
  getNode: () => DesignerNode;
  getCfg: () => Record<string, any>;
  getErrors: () => string[];
  onUpdateConfig: (key: string, value: any) => void;
  onPushHistory: () => void;
  readOnly?: boolean;
  toolDefinitions?: ToolDefinitionProp[];
  personas?: PersonaProp[];
  channels?: ChannelProp[];
  eventTopics?: { topic: string; description: string; payload_keys?: string[] }[];
  variables: () => WorkflowVariable[];
  wfAttachments: () => WorkflowAttachment[];
  renderExpressionHelper: (
    onInsert: (text: string) => void,
    inputEl?: () => HTMLInputElement | HTMLTextAreaElement | undefined,
  ) => JSX.Element;
}

export function StepConfigFields(props: StepConfigFieldsProps) {
    const sub = untrack(() => props.getNode()?.subtype ?? '');
    const ro = () => props.readOnly;
    const doUpdate = props.onUpdateConfig;
    const doPush = props.onPushHistory;

    function fieldBorder(field: string): string {
      return props.getErrors().includes(field) ? '1px solid hsl(38 92% 50%)' : '1px solid hsl(var(--border))';
    }

    function missingMark(field: string) {
      return props.getErrors().includes(field) ? <span style={{ color: 'hsl(38 92% 50%)' }}>*</span> : null;
    }

    // Map of field name → input/textarea element for cursor-position insertion
    const fieldInputRefs: Record<string, HTMLInputElement | HTMLTextAreaElement> = {};

    // Label with optional expression helper
    function exprLabel(text: string, field: string, opts?: { required?: boolean }) {
      return (
        <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center' }}>
          <span style={{ flex: '1' }}>{text} {opts?.required ? missingMark(field) : null}</span>
          <Show when={!ro()}>
            {props.renderExpressionHelper((newVal) => {
              doUpdate(field, newVal);
            }, () => fieldInputRefs[field])}
          </Show>
        </div>
      );
    }

    // Helper to capture input ref for a field (use as ref={captureFieldRef('fieldName')})
    function captureFieldRef(field: string) {
      return (el: HTMLInputElement | HTMLTextAreaElement) => { fieldInputRefs[field] = el; };
    }

    // Look up payload keys for a topic (matches exact or first wildcard-compatible entry)
    function payloadKeysForTopic(topic: string | undefined): string[] {
      return sharedPayloadKeysForTopic(topic ?? '', props.eventTopics ?? []);
    }

    // ── Filter expression builder ──────────────────────────────────────
    type FilterRow = { field: string; op: string; value: string };
    type FilterJoin = '&&' | '||';

    function parseFilter(filter: string): { rows: FilterRow[]; joins: FilterJoin[] } {
      if (!filter.trim()) return { rows: [{ field: '', op: '==', value: '' }], joins: [] };
      const rows: FilterRow[] = [];
      const joins: FilterJoin[] = [];
      // Split on && / || outside quotes
      const parts: string[] = [];
      const ops: FilterJoin[] = [];
      let cur = '';
      let inQuote = false;
      for (let i = 0; i < filter.length; i++) {
        if (filter[i] === '"') { inQuote = !inQuote; cur += filter[i]; continue; }
        if (!inQuote) {
          if (filter.slice(i, i + 2) === '&&') { parts.push(cur); ops.push('&&'); cur = ''; i++; continue; }
          if (filter.slice(i, i + 2) === '||') { parts.push(cur); ops.push('||'); cur = ''; i++; continue; }
        }
        cur += filter[i];
      }
      parts.push(cur);
      for (const part of parts) {
        const row = parseCondition(part.trim());
        rows.push(row);
      }
      return { rows, joins: ops };
    }

    function parseCondition(s: string): FilterRow {
      const compOps = ['!=', '<=', '>=', '==', '<', '>'];
      for (const op of compOps) {
        // Find the operator outside of quotes
        let inQ = false;
        for (let i = 0; i < s.length; i++) {
          if (s[i] === '"') { inQ = !inQ; continue; }
          if (!inQ && s.slice(i, i + op.length) === op) {
            // For single-char ops, skip if part of a two-char op
            if (op.length === 1 && i + 1 < s.length && s[i + 1] === '=') continue;
            if (op === '<' && i > 0 && s[i - 1] === '!') continue;
            const field = s.slice(0, i).trim();
            let value = s.slice(i + op.length).trim();
            // strip quotes for display
            if (value.startsWith('"') && value.endsWith('"')) value = value.slice(1, -1);
            return { field, op, value };
          }
        }
      }
      // Couldn't parse as condition — put the whole thing in value as fallback
      return { field: '', op: '==', value: s };
    }

    function serializeFilter(rows: FilterRow[], joins: FilterJoin[]): string {
      return rows.map((r, i) => {
        const val = r.value.includes(' ') || r.value.includes('&&') || r.value.includes('||')
          ? `"${r.value}"` : r.value;
        const cond = r.field && r.value ? `${r.field} ${r.op} ${val}` : '';
        const join = i > 0 && joins[i - 1] ? ` ${joins[i - 1]} ` : '';
        return join + cond;
      }).join('').trim();
    }

    function renderFilterBuilder(configKey: string) {
      // untrack: reading props.getCfg() at signal-creation time would create a
      // reactive dependency that re-runs the entire switch block on every
      // keystroke, destroying and recreating these signals (and losing focus).
      const initial = parseFilter(untrack(() => props.getCfg()[configKey]) ?? '');
      const [rows, setRows] = createSignal<FilterRow[]>(initial.rows);
      const [joins, setJoins] = createSignal<FilterJoin[]>(initial.joins);

      function syncToConfig() {
        const s = serializeFilter(rows(), joins());
        doUpdate(configKey, s);
        doPush();
      }

      function updateRow(idx: number, key: keyof FilterRow, val: string) {
        setRows(r => r.map((row, i) => i === idx ? { ...row, [key]: val } : row));
      }

      function removeRow(idx: number) {
        setRows(r => r.filter((_, i) => i !== idx));
        setJoins(j => {
          const nj = [...j];
          if (idx > 0) nj.splice(idx - 1, 1);
          else if (nj.length > 0) nj.splice(0, 1);
          return nj;
        });
        // Sync after removing so the config is updated
        requestAnimationFrame(syncToConfig);
      }

      function addRow(join: FilterJoin) {
        setRows(r => [...r, { field: '', op: '==', value: '' }]);
        setJoins(j => [...j, join]);
      }

      const payloadKeys = () => payloadKeysForTopic(props.getCfg().topic);
      const OPS = ['==', '!=', '>', '<', '>=', '<='];

      return (
        <div style={{ display: 'flex', 'flex-direction': 'column', gap: '4px' }}>
          <Index each={rows()}>
            {(row, idx) => (
              <div>
                <Show when={idx > 0}>
                  <div style={{ display: 'flex', 'justify-content': 'center', 'margin': '2px 0' }}>
                    <select
                      style={{ background: 'hsl(var(--card))', border: '1px solid hsl(var(--border))', color: 'hsl(var(--primary))', 'border-radius': '4px', padding: '1px 6px', 'font-size': '0.7em', cursor: 'pointer' }}
                      value={joins()[idx - 1] ?? '&&'}
                      onChange={(e) => { setJoins(j => j.map((v, i) => i === idx - 1 ? e.currentTarget.value as FilterJoin : v)); syncToConfig(); }}
                      disabled={ro()}
                    >
                      <option value="&&">AND</option>
                      <option value="||">OR</option>
                    </select>
                  </div>
                </Show>
                <div style={{ display: 'flex', gap: '4px', 'align-items': 'center' }}>
                  {/* Field selector — dropdown if payload keys known, else text input */}
                  <Show when={payloadKeys().length > 0} fallback={
                    <input
                      style={{ ...inputStyle, flex: '1 1 0', width: 'auto', 'min-width': '0', 'font-size': '0.82em' }}
                      placeholder="event.field"
                      value={row().field}
                      onInput={(e) => updateRow(idx, 'field', e.currentTarget.value)}
                      onBlur={syncToConfig}
                      disabled={ro()}
                    />
                  }>
                    {(_) => (
                      <select
                        style={{ ...inputStyle, flex: '1 1 0', width: 'auto', 'min-width': '0', 'font-size': '0.82em' }}
                        value={row().field}
                        onChange={(e) => { updateRow(idx, 'field', e.currentTarget.value); syncToConfig(); }}
                        disabled={ro()}
                      >
                        <option value="">— field —</option>
                        <For each={payloadKeys()}>
                          {(k) => <option value={`event.${k}`}>event.{k}</option>}
                        </For>
                      </select>
                    )}
                  </Show>

                  {/* Operator selector */}
                  <select
                    style={{ ...inputStyle, flex: '0 0 54px', width: '54px', 'font-size': '0.82em', 'text-align': 'center' }}
                    value={row().op}
                    onChange={(e) => { updateRow(idx, 'op', e.currentTarget.value); syncToConfig(); }}
                    disabled={ro()}
                  >
                    <For each={OPS}>
                      {(op) => <option value={op}>{op}</option>}
                    </For>
                  </select>

                  {/* Value input with expression helper */}
                  <div style={{ flex: '1 1 0', 'min-width': '0', display: 'flex', 'align-items': 'center', gap: '2px' }}>
                    <input
                      ref={captureFieldRef(`filter:${idx}`)}
                      style={{ ...inputStyle, flex: '1 1 0', width: 'auto', 'min-width': '0', 'font-size': '0.82em' }}
                      placeholder="value or {{expression}}"
                      value={row().value}
                      onInput={(e) => updateRow(idx, 'value', e.currentTarget.value)}
                      onBlur={syncToConfig}
                      disabled={ro()}
                    />
                    <Show when={!ro()}>
                      {props.renderExpressionHelper((newVal) => {
                        updateRow(idx, 'value', newVal);
                        syncToConfig();
                      }, () => fieldInputRefs[`filter:${idx}`])}
                    </Show>
                  </div>

                  {/* Remove button */}
                  <Show when={!ro() && rows().length > 1}>
                    <button
                      onClick={() => removeRow(idx)}
                      style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.9em', padding: '0 2px' }}
                      title="Remove condition"
                    >✕</button>
                  </Show>
                </div>
              </div>
            )}
          </Index>
          <Show when={!ro()}>
            <div style={{ display: 'flex', gap: '6px', 'margin-top': '2px' }}>
              <button
                class="wf-btn-secondary"
                style="padding:2px 8px;font-size:0.82em;"
                onClick={() => addRow('&&')}
              >+ AND</button>
              <button
                class="wf-btn-secondary"
                style="padding:2px 8px;font-size:0.82em;"
                onClick={() => addRow('||')}
              >+ OR</button>
            </div>
          </Show>
        </div>
      );
    }

    // Topic selector using shared component
    function renderTopicSelector(configKey: string, opts?: { required?: boolean; borderOverride?: string }) {
      return (
        <div>
          <TopicSelector
            value={untrack(() => props.getCfg()[configKey]) ?? ''}
            onChange={(topic) => { doUpdate(configKey, topic); doPush(); }}
            topics={props.eventTopics ?? []}
            disabled={ro()}
            placeholder="e.g. chat.session.* or workflow.definition.saved"
          />
          <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px' }}>
            Supports wildcards: <code>chat.session.*</code> matches any sub-topic
          </div>
        </div>
      );
    }

    switch (sub) {
      case 'call_tool': {
        const tools = () => props.toolDefinitions ?? [];
        const selectedTool = () => tools().find(t => t.id === props.getCfg().tool_id);
        const [toolSearch, setToolSearch] = createSignal('');
        const [showToolDropdown, setShowToolDropdown] = createSignal(false);
        const [jsonMode, setJsonMode] = createSignal(false);

        const filteredTools = () => {
          const q = toolSearch().toLowerCase();
          if (!q) return tools();
          return tools().filter(t =>
            t.name.toLowerCase().includes(q) || t.id.toLowerCase().includes(q) || (t.description || '').toLowerCase().includes(q)
          );
        };

        // Build form fields from tool's input_schema (JSON Schema)
        function getSchemaProperties(tool: ToolDefinitionProp | undefined): { key: string; type: string; description: string; required: boolean; enumValues?: string[] }[] {
          if (!tool?.input_schema) return [];
          const schema = tool.input_schema as any;
          const props = schema.properties || {};
          const req = new Set(schema.required || []);
          return Object.entries(props).map(([key, prop]: [string, any]) => ({
            key,
            type: prop.type || 'string',
            description: prop.description || '',
            required: req.has(key),
            enumValues: prop.enum,
          }));
        }

        function selectTool(tool_id: string) {
          doUpdate('tool_id', tool_id);
          setShowToolDropdown(false);
          setToolSearch('');
          doPush();
        }

        function getArgsValue(key: string): string {
          const args = props.getCfg().arguments;
          if (!args || typeof args !== 'object') return '';
          const val = (args as Record<string, any>)[key];
          if (val === undefined || val === null) return '';
          return typeof val === 'object' ? JSON.stringify(val) : String(val);
        }

        function setArgField(key: string, value: string, type: string) {
          const args = typeof props.getCfg().arguments === 'object' ? { ...(props.getCfg().arguments as Record<string, any>) } : {};
          if (value === '') { delete args[key]; }
          else if (type === 'number' || type === 'integer') { args[key] = value.includes('.') ? parseFloat(value) : parseInt(value, 10); if (isNaN(args[key])) args[key] = value; }
          else if (type === 'boolean') { args[key] = value === 'true'; }
          else if (type === 'object' || type === 'array') { try { args[key] = JSON.parse(value); } catch { args[key] = value; } }
          else { args[key] = value; }
          doUpdate('arguments', args);
        }

        const schemaFields = () => getSchemaProperties(selectedTool());

        return (<>
          {/* Tool Picker */}
          <div style={labelStyle}>Tool {missingMark('tool_id')}</div>
          {(() => {
            return <Popover
              placement="bottom-start"
              sameWidth
              open={showToolDropdown()}
              onOpenChange={(open) => { if (!ro()) setShowToolDropdown(open); if (!open) setToolSearch(''); }}
            >
              <PopoverTrigger as="div" style={{
                ...inputStyle,
                border: fieldBorder('tool_id'),
                cursor: ro() ? 'default' : 'pointer',
                display: 'flex', 'align-items': 'center', gap: '6px',
                'min-height': '28px',
              }}>
                  <Show when={selectedTool()} fallback={<span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>Select a tool…</span>}>
                    {(tool) => (
                      <span style={{ 'font-size': '0.85em' }}>
                        <span style={{ 'font-weight': '600' }}>{tool().name}</span>
                        <span style={{ color: 'hsl(var(--muted-foreground))', 'margin-left': '6px', 'font-size': '0.9em' }}>{tool().id}</span>
                      </span>
                    )}
                  </Show>
                  <span style={{ 'margin-left': 'auto', 'font-size': '0.7em', color: 'hsl(var(--muted-foreground))' }}>{showToolDropdown() ? '▲' : '▼'}</span>
              </PopoverTrigger>
              <PopoverContent class="w-auto p-0" style={{
                'z-index': '10000',
                'max-height': '220px', 'overflow-y': 'auto', background: 'hsl(var(--card))',
                border: '1px solid hsl(var(--border))', 'border-radius': '0 0 6px 6px',
                'box-shadow': '0 6px 16px hsl(var(--foreground) / 0.2)',
              }}>
              <div style={{ padding: '4px', 'border-bottom': '1px solid hsl(var(--border))' }}>
                <input
                  ref={(el) => requestAnimationFrame(() => el?.focus())}
                  style={{ ...inputStyle, border: 'none', 'font-size': '0.82em', width: '100%', 'box-sizing': 'border-box' }}
                  placeholder="Search tools…"
                  value={toolSearch()}
                  onInput={(e) => setToolSearch(e.currentTarget.value)}
                  onKeyDown={(e) => { if (e.key === 'Escape') setShowToolDropdown(false); }}
                />
              </div>
              <For each={filteredTools()}>
                {(tool) => (
                  <button
                    onMouseDown={(e) => { e.preventDefault(); selectTool(tool.id); }}
                    style={{
                      display: 'block', width: '100%', 'text-align': 'left',
                      background: tool.id === props.getCfg().tool_id ? 'hsl(var(--primary) / 0.12)' : 'none',
                      border: 'none', padding: '6px 8px', cursor: 'pointer',
                      color: 'hsl(var(--foreground))', 'font-size': '0.8em',
                    }}
                    onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.08)')}
                    onMouseLeave={(e) => (e.currentTarget.style.background = tool.id === props.getCfg().tool_id ? 'hsl(var(--primary) / 0.12)' : 'none')}
                  >
                    <div style={{ 'font-weight': '500' }}>{tool.name}</div>
                    <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'white-space': 'nowrap', overflow: 'hidden', 'text-overflow': 'ellipsis' }}>
                      {tool.description || tool.id}
                    </div>
                  </button>
                )}
              </For>
              <Show when={filteredTools().length === 0}>
                <div style={{ padding: '10px', 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'text-align': 'center' }}>No tools found</div>
              </Show>
              </PopoverContent>
            </Popover>;
          })()}

          {/* Tool description hint */}
          <Show when={selectedTool()?.description}>
            {(desc) => (
              <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px', 'font-style': 'italic' }}>
                {desc()}
              </div>
            )}
          </Show>

          {/* Inputs header + JSON toggle */}
          <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center', 'margin-top': '8px' }}>
            <span style={{ flex: '1' }}>Inputs</span>
            <Show when={schemaFields().length > 0}>
              <button
                onClick={() => setJsonMode(!jsonMode())}
                style={{
                  background: 'none', border: '1px solid hsl(var(--border))',
                  color: 'hsl(var(--muted-foreground))', cursor: 'pointer',
                  'border-radius': '3px', padding: '1px 6px', 'font-size': '0.7em',
                }}
                title={jsonMode() ? 'Switch to form view' : 'Switch to JSON view'}
              >{jsonMode() ? <><ClipboardList size={14} /> Form</> : '{ } JSON'}</button>
            </Show>
          </div>

          <Show when={!jsonMode() && schemaFields().length > 0} fallback={
            <textarea
              style={{ ...inputStyle, 'min-height': '60px', resize: 'vertical', 'font-family': 'monospace', 'font-size': '0.82em' }}
              value={typeof props.getCfg().arguments === 'object' ? JSON.stringify(props.getCfg().arguments, null, 2) : (props.getCfg().arguments ?? '{}')}
              onInput={(e) => { try { doUpdate('arguments', JSON.parse(e.currentTarget.value)); } catch { /* only update on valid JSON */ } }}
              onBlur={() => doPush()} disabled={ro()}
            />
          }>
            {(_) => (
              <div style={{ display: 'flex', 'flex-direction': 'column', gap: '4px' }}>
                <Index each={schemaFields()}>
                  {(field, _idx) => {
                    const channelList = () => (field().key === 'connector_id' || field().key === 'channel_id') ? (props.channels ?? []) : [];
                    return (
                    <div>
                      <div style={{ display: 'flex', 'align-items': 'center', gap: '4px' }}>
                        <label style={{ 'font-size': '0.85em', color: 'hsl(var(--foreground))', 'font-weight': '500' }}>
                          {field().key}
                          {field().required ? <span style={{ color: 'hsl(38 92% 50%)', 'margin-left': '2px' }}>*</span> : null}
                        </label>
                        <span style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))' }}>
                          {field().type === 'integer' ? 'number' : field().type}
                        </span>
                        <Show when={!ro()}>
                          {props.renderExpressionHelper((newVal) => {
                            setArgField(field().key, newVal, 'string');
                          }, () => fieldInputRefs[`arg:${field().key}`])}
                        </Show>
                      </div>
                      <Show when={field().description}>
                        <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '2px' }}>
                          {field().description}
                        </div>
                      </Show>
                      <Show when={channelList().length > 0} fallback={
                      <Show when={field().enumValues && field().enumValues!.length > 0} fallback={
                        <Show when={field().type === 'boolean'} fallback={
                          <input
                            ref={captureFieldRef(`arg:${field().key}`)}
                            style={{ ...inputStyle, 'font-size': '0.82em' }}
                            type={field().type === 'number' || field().type === 'integer' ? 'number' : 'text'}
                            value={getArgsValue(field().key)}
                            onInput={(e) => setArgField(field().key, e.currentTarget.value, field().type)}
                            onBlur={() => doPush()}
                            disabled={ro()}
                            placeholder={field().type === 'object' || field().type === 'array' ? 'JSON or {{expression}}' : ''}
                          />
                        }>
                          {(_) => (
                            <select
                              style={{ ...inputStyle, 'font-size': '0.82em' }}
                              value={getArgsValue(field().key)}
                              onChange={(e) => { setArgField(field().key, e.currentTarget.value, 'boolean'); doPush(); }}
                              disabled={ro()}
                            >
                              <option value="">—</option>
                              <option value="true">true</option>
                              <option value="false">false</option>
                            </select>
                          )}
                        </Show>
                      }>
                        {(_) => (
                          <select
                            style={{ ...inputStyle, 'font-size': '0.82em' }}
                            value={getArgsValue(field().key)}
                            onChange={(e) => { setArgField(field().key, e.currentTarget.value, field().type); doPush(); }}
                            disabled={ro()}
                          >
                            <option value="">—</option>
                            <For each={field().enumValues!}>
                              {(v) => <option value={v}>{v}</option>}
                            </For>
                          </select>
                        )}
                      </Show>
                      }>
                        {(_) => (
                          <select
                            style={{ ...inputStyle, 'font-size': '0.82em' }}
                            value={getArgsValue(field().key)}
                            onChange={(e) => { setArgField(field().key, e.currentTarget.value, field().type); doPush(); }}
                            disabled={ro()}
                          >
                            <option value="">Select a channel…</option>
                            <For each={channelList()}>
                              {(ch) => <option value={ch.id}>{ch.name}{ch.provider ? ` (${ch.provider})` : ''}</option>}
                            </For>
                          </select>
                        )}
                      </Show>
                    </div>
                  );}}
                </Index>
              </div>
            )}
          </Show>
        </>);
      }

      case 'invoke_agent': {
        return (<>
          <div style={labelStyle}>Persona {missingMark('persona_id')}</div>
          <PersonaSelector
            value={props.getCfg().persona_id ?? ''}
            onChange={(id) => { doUpdate('persona_id', id); doPush(); }}
            personas={props.personas ?? []}
            disabled={ro()}
          />
          {exprLabel('Task', 'task', { required: true })}
          <textarea ref={captureFieldRef('task')} style={{ ...inputStyle, 'min-height': '60px', resize: 'both', 'overflow-x': 'auto', 'white-space': 'pre', border: fieldBorder('task') }} value={props.getCfg().task ?? ''} onInput={(e) => doUpdate('task', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <Switch
            checked={props.getCfg().async_exec ?? false}
            onChange={(checked: boolean) => { doUpdate('async_exec', checked); doPush(); }}
            disabled={ro()}
            class="flex items-center gap-2"
          >
            <SwitchControl><SwitchThumb /></SwitchControl>
            <SwitchLabel>Async Execution</SwitchLabel>
          </Switch>
          <Show when={!props.getCfg().async_exec}>
            <div style={labelStyle}>Timeout (seconds)</div>
            <input type="number" style={{ ...inputStyle, border: fieldBorder('timeout_secs') }} value={props.getCfg().timeout_secs ?? ''} onInput={(e) => { const v = e.currentTarget.value; const n = parseInt(v, 10); doUpdate('timeout_secs', v === '' || isNaN(n) ? null : n); }} onBlur={() => doPush()} disabled={ro()} min="1" placeholder="Leave empty for no limit" />
            <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
              Maximum time the agent can run.
            </div>
          </Show>

          {/* Per-step permission rules */}
          <div style={{ ...labelStyle, 'margin-top': '10px' }}>Permission Rules</div>
          <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '4px' }}>
            Tool approval policies for this agent. Leave empty to use defaults.
          </div>
          <PermissionRulesEditor
            rules={() => props.getCfg().permissions ?? []}
            setRules={(rules) => { doUpdate('permissions', rules); doPush(); }}
            toolDefinitions={props.toolDefinitions}
          />
          {/* Workflow Attachments */}
          <Show when={props.wfAttachments().length > 0}>
            <div style={{...labelStyle, 'margin-top': '10px' }}>Attachments</div>
            <div style={{'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '6px' }}>
              Select which workflow attachments this agent should have access to.
            </div>
            <For each={props.wfAttachments()}>
              {(att) => {
                const checked = () => (props.getCfg().attachments ?? []).includes(att.id);
                return (
                  <label style={{ display: 'flex', 'align-items': 'center', gap: '6px', 'font-size': '0.85em', padding: '3px 0', cursor: 'pointer' }}>
                    <input
                      type="checkbox"
                      checked={checked()}
                      onChange={() => {
                        const current: string[] = props.getCfg().attachments ?? [];
                        const updated = checked()
                          ? current.filter((id: string) => id !== att.id)
                          : [...current, att.id];
                        doUpdate('attachments', updated);
                        doPush();
                      }}
                      disabled={ro()}
                    />
                    <span style={{ 'font-weight': '500' }}>{att.filename}</span>
                    <span style={{ color: 'hsl(var(--muted-foreground))' }}>— {att.description}</span>
                  </label>
                );
              }}
            </For>
          </Show>
        </>);
      }

      case 'invoke_prompt': {
        // Find the selected persona's prompts
        const selectedPersona = () => (props.personas ?? []).find(p => p.id === props.getCfg().persona_id);
        const availablePrompts = (): PromptTemplateProp[] => selectedPersona()?.prompts ?? [];
        const selectedPrompt = () => availablePrompts().find(p => p.id === props.getCfg().prompt_id);

        // Target mode: 'new' = spawn new agent, 'existing' = send to existing agent
        // Use explicit null/undefined check — empty string means user selected existing mode but hasn't filled in the ID yet
        const targetMode = () => (props.getCfg().target_agent_id != null ? 'existing' : 'new') as 'new' | 'existing';

        // Derive schema info from the selected prompt's input_schema
        const schemaProperties = (): Record<string, any> => {
          const schema = selectedPrompt()?.input_schema;
          if (schema && typeof schema === 'object' && schema.properties) {
            return schema.properties;
          }
          return {};
        };
        const schemaRequired = (): string[] => {
          const schema = selectedPrompt()?.input_schema;
          if (schema && typeof schema === 'object' && Array.isArray(schema.required)) {
            return schema.required;
          }
          return [];
        };
        const schemaParamKeys = (): string[] => Object.keys(schemaProperties());
        const hasSchema = () => schemaParamKeys().length > 0;

        // Current parameters as entries
        const paramEntries = (): [string, string][] => {
          const params = props.getCfg().parameters ?? {};
          return Object.entries(params);
        };

        // Extra parameters not in schema
        const extraParamEntries = (): [string, string][] => {
          const schemaKeys = new Set(schemaParamKeys());
          return paramEntries().filter(([k]) => !schemaKeys.has(k));
        };

        // Template preview toggle
        const [showTemplatePreview, setShowTemplatePreview] = createSignal(false);

        return (<>
          <div style={labelStyle}>Persona {missingMark('persona_id')}</div>
          <PersonaSelector
            value={props.getCfg().persona_id ?? ''}
            onChange={(id) => {
              doUpdate('persona_id', id);
              doUpdate('prompt_id', '');
              doUpdate('parameters', {});
              doPush();
            }}
            personas={props.personas ?? []}
            disabled={ro()}
          />

          <div style={labelStyle}>Prompt Template {missingMark('prompt_id')}</div>
          <select
            style={{ ...inputStyle, border: fieldBorder('prompt_id') }}
            value={props.getCfg().prompt_id ?? ''}
            onChange={(e) => {
              const newPromptId = e.currentTarget.value;
              doUpdate('prompt_id', newPromptId);
              const prompt = availablePrompts().find(p => p.id === newPromptId);
              if (prompt?.input_schema?.properties) {
                const existingParams = props.getCfg().parameters ?? {};
                const newParams: Record<string, string> = {};
                for (const key of Object.keys(prompt.input_schema.properties)) {
                  newParams[key] = existingParams[key] ?? '';
                }
                doUpdate('parameters', newParams);
              }
              doPush();
            }}
            disabled={ro()}
          >
            <option value="">— Select a prompt —</option>
            <For each={availablePrompts()}>
              {(pt) => <option value={pt.id}>{pt.name}{pt.description ? ` — ${pt.description}` : ''}</option>}
            </For>
          </select>
          {/* Prompt description shown below the selector */}
          <Show when={selectedPrompt()?.description}>
            <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '2px', 'font-style': 'italic' }}>
              {selectedPrompt()!.description}
            </div>
          </Show>
          <Show when={!props.getCfg().persona_id}>
            <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
              Select a persona to see available prompts.
            </div>
          </Show>
          <Show when={props.getCfg().persona_id && availablePrompts().length === 0}>
            <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
              This persona has no prompt templates defined.
            </div>
          </Show>

          {/* Prompt template preview */}
          <Show when={selectedPrompt()?.template}>
            <button
              style={{ background: 'none', border: 'none', color: 'hsl(var(--primary))', cursor: 'pointer', 'font-size': '0.82em', padding: '2px 0', 'margin-bottom': '4px', display: 'flex', 'align-items': 'center', gap: '4px' }}
              onClick={() => setShowTemplatePreview(!showTemplatePreview())}
            >
              {showTemplatePreview() ? '▾ Hide' : '▸ Show'} template preview
            </button>
            <Show when={showTemplatePreview()}>
              <pre style={{
                'font-size': '0.78em',
                'line-height': '1.4',
                background: 'hsl(var(--muted) / 0.5)',
                border: '1px solid hsl(var(--border))',
                'border-radius': '4px',
                padding: '8px',
                'margin-bottom': '8px',
                'max-height': '150px',
                'overflow-y': 'auto',
                'white-space': 'pre-wrap',
                'word-break': 'break-word',
                color: 'hsl(var(--muted-foreground))',
              }}>{selectedPrompt()!.template}</pre>
            </Show>
          </Show>

          {/* Parameters — schema-driven when available */}
          <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center', 'justify-content': 'space-between', 'margin-top': '10px' }}>
            <span>Parameters</span>
            <Show when={!ro()}>
              <button
                style={{ background: 'none', border: 'none', color: 'hsl(var(--primary))', cursor: 'pointer', 'font-size': '0.85em' }}
                onClick={() => {
                  const params = { ...(props.getCfg().parameters ?? {}) };
                  params[''] = '';
                  doUpdate('parameters', params);
                  doPush();
                }}
              >+ Add Parameter</button>
            </Show>
          </div>
          <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '6px' }}>
            Key-value pairs passed to the prompt template. Values can use <code style={{ background: 'hsl(var(--primary) / 0.15)', padding: '1px 3px', 'border-radius': '2px' }}>{'{{expressions}}'}</code>.
          </div>

          {/* Schema-driven fields */}
          <Show when={hasSchema()}>
            <For each={schemaParamKeys()}>
              {(key) => {
                const prop = () => schemaProperties()[key] ?? {};
                const isRequired = () => schemaRequired().includes(key);
                const currentVal = () => (props.getCfg().parameters ?? {})[key] ?? '';
                return (
                  <div style={{ 'margin-bottom': '6px' }}>
                    <div style={{ display: 'flex', 'align-items': 'center', gap: '4px', 'margin-bottom': '2px' }}>
                      <span style={{ 'font-size': '0.85em', 'font-weight': '500' }}>{prop()['x-ui']?.label || key}</span>
                      <Show when={isRequired()}>
                        <span style={{ color: 'hsl(var(--destructive))', 'font-weight': '600' }}>*</span>
                      </Show>
                      <Show when={prop().type}>
                        <span style={{ 'font-size': '0.72em', color: 'hsl(var(--muted-foreground))', background: 'hsl(var(--muted) / 0.5)', padding: '1px 4px', 'border-radius': '3px' }}>{prop().type}</span>
                      </Show>
                    </div>
                    <Show when={prop().description}>
                      <div style={{ 'font-size': '0.78em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '2px' }}>{prop().description}</div>
                    </Show>
                    <input
                      style={{ ...inputStyle, border: isRequired() && !currentVal() ? '1px solid hsl(var(--destructive) / 0.5)' : undefined }}
                      value={currentVal()}
                      placeholder={prop().default != null ? `Default: ${prop().default}` : 'value or {{expression}}'}
                      onInput={(e) => {
                        const params = { ...(props.getCfg().parameters ?? {}) };
                        params[key] = e.currentTarget.value;
                        doUpdate('parameters', params);
                      }}
                      onBlur={() => doPush()}
                      disabled={ro()}
                    />
                  </div>
                );
              }}
            </For>
            {/* Extra params not in schema */}
            <For each={extraParamEntries()}>
              {([key, val], idx) => (
                <div style={{ display: 'flex', gap: '4px', 'margin-bottom': '4px', 'align-items': 'center' }}>
                  <input
                    style={{ ...inputStyle, flex: '1' }}
                    value={key}
                    placeholder="key"
                    onInput={(e) => {
                      const params = { ...(props.getCfg().parameters ?? {}) };
                      const allEntries = Object.entries(params);
                      const schemaKeys = new Set(schemaParamKeys());
                      const extraEntries = allEntries.filter(([k]) => !schemaKeys.has(k));
                      const newKey = e.currentTarget.value;
                      // Rebuild params: schema keys first, then extras with updated key
                      const newParams: Record<string, string> = {};
                      for (const sk of schemaParamKeys()) newParams[sk] = params[sk] ?? '';
                      for (let i = 0; i < extraEntries.length; i++) {
                        if (i === idx()) newParams[newKey] = val;
                        else newParams[extraEntries[i][0]] = extraEntries[i][1] as string;
                      }
                      doUpdate('parameters', newParams);
                    }}
                    onBlur={() => doPush()}
                    disabled={ro()}
                  />
                  <input
                    style={{ ...inputStyle, flex: '2' }}
                    value={val}
                    placeholder="value or {{expression}}"
                    onInput={(e) => {
                      const params = { ...(props.getCfg().parameters ?? {}) };
                      params[key] = e.currentTarget.value;
                      doUpdate('parameters', params);
                    }}
                    onBlur={() => doPush()}
                    disabled={ro()}
                  />
                  <Show when={!ro()}>
                    <button
                      style={{ background: 'none', border: 'none', color: 'hsl(var(--destructive))', cursor: 'pointer', padding: '2px' }}
                      onClick={() => {
                        const params = { ...(props.getCfg().parameters ?? {}) };
                        delete params[key];
                        doUpdate('parameters', params);
                        doPush();
                      }}
                      title="Remove parameter"
                    >✕</button>
                  </Show>
                </div>
              )}
            </For>
          </Show>

          {/* Fallback: plain K/V editor when no schema */}
          <Show when={!hasSchema()}>
            <Show when={schemaParamKeys().length === 0 && paramEntries().length === 0 && selectedPrompt()}>
              <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '6px', 'font-style': 'italic' }}>
                This prompt template has no input schema. Add parameters manually if needed.
              </div>
            </Show>
            <For each={paramEntries()}>
              {([key, val], idx) => (
                <div style={{ display: 'flex', gap: '4px', 'margin-bottom': '4px', 'align-items': 'center' }}>
                  <input
                    style={{ ...inputStyle, flex: '1' }}
                    value={key}
                    placeholder="key"
                    onInput={(e) => {
                      const params = { ...(props.getCfg().parameters ?? {}) };
                      const entries = Object.entries(params);
                      const newKey = e.currentTarget.value;
                      entries[idx()] = [newKey, val];
                      const newParams: Record<string, string> = {};
                      for (const [k, v] of entries) newParams[k] = v as string;
                      doUpdate('parameters', newParams);
                    }}
                    onBlur={() => doPush()}
                    disabled={ro()}
                  />
                  <input
                    style={{ ...inputStyle, flex: '2' }}
                    value={val}
                    placeholder="value or {{expression}}"
                    onInput={(e) => {
                      const params = { ...(props.getCfg().parameters ?? {}) };
                      params[key] = e.currentTarget.value;
                      doUpdate('parameters', params);
                    }}
                    onBlur={() => doPush()}
                    disabled={ro()}
                  />
                  <Show when={!ro()}>
                    <button
                      style={{ background: 'none', border: 'none', color: 'hsl(var(--destructive))', cursor: 'pointer', padding: '2px' }}
                      onClick={() => {
                        const params = { ...(props.getCfg().parameters ?? {}) };
                        delete params[key];
                        doUpdate('parameters', params);
                        doPush();
                      }}
                      title="Remove parameter"
                    >✕</button>
                  </Show>
                </div>
              )}
            </For>
          </Show>

          {/* Target mode: new agent or existing agent */}
          <div style={{ ...labelStyle, 'margin-top': '10px' }}>Target</div>
          <select
            style={inputStyle}
            value={targetMode()}
            onChange={(e) => {
              if (e.currentTarget.value === 'new') {
                doUpdate('target_agent_id', null);
              } else {
                doUpdate('target_agent_id', '');
              }
              doPush();
            }}
            disabled={ro()}
          >
            <option value="new">Start new agent</option>
            <option value="existing">Send to existing agent</option>
          </select>
          <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
            {targetMode() === 'new'
              ? 'Spawns a new agent with the rendered prompt as its task.'
              : 'Sends the rendered prompt as a message to a running agent.'}
          </div>

          <Show when={targetMode() === 'existing'}>
            {exprLabel('Target Agent ID', 'target_agent_id', { required: true })}
            <input
              ref={captureFieldRef('target_agent_id')}
              style={{ ...inputStyle, border: fieldBorder('target_agent_id') }}
              value={props.getCfg().target_agent_id ?? ''}
              placeholder="e.g. {{steps.spawn_step.agent_id}}"
              onInput={(e) => doUpdate('target_agent_id', e.currentTarget.value)}
              onBlur={() => doPush()}
              disabled={ro()}
            />
            <Switch
              checked={props.getCfg().auto_create ?? false}
              onChange={(checked: boolean) => { doUpdate('auto_create', checked); doPush(); }}
              disabled={ro()}
              class="flex items-center gap-2"
              style={{ 'margin-top': '8px' }}
            >
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Auto-create if not found</SwitchLabel>
            </Switch>
            <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
              {props.getCfg().auto_create
                ? 'If the target agent doesn\'t exist, a new agent will be spawned with the rendered prompt.'
                : 'Step will fail if the target agent is not found.'}
            </div>
          </Show>

          {/* Agent settings: shown for new-agent mode OR existing-agent with auto-create */}
          <Show when={targetMode() === 'new' || props.getCfg().auto_create}>
            <Switch
              checked={props.getCfg().async_exec ?? false}
              onChange={(checked: boolean) => { doUpdate('async_exec', checked); doPush(); }}
              disabled={ro()}
              class="flex items-center gap-2"
            >
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Async Execution</SwitchLabel>
            </Switch>
            <Show when={!props.getCfg().async_exec}>
              <div style={labelStyle}>Timeout (seconds)</div>
              <input type="number" style={{ ...inputStyle, border: fieldBorder('timeout_secs') }} value={props.getCfg().timeout_secs ?? ''} onInput={(e) => { const v = e.currentTarget.value; const n = parseInt(v, 10); doUpdate('timeout_secs', v === '' || isNaN(n) ? null : n); }} onBlur={() => doPush()} disabled={ro()} min="1" placeholder="Leave empty for no limit" />
              <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
                Maximum time the agent can run.
              </div>
            </Show>

            {/* Per-step permission rules */}
            <div style={{ ...labelStyle, 'margin-top': '10px' }}>Permission Rules</div>
            <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '4px' }}>
              Tool approval policies for this agent. Leave empty to use defaults.
            </div>
            <PermissionRulesEditor
              rules={() => props.getCfg().permissions ?? []}
              setRules={(rules) => { doUpdate('permissions', rules); doPush(); }}
              toolDefinitions={props.toolDefinitions}
            />
          </Show>
        </>);
      }

      case 'feedback_gate':
        return (<>
          {exprLabel('Prompt', 'prompt', { required: true })}
          <textarea ref={captureFieldRef('prompt')} style={{ ...inputStyle, 'min-height': '80px', resize: 'vertical', border: fieldBorder('prompt') }} value={props.getCfg().prompt ?? ''} onInput={(e) => doUpdate('prompt', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px', 'margin-bottom': '4px' }}>
            Use <code style={{ background: 'hsl(var(--primary) / 0.15)', padding: '1px 3px', 'border-radius': '2px' }}>{'{{variable_name}}'}</code> to reference workflow variables
          </div>
          <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center', 'justify-content': 'space-between' }}>
            <span>Choices</span>
            <Show when={!ro()}>
              <button
                style={{ background: 'hsl(142 71% 45%)', color: 'hsl(var(--background))', border: 'none', 'border-radius': '4px', padding: '2px 8px', cursor: 'pointer', 'font-size': '0.8em' }}
                onClick={() => {
                  const current = Array.isArray(props.getCfg().choices) ? [...props.getCfg().choices] : [];
                  current.push('');
                  doUpdate('choices', current);
                  doPush();
                }}
              >+ Add</button>
            </Show>
          </div>
          <For each={Array.isArray(props.getCfg().choices) ? props.getCfg().choices : []}>
            {(choice: string, idx) => (
              <div style={{ display: 'flex', 'align-items': 'center', gap: '4px', 'margin-bottom': '3px' }}>
                <input
                  style={{ ...inputStyle, flex: '1' }}
                  value={choice}
                  placeholder="Choice label"
                  onInput={(e) => {
                    const current = [...(props.getCfg().choices ?? [])];
                    current[idx()] = e.currentTarget.value;
                    doUpdate('choices', current);
                  }}
                  onBlur={() => doPush()}
                  disabled={ro()}
                />
                <Show when={!ro()}>
                  <button
                    style={{ background: 'transparent', color: 'hsl(var(--destructive))', border: 'none', cursor: 'pointer', 'font-size': '0.8em', padding: '2px 4px' }}
                    onClick={() => {
                      const current = [...(props.getCfg().choices ?? [])];
                      current.splice(idx(), 1);
                      doUpdate('choices', current);
                      doPush();
                    }}
                  >✕</button>
                </Show>
              </div>
            )}
          </For>
          <Switch
            checked={props.getCfg().allow_freeform ?? false}
            onChange={(checked: boolean) => { doUpdate('allow_freeform', checked); doPush(); }}
            disabled={ro()}
            class="flex items-center gap-2"
          >
            <SwitchControl><SwitchThumb /></SwitchControl>
            <SwitchLabel>Allow Freeform</SwitchLabel>
          </Switch>
        </>);

      case 'delay': {
        const totalSecs = () => props.getCfg().duration_secs ?? 60;
        const days = () => Math.floor(totalSecs() / 86400);
        const hours = () => Math.floor((totalSecs() % 86400) / 3600);
        const minutes = () => Math.floor((totalSecs() % 3600) / 60);
        const seconds = () => totalSecs() % 60;

        function setDuration(d: number, h: number, m: number, s: number) {
          doUpdate('duration_secs', d * 86400 + h * 3600 + m * 60 + s);
        }

        const unitInput = (label: string, value: () => number, max: number, setter: (v: number) => void) => (
          <div style={{ display: 'flex', 'flex-direction': 'column', 'align-items': 'center', flex: '1' }}>
            <input
              style={{ ...inputStyle, width: '100%', 'text-align': 'center', padding: '4px 2px' }}
              type="number" min="0" max={max}
              value={value()}
              onInput={(e) => setter(Math.max(0, parseInt(e.currentTarget.value) || 0))}
              onBlur={() => doPush()}
              disabled={ro()}
            />
            <span style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px' }}>{label}</span>
          </div>
        );

        return (<>
          <div style={labelStyle}>Duration</div>
          <div style={{ display: 'flex', gap: '6px', 'align-items': 'flex-start' }}>
            {unitInput('days', days, 999, (v) => setDuration(v, hours(), minutes(), seconds()))}
            {unitInput('hours', hours, 23, (v) => setDuration(days(), v, minutes(), seconds()))}
            {unitInput('minutes', minutes, 59, (v) => setDuration(days(), hours(), v, seconds()))}
            {unitInput('seconds', seconds, 59, (v) => setDuration(days(), hours(), minutes(), v))}
          </div>
          <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', 'margin-top': '2px' }}>
            Total: {totalSecs().toLocaleString()} seconds
          </div>
        </>);
      }

      case 'set_variable': {
        const getAssignments = (): { variable: string; value: string; operation: string }[] => {
          const raw = props.getCfg().assignments;
          return Array.isArray(raw) ? raw : [];
        };

        function updateAssignment(index: number, field: string, value: string) {
          const current = [...getAssignments()];
          current[index] = { ...current[index], [field]: value };
          doUpdate('assignments', current);
        }

        function addAssignment() {
          doUpdate('assignments', [...getAssignments(), { variable: '', value: '', operation: 'set' }]);
          doPush();
        }

        function removeAssignment(index: number) {
          const current = [...getAssignments()];
          current.splice(index, 1);
          doUpdate('assignments', current);
          doPush();
        }

        return (<>
          <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center' }}>
            <span style={{ flex: '1' }}>Variable Assignments {missingMark('assignments')}</span>
            <Show when={!ro()}>
              <button
                style={{ background: 'hsl(142 71% 45%)', color: 'hsl(var(--background))', border: 'none', 'border-radius': '4px', padding: '2px 8px', cursor: 'pointer', 'font-size': '0.8em' }}
                onClick={addAssignment}
              >+ Add</button>
            </Show>
          </div>
          <For each={getAssignments()}>
            {(assign, idx) => (
              <div style={{ border: '1px solid hsl(var(--border))', 'border-radius': '6px', padding: '8px', 'margin-bottom': '8px', background: 'hsl(var(--muted) / 0.4)' }}>
                <div style={{ display: 'flex', 'align-items': 'center', gap: '4px', 'margin-bottom': '4px' }}>
                  <span style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))' }}>#{idx() + 1}</span>
                  <span style={{ flex: '1' }} />
                  <Show when={!ro()}>
                    <button
                      style={{ background: 'transparent', color: 'hsl(var(--destructive))', border: 'none', cursor: 'pointer', 'font-size': '0.8em', padding: '2px 4px' }}
                      onClick={() => removeAssignment(idx())}
                    >✕</button>
                  </Show>
                </div>
                <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '2px' }}>Variable Name</div>
                <select
                  style={{ ...inputStyle, 'margin-bottom': '6px' }}
                  value={assign.variable}
                  onChange={(e) => { updateAssignment(idx(), 'variable', e.currentTarget.value); doPush(); }}
                  disabled={ro()}
                >
                  <option value="">— select variable —</option>
                  <For each={props.variables()}>
                    {(v) => <option value={v.name}>{v.name}{v.description ? ` — ${v.description}` : ''}</option>}
                  </For>
                  <Show when={assign.variable && !props.variables().some(v => v.name === assign.variable)}>
                    <option value={assign.variable}>{assign.variable} (not defined)</option>
                  </Show>
                </select>
                <div style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '2px' }}>Operation</div>
                <select
                  style={{ ...inputStyle, 'margin-bottom': '6px' }}
                  value={assign.operation}
                  onChange={(e) => { updateAssignment(idx(), 'operation', e.currentTarget.value); doPush(); }}
                  disabled={ro()}
                >
                  <option value="set">Set (overwrite)</option>
                  <option value="append_list">Append to List</option>
                  <option value="merge_map">Merge into Map</option>
                </select>
                <div style={{ ...labelStyle, display: 'flex', 'align-items': 'center', 'margin-bottom': '2px' }}>
                  <span style={{ flex: '1', 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))' }}>Value Expression</span>
                  <Show when={!ro()}>
                    {props.renderExpressionHelper((newVal) => {
                      updateAssignment(idx(), 'value', newVal);
                    }, () => fieldInputRefs[`assign:${idx()}`])}
                  </Show>
                </div>
                <input
                  ref={captureFieldRef(`assign:${idx()}`)}
                  style={inputStyle}
                  placeholder="e.g. {{steps.fetch.outputs.data}}"
                  value={assign.value}
                  onInput={(e) => updateAssignment(idx(), 'value', e.currentTarget.value)}
                  onBlur={() => doPush()}
                  disabled={ro()}
                />
              </div>
            )}
          </For>
          <Show when={getAssignments().length === 0}>
            <div style={{ 'font-size': '0.8em', color: 'hsl(38 92% 50%)', 'text-align': 'center', padding: '12px' }}>
              No assignments — click "+ Add" to create one.
            </div>
          </Show>
        </>);
      }

      case 'event_gate':
        return (<>
          <div style={labelStyle}>Event Topic {missingMark('topic')}</div>
          {renderTopicSelector('topic', { borderOverride: fieldBorder('topic') })}
          <div style={labelStyle}>Payload Filter</div>
          {renderFilterBuilder('filter')}
          <div style={labelStyle}>Timeout (seconds)</div>
          <input style={inputStyle} type="number" placeholder="Optional" value={props.getCfg().timeout_secs ?? ''} onInput={(e) => doUpdate('timeout_secs', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
        </>);

      case 'signal_agent':
        return (<>
          <div style={labelStyle}>Target Type</div>
          <select style={inputStyle} value={props.getCfg().target_type ?? 'session'} onChange={(e) => { doUpdate('target_type', e.currentTarget.value); doPush(); }} disabled={ro()}>
            <option value="session">Session</option>
            <option value="agent">Agent</option>
          </select>
          <div style={labelStyle}>Target ID</div>
          <input style={inputStyle} value={props.getCfg().target_id ?? ''} onInput={(e) => doUpdate('target_id', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          {exprLabel('Content', 'content', { required: true })}
          <textarea ref={captureFieldRef('content')} style={{ ...inputStyle, 'min-height': '60px', resize: 'vertical', border: fieldBorder('content') }} value={props.getCfg().content ?? ''} onInput={(e) => doUpdate('content', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
        </>);

      case 'launch_workflow':
        return (<>
          <div style={labelStyle}>Workflow Name {missingMark('workflow_name')}</div>
          <input style={{ ...inputStyle, border: fieldBorder('workflow_name') }} value={props.getCfg().workflow_name ?? ''} onInput={(e) => doUpdate('workflow_name', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <div style={labelStyle}>Inputs (JSON)</div>
          <textarea
            style={{ ...inputStyle, 'min-height': '60px', resize: 'vertical' }}
            value={typeof props.getCfg().inputs === 'object' ? JSON.stringify(props.getCfg().inputs, null, 2) : (props.getCfg().inputs ?? '{}')}
            onInput={(e) => { try { doUpdate('inputs', JSON.parse(e.currentTarget.value)); } catch { /* keep raw */ } }}
            onBlur={() => doPush()} disabled={ro()}
          />
        </>);

      case 'schedule_task': {
        const actionType = () => (props.getCfg().action_type || 'emit_event') as string;
        return (<>
          <div style={labelStyle}>Task Name {missingMark('task_name')}</div>
          <input style={{ ...inputStyle, border: fieldBorder('task_name') }} value={props.getCfg().task_name ?? ''} onInput={(e) => doUpdate('task_name', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />

          <div style={labelStyle}>Description</div>
          <input style={inputStyle} value={props.getCfg().task_description ?? ''} onInput={(e) => doUpdate('task_description', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="Optional description" />

          <div style={{ ...labelStyle, 'margin-top': '10px' }}>Schedule</div>
          <div style={{ 'font-size': '0.72em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '4px' }}>Leave empty for a one-time (immediate) task, or set a cron expression for recurring.</div>
          <CronBuilder
            value={(props.getCfg().schedule ?? '') as string}
            onChange={(v) => { doUpdate('schedule', v); doPush(); }}
            disabled={ro()}
          />

          <div style={{ ...labelStyle, 'margin-top': '12px' }}>Action Type {missingMark('action_type')}</div>
          <select style={{ ...inputStyle, cursor: 'pointer' }} value={actionType()} onChange={(e) => { doUpdate('action_type', e.currentTarget.value); doPush(); }} disabled={ro()}>
            <option value="emit_event">Emit Event</option>
            <option value="send_message">Send Message</option>
            <option value="http_webhook">HTTP Webhook</option>
            <option value="invoke_agent">Invoke Agent</option>
            <option value="call_tool">Call Tool</option>
            <option value="launch_workflow">Launch Workflow</option>
          </select>

          {/* Emit Event */}
          <Show when={actionType() === 'emit_event'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Topic</div>
            <TopicSelector
              value={props.getCfg().action_topic ?? ''}
              onChange={(t) => { doUpdate('action_topic', t); doPush(); }}
              topics={props.eventTopics ?? []}
            />
            <div style={labelStyle}>Payload (JSON)</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical', 'font-family': 'monospace' }}
              value={props.getCfg().action_payload ?? '{}'}
              onInput={(e) => doUpdate('action_payload', e.currentTarget.value)}
              onBlur={() => doPush()} disabled={ro()}
              placeholder='{"key": "value"}'
            />
          </Show>

          {/* Send Message */}
          <Show when={actionType() === 'send_message'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Session ID</div>
            <input style={inputStyle} value={props.getCfg().action_session_id ?? ''} onInput={(e) => doUpdate('action_session_id', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="Target session ID" />
            <div style={labelStyle}>Content</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical' }}
              value={props.getCfg().action_content ?? ''}
              onInput={(e) => doUpdate('action_content', e.currentTarget.value)}
              onBlur={() => doPush()} disabled={ro()}
              placeholder="Message content"
            />
          </Show>

          {/* HTTP Webhook */}
          <Show when={actionType() === 'http_webhook'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Method</div>
            <select style={{ ...inputStyle, cursor: 'pointer' }} value={props.getCfg().action_method ?? 'POST'} onChange={(e) => { doUpdate('action_method', e.currentTarget.value); doPush(); }} disabled={ro()}>
              <option value="GET">GET</option>
              <option value="POST">POST</option>
              <option value="PUT">PUT</option>
              <option value="DELETE">DELETE</option>
              <option value="PATCH">PATCH</option>
            </select>
            <div style={labelStyle}>URL</div>
            <input style={inputStyle} value={props.getCfg().action_url ?? ''} onInput={(e) => doUpdate('action_url', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="https://example.com/webhook" />
            <div style={labelStyle}>Body (optional JSON)</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical', 'font-family': 'monospace' }}
              value={props.getCfg().action_body ?? ''}
              onInput={(e) => doUpdate('action_body', e.currentTarget.value)}
              onBlur={() => doPush()} disabled={ro()}
              placeholder='{"key": "value"}'
            />
          </Show>

          {/* Invoke Agent */}
          <Show when={actionType() === 'invoke_agent'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Persona</div>
            <PersonaSelector
              value={props.getCfg().action_persona_id ?? ''}
              onChange={(v) => { doUpdate('action_persona_id', v); doPush(); }}
              personas={props.personas ?? []}
            />
            <div style={labelStyle}>Friendly Name</div>
            <input style={inputStyle} value={props.getCfg().action_friendly_name ?? ''} onInput={(e) => doUpdate('action_friendly_name', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="Optional display name" />
            <div style={labelStyle}>Task</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical' }}
              value={props.getCfg().action_task ?? ''}
              onInput={(e) => doUpdate('action_task', e.currentTarget.value)}
              onBlur={() => doPush()} disabled={ro()}
              placeholder="Task description for the agent"
            />
            <div style={labelStyle}>Timeout (seconds)</div>
            <input style={inputStyle} type="number" value={props.getCfg().action_timeout_secs ?? 300} onInput={(e) => doUpdate('action_timeout_secs', parseInt(e.currentTarget.value) || 300)} onBlur={() => doPush()} disabled={ro()} />
          </Show>

          {/* Call Tool */}
          <Show when={actionType() === 'call_tool'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Tool</div>
            <select style={{ ...inputStyle, cursor: 'pointer' }} value={props.getCfg().action_tool_id ?? ''} onChange={(e) => { doUpdate('action_tool_id', e.currentTarget.value); doUpdate('action_arguments', {}); doPush(); }} disabled={ro()}>
              <option value="">Select a tool…</option>
              <For each={props.toolDefinitions ?? []}>
                {(tool) => <option value={tool.id}>{tool.name || tool.id}</option>}
              </For>
            </select>
            <div style={labelStyle}>Arguments (JSON)</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical', 'font-family': 'monospace' }}
              value={typeof props.getCfg().action_arguments === 'object' ? JSON.stringify(props.getCfg().action_arguments, null, 2) : (props.getCfg().action_arguments ?? '{}')}
              onInput={(e) => { try { doUpdate('action_arguments', JSON.parse(e.currentTarget.value)); } catch { /* keep raw */ } }}
              onBlur={() => doPush()} disabled={ro()}
              placeholder='{"key": "value"}'
            />
          </Show>

          {/* Launch Workflow */}
          <Show when={actionType() === 'launch_workflow'}>
            <div style={{ ...labelStyle, 'margin-top': '8px' }}>Workflow Definition</div>
            <input style={inputStyle} value={props.getCfg().action_workflow ?? ''} onInput={(e) => doUpdate('action_workflow', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="Workflow name" />
            <div style={labelStyle}>Version (optional)</div>
            <input style={inputStyle} value={props.getCfg().action_workflow_version ?? ''} onInput={(e) => doUpdate('action_workflow_version', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} placeholder="Leave empty for latest" />
            <div style={labelStyle}>Inputs (JSON)</div>
            <textarea
              style={{ ...inputStyle, 'min-height': '50px', resize: 'vertical', 'font-family': 'monospace' }}
              value={props.getCfg().action_workflow_inputs ?? '{}'}
              onInput={(e) => doUpdate('action_workflow_inputs', e.currentTarget.value)}
              onBlur={() => doPush()} disabled={ro()}
              placeholder='{"key": "value"}'
            />
          </Show>
        </>);
      }

      case 'branch':
        return (<>
          {exprLabel('Condition', 'condition', { required: true })}
          <textarea ref={captureFieldRef('condition')} style={{ ...inputStyle, 'min-height': '40px', resize: 'vertical', border: fieldBorder('condition') }} value={props.getCfg().condition ?? ''} onInput={(e) => doUpdate('condition', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <div style={{ 'font-size': '0.7em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            Drag from <span style={{ color: 'hsl(142 71% 45%)' }}>green port</span> for <b>then</b>, <span style={{ color: 'hsl(var(--destructive))' }}>red port</span> for <b>else</b>
          </div>
        </>);

      case 'for_each':
        return (<>
          {exprLabel('Collection', 'collection', { required: true })}
          <input ref={captureFieldRef('collection')} style={{ ...inputStyle, border: fieldBorder('collection') }} value={props.getCfg().collection ?? ''} onInput={(e) => doUpdate('collection', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <div style={labelStyle}>Item Variable</div>
          <input style={inputStyle} value={props.getCfg().item_var ?? 'item'} onInput={(e) => doUpdate('item_var', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
        </>);

      case 'while':
        return (<>
          {exprLabel('Condition', 'condition', { required: true })}
          <textarea ref={captureFieldRef('condition')} style={{ ...inputStyle, 'min-height': '40px', resize: 'vertical', border: fieldBorder('condition') }} value={props.getCfg().condition ?? ''} onInput={(e) => doUpdate('condition', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()} />
          <div style={labelStyle}>Max Iterations</div>
          <input style={inputStyle} type="number" value={props.getCfg().max_iterations ?? 100} onInput={(e) => doUpdate('max_iterations', parseInt(e.currentTarget.value) || 0)} onBlur={() => doPush()} disabled={ro()} />
        </>);

      case 'manual': {
        const input_schema: WorkflowVariable[] = props.getCfg().input_schema ?? [];
        const [showInputSchemaDialog, setShowInputSchemaDialog] = createSignal(false);
        const [localInputVars, setLocalInputVars] = createSignal<WorkflowVariable[]>(
          JSON.parse(JSON.stringify(input_schema))
        );

        function inputSchemaAddVar() {
          setLocalInputVars(prev => [...prev, {
            name: `input_${prev.length + 1}`,
            varType: 'string' as const,
            description: '',
            required: false,
            defaultValue: '',
            enumValues: [],
          }]);
        }
        function inputSchemaRemoveVar(idx: number) {
          setLocalInputVars(prev => prev.filter((_, i) => i !== idx));
        }
        function inputSchemaUpdateVar(idx: number, field: keyof WorkflowVariable, value: any) {
          setLocalInputVars(prev => prev.map((v, i) => i === idx ? { ...v, [field]: value } : v));
        }
        function inputSchemaAddNestedProp(varIdx: number) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx) return v;
            const props = v.properties ? [...v.properties] : [];
            props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
            return { ...v, properties: props };
          }));
        }
        function inputSchemaUpdateNestedProp(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx || !v.properties) return v;
            const props = v.properties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
            return { ...v, properties: props };
          }));
        }
        function inputSchemaRemoveNestedProp(varIdx: number, propIdx: number) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx || !v.properties) return v;
            return { ...v, properties: v.properties.filter((_, pi) => pi !== propIdx) };
          }));
        }
        function inputSchemaAddItemProp(varIdx: number) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx) return v;
            const props = v.itemProperties ? [...v.itemProperties] : [];
            props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
            return { ...v, itemProperties: props };
          }));
        }
        function inputSchemaUpdateItemProp(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx || !v.itemProperties) return v;
            const props = v.itemProperties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
            return { ...v, itemProperties: props };
          }));
        }
        function inputSchemaRemoveItemProp(varIdx: number, propIdx: number) {
          setLocalInputVars(prev => prev.map((v, i) => {
            if (i !== varIdx || !v.itemProperties) return v;
            return { ...v, itemProperties: v.itemProperties.filter((_, pi) => pi !== propIdx) };
          }));
        }

        function handleInputSchemaOk() {
          doUpdate('input_schema', localInputVars());
          doPush();
          setShowInputSchemaDialog(false);
        }

        return (<>
          <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            <Play size={14} style={{ display: 'inline', 'vertical-align': 'middle' }} /> Manual trigger — workflow starts when launched by a user or API call.
          </div>
          <div style={{ 'margin-top': '8px' }}>
            <div style={labelStyle}>Input Schema ({input_schema.length} field{input_schema.length !== 1 ? 's' : ''})</div>
            <Show when={input_schema.length > 0}>
              <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-bottom': '4px' }}>
                <For each={input_schema}>
                  {(v) => <div>• {v.name}: {v.varType}{v.required ? ' (required)' : ''}</div>}
                </For>
              </div>
            </Show>
            <Show when={!ro()}>
              <button
                class="wf-btn-secondary"
                style="padding:4px 12px;font-size:0.82em;"
                onClick={() => {
                  setLocalInputVars(JSON.parse(JSON.stringify(props.getCfg().input_schema ?? [])));
                  setShowInputSchemaDialog(true);
                }}
              >Edit Input Schema</button>
            </Show>
          </div>


          {/* Input Schema Editor Dialog */}
          <Dialog open={!!showInputSchemaDialog()} onOpenChange={(open: boolean) => { if (!open) setShowInputSchemaDialog(false); }}>
            <DialogContent class="max-w-lg">
                <DialogHeader>
                  <DialogTitle>Trigger Input Schema</DialogTitle>
                </DialogHeader>

                <div style={{ display: 'flex', 'flex-direction': 'column', gap: '12px' }}>
                  <Index each={localInputVars()}>
                    {(v, idx) => (
                      <div class="wf-var-card">
                        <div class="wf-var-card-header">
                          <input
                            class="wf-launch-input"
                            value={v().name}
                            onInput={(e) => inputSchemaUpdateVar(idx, 'name', e.currentTarget.value)}
                            placeholder="Input name"
                          />
                          <button
                            onClick={() => inputSchemaRemoveVar(idx)}
                            style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '1em', padding: '0 4px' }}
                            title="Delete input"
                          >✕</button>
                        </div>

                        <div class="wf-var-grid">
                          <div class="wf-var-field">
                            <label>Type</label>
                            <select
                              class="wf-launch-input"
                              value={v().varType}
                              onChange={(e) => inputSchemaUpdateVar(idx, 'varType', e.currentTarget.value)}
                            >
                              <For each={TYPE_VALUES}>
                                {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                              </For>
                            </select>
                          </div>
                          <div class="wf-var-field" style={{ 'justify-content': 'flex-end' }}>
                            <label style={{ display: 'flex', 'align-items': 'center', gap: '6px', cursor: 'pointer' }}>
                              <input
                                type="checkbox"
                                checked={v().required}
                                onChange={(e) => inputSchemaUpdateVar(idx, 'required', e.currentTarget.checked)}
                              />
                              Required
                            </label>
                          </div>
                          <div class="wf-var-field full-width">
                            <label>Description</label>
                            <textarea
                              class="wf-launch-input"
                              value={v().description}
                              onInput={(e) => inputSchemaUpdateVar(idx, 'description', e.currentTarget.value)}
                              placeholder="Input description"
                              rows={1}
                              style={{ resize: 'vertical' }}
                            />
                          </div>
                          <div class="wf-var-field full-width">
                            <label>Default value</label>
                            {v().varType === 'boolean' ? (
                              <label style={{ display: 'flex', 'align-items': 'center', gap: '6px', cursor: 'pointer', 'font-size': '0.9em', padding: '4px 0' }}>
                                <input
                                  type="checkbox"
                                  checked={v().defaultValue === 'true'}
                                  onChange={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.checked ? 'true' : 'false')}
                                />
                                {v().defaultValue === 'true' ? 'true' : 'false'}
                              </label>
                            ) : v().varType === 'number' ? (
                              <input
                                class="wf-launch-input"
                                type="number"
                                value={v().defaultValue}
                                onInput={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.value)}
                                placeholder="0"
                              />
                            ) : (
                              <input
                                class="wf-launch-input"
                                type="text"
                                value={v().defaultValue}
                                onInput={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.value)}
                                placeholder="default"
                              />
                            )}
                          </div>
                        </div>

                        {/* Widget override */}
                        <div class="wf-var-grid">
                          <div class="wf-var-field full-width">
                            <label>Widget</label>
                            <select
                              class="wf-launch-input"
                              value={v().xUi?.widget ?? ''}
                              onChange={(e) => {
                                const w = e.currentTarget.value;
                                inputSchemaUpdateVar(idx, 'xUi', w ? { ...v().xUi, widget: w } : undefined);
                              }}
                            >
                              <option value="">(default)</option>
                              <Show when={v().varType === 'string'}>
                                <option value="textarea">Textarea</option>
                                <option value="code-editor">Code Editor</option>
                                <option value="password">Password</option>
                                <option value="date">Date</option>
                                <option value="color-picker">Color Picker</option>
                              </Show>
                              <Show when={v().varType === 'number'}>
                                <option value="slider">Slider</option>
                              </Show>
                            </select>
                          </div>
                          <Show when={v().xUi?.widget === 'textarea' || v().xUi?.widget === 'code-editor'}>
                            <div class="wf-var-field">
                              <label>Rows</label>
                              <input
                                class="wf-launch-input"
                                type="number"
                                value={v().xUi?.rows ?? ''}
                                min={1}
                                max={50}
                                onInput={(e) => {
                                  const rows = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                                  inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, rows });
                                }}
                                placeholder="4"
                              />
                            </div>
                          </Show>
                          <Show when={v().xUi?.widget === 'slider'}>
                            <div class="wf-var-field">
                              <label>Step</label>
                              <input
                                class="wf-launch-input"
                                type="number"
                                value={v().xUi?.step ?? ''}
                                min={0}
                                onInput={(e) => {
                                  const step = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                                  inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, step });
                                }}
                                placeholder="1"
                              />
                            </div>
                          </Show>
                        </div>

                        {/* Conditional visibility */}
                        <div class="wf-var-grid">
                          <div class="wf-var-field">
                            <label>Visible when</label>
                            <select
                              class="wf-launch-input"
                              value={v().xUi?.condition?.field ?? ''}
                              onChange={(e) => {
                                const field = e.currentTarget.value;
                                if (!field) {
                                  const { condition, ...rest } = v().xUi ?? {};
                                  inputSchemaUpdateVar(idx, 'xUi', Object.keys(rest).length > 0 ? rest : undefined);
                                } else {
                                  inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, condition: { field, eq: true } });
                                }
                              }}
                            >
                              <option value="">(always visible)</option>
                              <For each={localInputVars().filter((_, i) => i !== idx)}>
                                {(other) => <option value={other.name}>{other.name}</option>}
                              </For>
                            </select>
                          </div>
                          <Show when={v().xUi?.condition?.field}>
                            <div class="wf-var-field">
                              <label>equals</label>
                              <input
                                class="wf-launch-input"
                                value={v().xUi?.condition?.eq != null ? String(v().xUi!.condition!.eq) : ''}
                                onInput={(e) => {
                                  const raw = e.currentTarget.value;
                                  let val: any = raw;
                                  if (raw === 'true') val = true;
                                  else if (raw === 'false') val = false;
                                  else if (raw !== '' && !isNaN(Number(raw))) val = Number(raw);
                                  inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, condition: { ...v().xUi?.condition, eq: val } });
                                }}
                                placeholder="true"
                              />
                            </div>
                          </Show>
                        </div>

                        {/* String constraints */}
                        <Show when={v().varType === 'string'}>
                          <div class="wf-var-field">
                            <label>Allowed values</label>
                            <EnumEditor values={v().enumValues} onUpdate={(vals) => inputSchemaUpdateVar(idx, 'enumValues', vals)} disabled={false} />
                          </div>
                          <div class="wf-var-grid">
                            <div class="wf-var-field">
                              <label>Min length</label>
                              <input class="wf-launch-input" type="number" value={v().minLength ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'minLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                            </div>
                            <div class="wf-var-field">
                              <label>Max length</label>
                              <input class="wf-launch-input" type="number" value={v().maxLength ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'maxLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                            </div>
                          </div>
                          <div class="wf-var-field">
                            <label>Pattern (regex)</label>
                            <input class="wf-launch-input" type="text" value={v().pattern ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'pattern', e.currentTarget.value || undefined)} placeholder="^[a-z]+$" />
                          </div>
                        </Show>

                        {/* Number constraints */}
                        <Show when={v().varType === 'number'}>
                          <div class="wf-var-field">
                            <label>Allowed values</label>
                            <EnumEditor values={v().enumValues} onUpdate={(vals) => inputSchemaUpdateVar(idx, 'enumValues', vals)} disabled={false} />
                          </div>
                          <div class="wf-var-grid">
                            <div class="wf-var-field">
                              <label>Minimum</label>
                              <input class="wf-launch-input" type="number" value={v().minimum ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'minimum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                            </div>
                            <div class="wf-var-field">
                              <label>Maximum</label>
                              <input class="wf-launch-input" type="number" value={v().maximum ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'maximum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                            </div>
                          </div>
                        </Show>

                        {/* Array items type */}
                        <Show when={v().varType === 'array'}>
                          <div class="wf-var-field">
                            <label>Item type</label>
                            <select class="wf-launch-input" value={v().itemsType ?? 'string'} onChange={(e) => inputSchemaUpdateVar(idx, 'itemsType', e.currentTarget.value)}>
                              <For each={TYPE_VALUES.filter(t => t !== 'array')}>
                                {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                              </For>
                            </select>
                          </div>
                          <Show when={v().itemsType === 'object'}>
                            <div class="wf-var-section-label">Item Properties</div>
                            <div style={{ display: 'flex', 'flex-direction': 'column', gap: '8px' }}>
                              <Index each={v().itemProperties ?? []}>
                                {(p, pIdx) => (
                                  <div class="wf-nested-prop">
                                    <div class="wf-nested-prop-header">
                                      <input class="wf-launch-input" value={p().name} onInput={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                                      <select class="wf-launch-input" style={{ width: '100px', flex: 'none' }} value={p().varType} onChange={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                        <option value="string">{TYPE_LABELS['string']}</option>
                                        <option value="number">{TYPE_LABELS['number']}</option>
                                        <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                      </select>
                                      <button onClick={() => inputSchemaRemoveItemProp(idx, pIdx)} style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.9em', padding: '0 4px' }} title="Remove property">✕</button>
                                    </div>
                                    <div class="wf-var-grid">
                                      <div class="wf-var-field full-width">
                                        <label>Description</label>
                                        <input class="wf-launch-input" value={p().description} onInput={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                                      </div>
                                    </div>
                                  </div>
                                )}
                              </Index>
                              <button class="wf-btn-secondary" style="align-self:flex-start;padding:4px 12px;font-size:0.82em;" onClick={() => inputSchemaAddItemProp(idx)}>+ Add property</button>
                            </div>
                          </Show>
                        </Show>

                        {/* Object nested properties */}
                        <Show when={v().varType === 'object'}>
                          <div class="wf-var-section-label">Properties</div>
                          <div style={{ display: 'flex', 'flex-direction': 'column', gap: '8px' }}>
                            <Index each={v().properties ?? []}>
                              {(p, pIdx) => (
                                <div class="wf-nested-prop">
                                  <div class="wf-nested-prop-header">
                                    <input class="wf-launch-input" value={p().name} onInput={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                                    <select class="wf-launch-input" style={{ width: '100px', flex: 'none' }} value={p().varType} onChange={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                      <option value="string">{TYPE_LABELS['string']}</option>
                                      <option value="number">{TYPE_LABELS['number']}</option>
                                      <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                    </select>
                                    <button onClick={() => inputSchemaRemoveNestedProp(idx, pIdx)} style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.9em', padding: '0 4px' }} title="Remove property">✕</button>
                                  </div>
                                  <div class="wf-var-grid">
                                    <div class="wf-var-field full-width">
                                      <label>Description</label>
                                      <input class="wf-launch-input" value={p().description} onInput={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                                    </div>
                                  </div>
                                </div>
                              )}
                            </Index>
                            <button class="wf-btn-secondary" style="align-self:flex-start;padding:4px 12px;font-size:0.82em;" onClick={() => inputSchemaAddNestedProp(idx)}>+ Add property</button>
                          </div>
                        </Show>
                      </div>
                    )}
                  </Index>
                </div>

                <button
                  class="wf-btn-secondary"
                  style="align-self:flex-start;padding:6px 14px;font-size:0.85em;"
                  onClick={inputSchemaAddVar}
                >+ Add input</button>

                <DialogFooter>
                  <Button variant="outline" onClick={() => setShowInputSchemaDialog(false)}>Cancel</Button>
                  <Button onClick={handleInputSchemaOk}>OK</Button>
                </DialogFooter>
            </DialogContent>
          </Dialog>
        </>);
      }

      case 'event':
        return (<>
          <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            <Bell size={14} style={{ display: 'inline', 'vertical-align': 'middle' }} /> Event trigger — workflow starts when a matching event occurs.
          </div>
          <div style={{ 'margin-top': '8px' }}>
            <div style={labelStyle}>Topic <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(required)</span></div>
            {renderTopicSelector('topic', { borderOverride: fieldBorder('topic') })}
          </div>
          <div style={{ 'margin-top': '8px' }}>
            <div style={labelStyle}>Filter <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(optional)</span></div>
            {renderFilterBuilder('filter')}
          </div>
        </>);

      case 'incoming_message': {
        const commsChannels = () => (props.channels ?? []).filter((c: any) => c.hasComms !== false);
        const selectedConnector = () => {
          const cid = props.getCfg().connector_id;
          return cid ? commsChannels().find((c: any) => c.id === cid) : undefined;
        };
        const providerType = () => selectedConnector()?.provider ?? '';
        const isChat = () => ['discord', 'slack'].includes(providerType());
        const isEmail = () => ['microsoft', 'gmail', 'imap'].includes(providerType());

        // Channel list for chat-based connectors
        const [connectorChannels, setConnectorChannels] = createSignal<{ id: string; name: string; channel_type?: string; group_name?: string }[]>([]);
        const [channelsLoading, setChannelsLoading] = createSignal(false);
        const [channelsError, setChannelsError] = createSignal<string | null>(null);

        let channelFetchSeq = 0;
        const fetchChannels = async (connectorId: string) => {
          if (!connectorId) { setConnectorChannels([]); setChannelsError(null); return; }
          const mySeq = ++channelFetchSeq;
          setChannelsLoading(true);
          setChannelsError(null);
          try {
            const chs = await invoke<any[]>('list_connector_channels', { connector_id: connectorId });
            if (mySeq !== channelFetchSeq) return;
            setConnectorChannels(chs ?? []);
          } catch (e: any) {
            if (mySeq !== channelFetchSeq) return;
            console.error('Failed to load connector channels:', e);
            setConnectorChannels([]);
            setChannelsError(typeof e === 'string' ? e : e?.message ?? 'Failed to load channels');
          } finally {
            if (mySeq === channelFetchSeq) setChannelsLoading(false);
          }
        };

        // Fetch channels when connector changes and it's a chat provider
        createEffect(() => {
          const conn = selectedConnector();
          if (conn && ['discord', 'slack'].includes(conn.provider ?? '')) {
            fetchChannels(conn.id);
          } else {
            setConnectorChannels([]);
          }
        });

        const onConnectorChange = (e: Event) => {
          const val = (e.currentTarget as HTMLSelectElement).value;
          doUpdate('connector_id', val);
          doUpdate('listen_channel_id', '');
          doPush();
        };

        return (<>
          <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            <Inbox size={14} style={{ display: 'inline', 'vertical-align': 'middle' }} /> Incoming message trigger — workflow starts when a message is received on a communication channel.
          </div>
          <div style={{ 'margin-top': '8px' }}>
            <div style={labelStyle}>Connector</div>
            <Show when={commsChannels().length > 0} fallback={
              <div style={{ 'font-size': '0.8em', color: 'var(--warning-color)', padding: '8px', background: 'var(--warning-bg)', 'border-radius': '4px' }}>
                No connectors with communication enabled. Add or enable one in Settings → Connectors.
              </div>
            }>
              <select style={inputStyle} value={props.getCfg().connector_id ?? ''} onChange={onConnectorChange} disabled={ro()}>
                <option value="">— Select connector —</option>
                <For each={commsChannels()}>
                  {(ch: any) => <option value={ch.id}>{ch.name}{ch.provider ? ` (${ch.provider})` : ''}</option>}
                </For>
              </select>
            </Show>
          </div>
          <Show when={selectedConnector()}>
            <Show when={isChat()}>
              <div style={{ 'margin-top': '8px' }}>
                <div style={labelStyle}>Channel <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(optional — blank listens to all)</span></div>
                <Show when={!channelsLoading()} fallback={
                  <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', padding: '6px' }}>Loading channels…</div>
                }>
                  <Show when={channelsError()} fallback={
                    <select style={inputStyle} value={props.getCfg().listen_channel_id ?? ''} onChange={(e) => { doUpdate('listen_channel_id', e.currentTarget.value); doPush(); }} disabled={ro()}>
                      <option value="">— All channels —</option>
                      <For each={connectorChannels()}>
                        {(ch) => <option value={ch.id}>{ch.group_name ? `${ch.group_name} / ` : ''}{ch.name}{ch.channel_type ? ` (${ch.channel_type})` : ''}</option>}
                      </For>
                    </select>
                  }>
                    <div style={{ 'font-size': '0.8em', color: 'hsl(0 70% 70%)', padding: '8px', background: 'hsl(0 70% 70% / 0.1)', 'border-radius': '4px' }}>
                      Failed to load channels: {channelsError()}
                      <button style={{ 'margin-left': '8px', 'font-size': '0.9em', 'text-decoration': 'underline', cursor: 'pointer', background: 'none', border: 'none', color: 'inherit' }}
                        onClick={() => { const conn = selectedConnector(); if (conn) fetchChannels(conn.id); }}>
                        Retry
                      </button>
                    </div>
                  </Show>
                </Show>
              </div>
            </Show>
            <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '6px', padding: '6px 8px', background: 'hsl(var(--muted) / 0.5)', 'border-radius': '4px' }}>
              {isChat()
                ? `Messages from ${providerType()} will trigger this workflow. Use the filters below to narrow which messages trigger it.`
                : `Incoming email on this connector will trigger this workflow. Use the filters below to narrow which messages trigger it.`}
            </div>
            <div style={{ 'margin-top': '12px', 'font-size': '0.85em', 'font-weight': 'bold', color: 'hsl(var(--foreground))' }}>
              Filters <span style={{ 'font-weight': 'normal', color: 'hsl(var(--muted-foreground))', 'font-size': '0.9em' }}>(optional — leave blank to match all messages)</span>
            </div>
            <div style={{ 'margin-top': '8px' }}>
              <div style={labelStyle}>From <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(contains)</span></div>
              <input style={inputStyle} type="text" value={props.getCfg().from_filter ?? ''} onInput={(e) => doUpdate('from_filter', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()}
                placeholder={isEmail() ? 'e.g. alice@example.com' : 'e.g. username'} />
            </div>
            <Show when={isEmail()}>
              <div style={{ 'margin-top': '8px' }}>
                <div style={labelStyle}>Subject <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(contains)</span></div>
                <input style={inputStyle} type="text" value={props.getCfg().subject_filter ?? ''} onInput={(e) => doUpdate('subject_filter', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()}
                  placeholder="e.g. Invoice" />
              </div>
            </Show>
            <div style={{ 'margin-top': '8px' }}>
              <div style={labelStyle}>Body <span style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em' }}>(contains)</span></div>
              <input style={inputStyle} type="text" value={props.getCfg().body_filter ?? ''} onInput={(e) => doUpdate('body_filter', e.currentTarget.value)} onBlur={() => doPush()} disabled={ro()}
                placeholder="e.g. keyword or phrase" />
            </div>
            <Switch
              checked={props.getCfg().mark_as_read ?? false}
              onChange={(checked: boolean) => { doUpdate('mark_as_read', checked); doPush(); }}
              disabled={ro()}
              class="flex items-center gap-2"
            >
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Mark message as read after triggering</SwitchLabel>
            </Switch>
            <Switch
              checked={props.getCfg().ignore_replies ?? false}
              onChange={(checked: boolean) => { doUpdate('ignore_replies', checked); doPush(); }}
              disabled={ro()}
              class="flex items-center gap-2"
            >
              <SwitchControl><SwitchThumb /></SwitchControl>
              <SwitchLabel>Ignore replies (only trigger on new messages)</SwitchLabel>
            </Switch>
          </Show>
        </>);
      }

      case 'schedule': {
        return (<>
          <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            <Calendar size={14} style={{ display: 'inline', 'vertical-align': 'middle' }} /> Schedule trigger — workflow runs on a cron schedule.
          </div>
          <div style={{ 'margin-top': '8px' }}>
            <div style={labelStyle}>Cron expression</div>
            <CronBuilder
              value={props.getCfg().cron ?? ''}
              onChange={(v) => { doUpdate('cron', v); doPush(); }}
              disabled={ro()}
            />
          </div>
        </>);
      }

      case 'end_workflow':
        return (
          <div style={{ 'font-size': '0.8em', color: 'hsl(var(--muted-foreground))', 'margin-top': '4px' }}>
            No configuration needed.
          </div>
        );

      default:
        return null;
    }

}

// ── NodeEditorPanel ────────────────────────────────────────────────────

export interface NodeEditorPanelProps {
  nodeId: string;
  getNode: () => DesignerNode;
  getCfg: () => Record<string, any>;
  getErrors: () => string[];
  getConnEdges: () => { incoming: DesignerEdge[]; outgoing: DesignerEdge[] };
  getStepState: () => { status: string; error?: string | null } | undefined;
  readOnly?: boolean;
  channels?: ChannelProp[];
  onRenameNode: (oldId: string, newId: string) => void;
  onUpdateNode: (nodeId: string, updates: Partial<DesignerNode>) => void;
  onDeleteNode: (nodeId: string) => void;
  onRemoveEdge: (edgeId: string) => void;
  onPushHistory: () => void;
  onOpenStepConfig: (nodeId: string) => void;
}

export function NodeEditorPanel(props: NodeEditorPanelProps) {
    const nodeId = props.nodeId;

    return (
      <div style={{ padding: '10px 12px', 'overflow-y': 'auto', flex: '1' }}>
        <Show when={props.getStepState()}>
          {(_) => {
            const ss = () => props.getStepState()!;
            return (
              <div style={{ 'margin-bottom': '8px' }}>
                <span
                  class={`pill ${ss().status === 'completed' ? 'success' : ss().status === 'failed' ? 'danger' : ss().status === 'running' ? 'info' : 'neutral'}`}
                  style={{ 'font-size': '0.82em' }}
                >{ss().status.replace(/_/g, ' ')}</span>
                <Show when={ss().error}>
                  <div style={{ 'font-size': '0.82em', color: 'hsl(var(--destructive))', 'margin-top': '4px', 'word-break': 'break-all' }}>
                    {ss().error}
                  </div>
                </Show>
              </div>
            );
          }}
        </Show>

        <Show when={props.getErrors().length > 0}>
          {(_) => (
            <div style={{ 'margin-bottom': '8px', background: 'hsl(38 92% 50% / 0.1)', border: '1px solid hsl(38 92% 50% / 0.3)', 'border-radius': '4px', padding: '4px 8px', 'font-size': '0.82em', color: 'hsl(38 92% 50%)' }}>
              <AlertTriangle size={14} /> Missing: {props.getErrors().join(', ')}
            </div>
          )}
        </Show>

        <div style={labelStyle}>Step ID</div>
        <input
          style={inputStyle}
          value={nodeId}
          onBlur={(e) => props.onRenameNode(nodeId, e.currentTarget.value.trim())}
          onKeyDown={(e) => { if (e.key === 'Enter') (e.target as HTMLInputElement).blur(); }}
          disabled={props.readOnly}
        />

        <div style={{ 'margin-top': '6px', 'margin-bottom': '4px' }}>
          <span style={{ display: 'inline-block', padding: '2px 8px', 'border-radius': '4px', 'font-size': '0.7em', background: NODE_CATEGORY_COLORS[props.getNode().type]?.bg ?? 'hsl(var(--card))', border: `1px solid ${NODE_CATEGORY_COLORS[props.getNode().type]?.border ?? 'hsl(var(--border))'}`, color: NODE_CATEGORY_COLORS[props.getNode().type]?.border ?? 'hsl(var(--muted-foreground))' }}>
            <SubtypeIcon subtype={props.getNode().subtype} /> {props.getNode().subtype.replace(/_/g, ' ')}
          </span>
        </div>

        {/* Config summary */}
        {(() => {
          const cfg = props.getCfg();
          const sub = props.getNode().subtype;
          let summary = '';
          switch (sub) {
            case 'call_tool': summary = cfg.tool_id ? `Tool: ${cfg.tool_id}` : 'No tool selected'; break;
            case 'invoke_agent': summary = cfg.persona_id ? `Persona: ${cfg.persona_id}` : 'No persona set'; break;
            case 'invoke_prompt': summary = cfg.persona_id ? `${cfg.persona_id}${cfg.prompt_id ? ' → ' + cfg.prompt_id : ''}` : 'No persona set'; break;
            case 'feedback_gate': summary = cfg.prompt ? `Prompt: ${(cfg.prompt as string).slice(0, 50)}${(cfg.prompt as string).length > 50 ? '…' : ''}` : 'No prompt set'; break;
            case 'delay': {
              const ds = Number(cfg.duration_secs ?? 60);
              const parts: string[] = [];
              const d = Math.floor(ds / 86400); if (d) parts.push(`${d}d`);
              const h = Math.floor((ds % 86400) / 3600); if (h) parts.push(`${h}h`);
              const m = Math.floor((ds % 3600) / 60); if (m) parts.push(`${m}m`);
              const s = ds % 60; if (s || parts.length === 0) parts.push(`${s}s`);
              summary = parts.join(' ');
              break;
            }
            case 'signal_agent': summary = cfg.content ? `Content: ${(cfg.content as string).slice(0, 50)}${(cfg.content as string).length > 50 ? '…' : ''}` : 'No content'; break;
            case 'launch_workflow': summary = cfg.workflow_name ? `Workflow: ${cfg.workflow_name}` : 'No workflow set'; break;
            case 'schedule_task': {
              const actionLabel: Record<string, string> = { emit_event: 'Event', send_message: 'Message', http_webhook: 'Webhook', invoke_agent: 'Agent', call_tool: 'Tool', launch_workflow: 'Workflow' };
              const at = cfg.action_type as string;
              summary = cfg.task_name ? `${cfg.task_name} → ${actionLabel[at] || at}` : 'No task set';
              break;
            }
            case 'event_gate': summary = cfg.topic ? `Topic: ${cfg.topic}` : 'No topic set'; break;
            case 'set_variable': {
              const assigns = Array.isArray(cfg.assignments) ? cfg.assignments : [];
              summary = assigns.length > 0 ? `${assigns.length} assignment${assigns.length > 1 ? 's' : ''}: ${assigns.map((a: any) => a.variable || '?').join(', ')}` : 'No assignments';
              break;
            }
            case 'branch': summary = cfg.condition ? `Condition: ${(cfg.condition as string).slice(0, 50)}${(cfg.condition as string).length > 50 ? '…' : ''}` : 'No condition'; break;
            case 'for_each': summary = cfg.collection ? `Collection: ${(cfg.collection as string).slice(0, 50)}` : 'No collection'; break;
            case 'while': summary = cfg.condition ? `Condition: ${(cfg.condition as string).slice(0, 50)}` : 'No condition'; break;
            case 'manual': summary = 'Manual trigger'; break;
            case 'event': summary = cfg.topic ? `Topic: ${cfg.topic}` : 'Event trigger'; break;
            case 'incoming_message': {
              const connId = cfg.connector_id as string;
              if (connId) {
                const ch = (props.channels ?? []).find((c: any) => c.id === connId);
                const base = ch ? `${ch.name}${ch.provider ? ` (${ch.provider})` : ''}` : `Connector: ${connId.slice(0, 30)}`;
                const lcid = cfg.listen_channel_id as string;
                summary = lcid ? `${base} → #${lcid}` : base;
              } else {
                summary = 'Incoming message trigger';
              }
              break;
            }
            case 'schedule': summary = cfg.cron ? `Cron: ${cfg.cron}` : 'Schedule trigger'; break;
            case 'end_workflow': summary = 'No configuration needed'; break;
            default: summary = ''; break;
          }
          return (
            <div style={{ 'margin-top': '8px' }}>
              <Show when={summary}>
                <div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))', padding: '6px 8px', background: 'hsl(var(--muted) / 0.5)', 'border-radius': '4px', 'margin-bottom': '8px', 'word-break': 'break-all' }}>
                  {summary}
                </div>
              </Show>
              <button
                onClick={() => props.onOpenStepConfig(nodeId)}
                style={{
                  width: '100%', padding: '6px',
                  background: 'hsl(160 60% 76% / 0.1)', color: 'hsl(var(--foreground))',
                  border: '1px solid hsl(160 60% 76% / 0.3)', 'border-radius': '4px',
                  cursor: 'pointer', 'font-size': '0.8em', display: 'flex',
                  'align-items': 'center', 'justify-content': 'center', gap: '6px',
                }}
              >
                <Pencil size={14} /> Edit Inputs
              </button>
            </div>
          );
        })()}


        <div style={{ ...labelStyle, 'margin-top': '12px' }}>Error Strategy</div>
        <select
          style={inputStyle}
          value={props.getNode().onError?.strategy ?? ''}
          onChange={(e) => {
            const val = e.currentTarget.value;
            if (!val) props.onUpdateNode(nodeId, { onError: null });
            else props.onUpdateNode(nodeId, { onError: { strategy: val, ...(val === 'retry' ? { max_retries: 3, delay_secs: 5 } : {}), ...(val === 'fallback' ? { fallback_step: '' } : {}) } });
            props.onPushHistory();
          }}
          disabled={props.readOnly}
        >
          <option value="">None</option>
          <option value="fail_workflow">Fail Workflow</option>
          <option value="retry">Retry</option>
          <option value="skip">Skip</option>
          <option value="fallback">Fallback</option>
        </select>
        <Show when={props.getNode().onError?.strategy === 'retry'}>
          {(_) => (<>
            <div style={labelStyle}>Max Retries</div>
            <input style={inputStyle} type="number" value={props.getNode().onError?.max_retries ?? 3} onInput={(e) => props.onUpdateNode(nodeId, { onError: { ...props.getNode().onError!, max_retries: parseInt(e.currentTarget.value) || 0 } })} disabled={props.readOnly} />
            <div style={labelStyle}>Retry Delay (sec)</div>
            <input style={inputStyle} type="number" value={props.getNode().onError?.delay_secs ?? 5} onInput={(e) => props.onUpdateNode(nodeId, { onError: { ...props.getNode().onError!, delay_secs: parseInt(e.currentTarget.value) || 0 } })} disabled={props.readOnly} />
          </>)}
        </Show>
        <Show when={props.getNode().onError?.strategy === 'fallback'}>
          {(_) => (<>
            <div style={labelStyle}>Fallback Step</div>
            <input style={inputStyle} value={props.getNode().onError?.fallback_step ?? ''} onInput={(e) => props.onUpdateNode(nodeId, { onError: { ...props.getNode().onError!, fallback_step: e.currentTarget.value } })} disabled={props.readOnly} />
          </>)}
        </Show>

        <div style={{ ...labelStyle, 'margin-top': '12px' }}>Connections</div>
        <Show when={props.getConnEdges().outgoing.length > 0} fallback={<div style={{ 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))' }}>No connections</div>}>
          <For each={props.getConnEdges().outgoing}>
            {(edge) => (
              <div style={{ display: 'flex', 'align-items': 'center', gap: '4px', 'font-size': '0.8em', color: 'hsl(var(--foreground))', padding: '2px 0' }}>
                <span style={{ color: edge.edgeType === 'then' ? 'hsl(142 71% 45%)' : edge.edgeType === 'else' ? 'hsl(var(--destructive))' : edge.edgeType === 'body' ? 'hsl(38 92% 50%)' : 'hsl(var(--muted-foreground))', 'font-size': '0.7em' }}>
                  {edge.edgeType === 'then' ? <CheckCircle size={12} /> : edge.edgeType === 'else' ? <XCircle size={12} /> : edge.edgeType === 'body' ? '🔁' : '→'}
                </span>
                <span>{edge.edgeType && edge.edgeType !== 'default' ? `[${edge.edgeType}] ` : ''}{edge.target}</span>
                <Show when={!props.readOnly}>
                  <button onClick={() => props.onRemoveEdge(edge.id)} style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.8em', padding: '0 2px' }} title="Remove connection">✕</button>
                </Show>
              </div>
            )}
          </For>
        </Show>

        <Show when={!props.readOnly}>
          <button
            onClick={() => props.onDeleteNode(nodeId)}
            style={{
              'margin-top': '16px', width: '100%', padding: '6px',
              background: 'hsl(var(--destructive) / 0.15)', color: 'hsl(var(--destructive))',
              border: '1px solid hsl(var(--destructive) / 0.3)', 'border-radius': '4px',
              cursor: 'pointer', 'font-size': '0.8em',
            }}
          ><Trash2 size={14} /> Delete Node</button>
        </Show>
      </div>
    );

}
