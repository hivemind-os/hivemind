import { Component, createSignal, createMemo, createEffect, onMount, onCleanup, For, Index, Show, untrack, ErrorBoundary, batch, type JSX } from 'solid-js';
import { Wrench, Hand, Timer, RotateCcw, RotateCw, ClipboardList, AlertTriangle, Send, Paperclip, Sparkles, ArrowLeft, Save, LayoutGrid, Maximize2, Grid3x3, Code2, AlignLeft } from 'lucide-solid';

import { GraphCanvas, type CanvasNode, type CanvasEdge } from './WorkflowCanvas';
import PermissionRulesEditor, { type PermissionRule as WfPermissionRule } from './PermissionRulesEditor';
import { CronBuilder, TopicSelector, PersonaSelector, payloadKeysForTopic as sharedPayloadKeysForTopic } from './shared';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import { useTimerCleanup } from '~/lib/useTimerCleanup';
import yaml from 'js-yaml';
import { invoke } from '@tauri-apps/api/core';

// Extracted workflow sub-components
import {
  type DesignerNode,
  type DesignerEdge,
  type WorkflowVariable,
  type WorkflowAttachment,
  type ToolDefinitionProp,
  type PersonaProp,
  type ChannelProp,
  SubtypeIcon,
  EnumEditor,
  inputStyle,
  labelStyle,
} from './workflow/types';
import { StepConfigFields, NodeEditorPanel } from './workflow/StepEditor';
import { AiAssistPanel, type AiQuestion } from './workflow/AiAssistPanel';
import { YamlEditorPanel } from './workflow/YamlEditorPanel';
import { AttachmentsPanel } from './workflow/WorkflowPreview';

// Module-level counter for stable expression helper IDs across re-renders
let _exprHelperNextId = 0;

// ── Types (re-exported from workflow/types.tsx) ────────────────────────
// DesignerNode, DesignerEdge, WorkflowVariable, WorkflowAttachment,
// ToolDefinitionProp, PersonaProp, ChannelProp are imported above.

interface WorkflowDesignerProps {
  initialYaml?: string;
  onYamlChange?: (yaml: string) => void;
  onSaveResult?: (ok: boolean, message: string) => void;
  readOnly?: boolean;
  instanceStepStates?: Record<string, { status: string; error?: string | null }>;
  toolDefinitions?: ToolDefinitionProp[];
  personas?: PersonaProp[];
  channels?: ChannelProp[];
  eventTopics?: { topic: string; description: string; payload_keys?: string[] }[];
  /** Called when user clicks Save or presses Ctrl+S. */
  onSave?: () => void;
  /** Called when user clicks Close/Back. */
  onClose?: () => void;
  /** Whether a save operation is in progress. */
  saving?: boolean;
  /** Called when user invokes AI assist. Returns the agent_id. */
  onAiAssist?: (yaml: string, prompt: string, agent_id?: string | null) => Promise<{ agent_id: string }>;
  /** Subscribe to bot events for streaming. Returns an unsubscribe function. */
  onAiAssistSubscribe?: (agent_id: string, callbacks: {
    onToken: (delta: string) => void;
    onToolCall: (tool_id: string, input: any) => void;
    onDone: () => void;
    onError: (error: string) => void;
    onQuestion: (request_id: string, agent_id: string, text: string, choices: string[], allow_freeform: boolean, message?: string) => void;
  }) => (() => void);
  /** Called to clean up (kill) the AI assist agent. */
  onAiAssistCleanup?: (agent_id: string) => void;
  /** Upload a file attachment. Returns the created attachment metadata. */
  onUploadAttachment?: (workflowId: string, version: string, filePath: string, description: string) => Promise<WorkflowAttachment>;
  /** Delete a file attachment. */
  onDeleteAttachment?: (workflowId: string, version: string, attachmentId: string) => Promise<void>;
  /** When true, the mode selector is locked (editing existing workflow). */
  lockMode?: boolean;
  /** If set, automatically trigger AI assist with this prompt when the designer opens. */
  initialAiPrompt?: string;
}



interface PaletteItem {
  type: 'trigger' | 'task' | 'control_flow';
  subtype: string;
  label: string;
}



interface HistoryEntry {
  nodes: DesignerNode[];
  edges: DesignerEdge[];
  variables: WorkflowVariable[];
  permissions: WfPermissionRule[];
  attachments: WorkflowAttachment[];
  wfName: string;
  wfVersion: string;
  wfDescription: string;
  wfMode: string;
  wfResultMessage: string;
  _json?: string;
}

// ── Constants ──────────────────────────────────────────────────────────

const NODE_MIN_W = 140;
const NODE_H = 60;
const PORT_R = 8;
const PORT_HIT_R = 30;
const MIN_ZOOM = 0.2;
const MAX_ZOOM = 3;
const GRID_SIZE = 20;
const MAX_HISTORY = 50;

const PALETTE_ITEMS: { category: string; items: PaletteItem[] }[] = [
  {
    category: 'Triggers',
    items: [
      { type: 'trigger', subtype: 'manual', label: 'Manual Trigger' },
      { type: 'trigger', subtype: 'event', label: 'Event Trigger' },
      { type: 'trigger', subtype: 'incoming_message', label: 'Incoming Message' },
      { type: 'trigger', subtype: 'schedule', label: 'Schedule Trigger' },
    ],
  },
  {
    category: 'Tasks',
    items: [
      { type: 'task', subtype: 'call_tool', label: 'Call Tool' },
      { type: 'task', subtype: 'invoke_agent', label: 'Invoke Agent' },
      { type: 'task', subtype: 'invoke_prompt', label: 'Invoke Prompt' },
      { type: 'task', subtype: 'feedback_gate', label: 'Feedback Gate' },
      { type: 'task', subtype: 'delay', label: 'Delay' },
      { type: 'task', subtype: 'signal_agent', label: 'Signal Agent' },
      { type: 'task', subtype: 'launch_workflow', label: 'Launch Workflow' },
      { type: 'task', subtype: 'schedule_task', label: 'Schedule Task' },
      { type: 'task', subtype: 'event_gate', label: 'Event Gate' },
      { type: 'task', subtype: 'set_variable', label: 'Set Variable' },
    ],
  },
  {
    category: 'Control Flow',
    items: [
      { type: 'control_flow', subtype: 'branch', label: 'Branch' },
      { type: 'control_flow', subtype: 'for_each', label: 'For Each' },
      { type: 'control_flow', subtype: 'while', label: 'While Loop' },
      { type: 'control_flow', subtype: 'end_workflow', label: 'End Workflow' },
    ],
  },
];

const STATUS_COLORS: Record<string, string> = {
  pending: 'hsl(var(--muted-foreground))',
  ready: 'hsl(var(--primary))',
  running: 'hsl(var(--primary))',
  completed: 'hsl(var(--chart-2, 142 71% 45%))',
  failed: 'hsl(var(--destructive))',
  skipped: 'hsl(var(--muted-foreground))',
  waiting_on_input: 'hsl(var(--chart-4, 38 92% 50%))',
  waiting_on_event: 'hsl(var(--chart-4, 38 92% 50%))',
};

// NODE_CATEGORY_COLORS imported from workflow/types

function nodeWidth(id: string): number {
  const textLen = id.length * 7 + 44;
  return Math.max(NODE_MIN_W, Math.min(textLen, 240));
}

function snapToGrid(val: number): number {
  return Math.round(val / GRID_SIZE) * GRID_SIZE;
}

function requiredFields(subtype: string): string[] {
  switch (subtype) {
    case 'call_tool': return ['tool_id'];
    case 'invoke_agent': return ['persona_id', 'task'];
    case 'invoke_prompt': return ['persona_id', 'prompt_id'];
    case 'feedback_gate': return ['prompt'];
    case 'signal_agent': return ['content'];
    case 'launch_workflow': return ['workflow_name'];
    case 'schedule_task': return ['task_name'];
    case 'event_gate': return ['topic'];
    case 'event': return ['topic'];
    case 'incoming_message': return ['connector_id'];
    case 'set_variable': return ['assignments'];
    case 'branch': return ['condition'];
    case 'for_each': return ['collection'];
    case 'while': return ['condition'];
    default: return [];
  }
}

function nodeHasValidationErrors(node: DesignerNode): boolean {
  const fields = requiredFields(node.subtype);
  return fields.some(f => {
    const val = node.config[f];
    if (f === 'assignments') return !Array.isArray(val) || val.length === 0;
    return val === undefined || val === null || val === '';
  });
}

// SubtypeIcon imported from workflow/types

function defaultConfig(subtype: string): Record<string, any> {
  switch (subtype) {
    case 'manual': return { input_schema: [] };
    case 'event': return { topic: '', filter: '' };
    case 'incoming_message': return { connector_id: '', listen_channel_id: '', from_filter: '', subject_filter: '', body_filter: '', mark_as_read: false, ignore_replies: false };
    case 'schedule': return { cron: '' };
    case 'call_tool': return { tool_id: '', arguments: {} };
    case 'invoke_agent': return { persona_id: '', task: '', async_exec: false, timeout_secs: 300, permissions: [], attachments: [] };
    case 'invoke_prompt': return { persona_id: '', prompt_id: '', parameters: {}, async_exec: false, timeout_secs: 300, permissions: [], attachments: [] };
    case 'feedback_gate': return { prompt: '', choices: ['approve', 'reject'], allow_freeform: false };
    case 'delay': return { duration_secs: 60 };
    case 'signal_agent': return { target_type: 'session', target_id: '', content: '' };
    case 'launch_workflow': return { workflow_name: '', inputs: {} };
    case 'schedule_task': return {
      task_name: '', task_description: '', schedule: '', action_type: 'emit_event',
      // emit_event
      action_topic: '', action_payload: '{}',
      // send_message
      action_session_id: '', action_content: '',
      // http_webhook
      action_url: '', action_method: 'POST', action_body: '',
      // invoke_agent
      action_persona_id: '', action_task: '', action_friendly_name: '', action_timeout_secs: 300,
      // call_tool
      action_tool_id: '', action_arguments: {},
      // launch_workflow
      action_workflow: '', action_workflow_version: '', action_workflow_inputs: '{}',
    };
    case 'event_gate': return { topic: '', filter: '', timeout_secs: '' };
    case 'set_variable': return { assignments: [{ variable: '', value: '', operation: 'set' }] };
    case 'branch': return { condition: '' };
    case 'for_each': return { collection: '', item_var: 'item' };
    case 'while': return { condition: '', max_iterations: 100 };
    case 'end_workflow': return {};
    default: return {};
  }
}

let idCounter = 0;
function nextId(subtype: string): string {
  return `${subtype}_${++idCounter}`;
}

// ── YAML helpers (using js-yaml) ───────────────────────────────────────

function buildVariableSchema(v: WorkflowVariable): Record<string, any> {
  const prop: Record<string, any> = { type: v.varType };
  if (v.description) prop.description = v.description;
  if (v.defaultValue) {
    if (v.varType === 'number') prop.default = Number(v.defaultValue);
    else if (v.varType === 'boolean') prop.default = v.defaultValue === 'true';
    else prop.default = v.defaultValue;
  }
  if (v.enumValues && v.enumValues.length > 0) prop.enum = v.enumValues;
  if (v.varType === 'string') {
    if (v.minLength != null) prop.minLength = v.minLength;
    if (v.maxLength != null) prop.maxLength = v.maxLength;
    if (v.pattern) prop.pattern = v.pattern;
  }
  if (v.varType === 'number') {
    if (v.minimum != null) prop.minimum = v.minimum;
    if (v.maximum != null) prop.maximum = v.maximum;
  }
  if (v.varType === 'array' && v.itemsType) {
    const items: Record<string, any> = { type: v.itemsType };
    if (v.itemsType === 'object' && v.itemProperties && v.itemProperties.length > 0) {
      items.properties = Object.fromEntries(
        v.itemProperties.map(p => [p.name, buildSubPropertySchema(p)])
      );
    }
    prop.items = items;
  }
  if (v.varType === 'object' && v.properties && v.properties.length > 0) {
    prop.properties = Object.fromEntries(
      v.properties.map(p => [p.name, buildSubPropertySchema(p)])
    );
  }
  if (v.xUi && Object.values(v.xUi).some(val => val !== undefined)) prop['x-ui'] = v.xUi;
  return prop;
}

function buildSubPropertySchema(p: { name: string; varType: string; description?: string; defaultValue?: string; enumValues?: string[] }): Record<string, any> {
  const prop: Record<string, any> = { type: p.varType };
  if (p.description) prop.description = p.description;
  if (p.defaultValue) prop.default = p.defaultValue;
  if (p.enumValues && p.enumValues.length > 0) prop.enum = p.enumValues;
  return prop;
}

function toYaml(
  nodes: DesignerNode[],
  edges: DesignerEdge[],
  wfId: string,
  wfName: string,
  wfVersion: string,
  wfDescription: string,
  wfMode: string,
  wfResultMessage: string,
  variables: WorkflowVariable[],
  permissions: WfPermissionRule[],
  attachments: WorkflowAttachment[],
): string {
  const doc: Record<string, any> = {
    id: wfId,
    name: wfName,
    version: wfVersion,
  };
  if (wfDescription) doc.description = wfDescription;
  if (wfMode && wfMode !== 'background') doc.mode = wfMode;
  if (wfResultMessage) doc.result_message = wfResultMessage;
  if (attachments.length > 0) {
    doc.attachments = attachments.map(a => ({
      id: a.id,
      filename: a.filename,
      description: a.description,
      ...(a.media_type ? { media_type: a.media_type } : {}),
      ...(a.size_bytes != null ? { size_bytes: a.size_bytes } : {}),
    }));
  }

  // Variables
  if (variables.length > 0) {
    const variablesSchema: Record<string, any> = { type: 'object' };
    const requiredVars = variables.filter(v => v.required).map(v => v.name);
    if (requiredVars.length > 0) variablesSchema.required = requiredVars;
    variablesSchema.properties = Object.fromEntries(
      variables.map(v => [v.name, buildVariableSchema(v)])
    );
    doc.variables = variablesSchema;
  }

  // Steps
  const steps: any[] = [];
  for (const node of nodes) {
    const outEdges = edges.filter(e => e.source === node.id);
    let nextSteps = outEdges.map(e => e.target);
    const step: Record<string, any> = { id: node.id, type: node.type };

    if (node.type === 'trigger') {
      const subtype = node.subtype ?? 'manual';
      if (subtype === 'manual') {
        const trigger: Record<string, any> = { type: 'manual' };
        const triggerInputSchema: WorkflowVariable[] = node.config?.input_schema ?? [];
        if (triggerInputSchema.length > 0) {
          const schema: Record<string, any> = { type: 'object' };
          const requiredInputs = triggerInputSchema.filter(v => v.required).map(v => v.name);
          if (requiredInputs.length > 0) schema.required = requiredInputs;
          schema.properties = Object.fromEntries(
            triggerInputSchema.map(v => [v.name, buildVariableSchema(v)])
          );
          trigger.input_schema = schema;
        }
        trigger.inputs = [];
        step.trigger = trigger;
      } else if (subtype === 'event') {
        const trigger: Record<string, any> = { type: 'event_pattern', topic: node.config?.topic ?? '' };
        const filter = node.config?.filter ?? '';
        if (filter) trigger.filter = filter;
        step.trigger = trigger;
      } else if (subtype === 'incoming_message') {
        const trigger: Record<string, any> = { type: 'incoming_message', channel_id: node.config?.connector_id ?? '' };
        const listenChannelId = node.config?.listen_channel_id ?? '';
        if (listenChannelId) trigger.listen_channel_id = listenChannelId;
        const fromFilter = node.config?.from_filter ?? '';
        const subjectFilter = node.config?.subject_filter ?? '';
        const bodyFilter = node.config?.body_filter ?? '';
        if (fromFilter) trigger.from_filter = fromFilter;
        if (subjectFilter) trigger.subject_filter = subjectFilter;
        if (bodyFilter) trigger.body_filter = bodyFilter;
        const markAsRead = node.config?.mark_as_read ?? false;
        if (markAsRead) trigger.mark_as_read = true;
        const ignoreReplies = node.config?.ignore_replies ?? false;
        if (ignoreReplies) trigger.ignore_replies = true;
        step.trigger = trigger;
      } else if (subtype === 'schedule') {
        const trigger: Record<string, any> = { type: 'schedule', cron: node.config?.cron ?? '' };
        step.trigger = trigger;
      }
    } else if (node.type === 'task') {
      const taskConf = { ...node.config };
      if (node.subtype === 'signal_agent') {
        const targetType = taskConf.target_type || 'session';
        const targetId = taskConf.target_id || '';
        delete taskConf.target_type;
        delete taskConf.target_id;
        taskConf.target = {
          type: targetType,
          ...(targetType === 'agent' ? { agent_id: targetId } : { session_id: targetId }),
        };
      }
      if (node.subtype === 'schedule_task') {
        const taskName = taskConf.task_name || '';
        const schedule = taskConf.schedule || '';
        const actionType = taskConf.action_type || 'emit_event';
        let action: Record<string, any> = {};
        switch (actionType) {
          case 'emit_event':
            action = { type: 'emit_event', topic: taskConf.action_topic || '', payload: (() => { try { return JSON.parse(taskConf.action_payload || '{}'); } catch { return {}; } })() };
            break;
          case 'send_message':
            action = { type: 'send_message', session_id: taskConf.action_session_id || '', content: taskConf.action_content || '' };
            break;
          case 'http_webhook':
            action = { type: 'http_webhook', url: taskConf.action_url || '', method: taskConf.action_method || 'POST', body: taskConf.action_body || null };
            break;
          case 'invoke_agent':
            action = { type: 'invoke_agent', persona_id: taskConf.action_persona_id || '', task: taskConf.action_task || '' };
            if (taskConf.action_friendly_name) action.friendly_name = taskConf.action_friendly_name;
            if (taskConf.action_timeout_secs) action.timeout_secs = parseInt(taskConf.action_timeout_secs, 10) || 300;
            break;
          case 'call_tool':
            action = { type: 'call_tool', tool_id: taskConf.action_tool_id || '', arguments: taskConf.action_arguments ?? {} };
            break;
          case 'launch_workflow':
            action = { type: 'launch_workflow', definition: taskConf.action_workflow || '' };
            if (taskConf.action_workflow_version) action.version = taskConf.action_workflow_version;
            action.inputs = (() => { try { return JSON.parse(taskConf.action_workflow_inputs || '{}'); } catch { return {}; } })();
            break;
        }
        // Remove flat config fields
        for (const k of Object.keys(taskConf)) {
          if (k.startsWith('action_') || k === 'task_name' || k === 'task_description') delete taskConf[k];
        }
        taskConf.schedule = { name: taskName, schedule, action };
        if (taskConf.task_description) taskConf.schedule.description = taskConf.task_description;
      }
      if (node.subtype === 'event_gate') {
        if (!taskConf.filter) delete taskConf.filter;
        if (taskConf.timeout_secs) {
          taskConf.timeout_secs = parseInt(taskConf.timeout_secs, 10);
        } else {
          delete taskConf.timeout_secs;
        }
      }
      if (node.subtype === 'invoke_agent') {
        if (taskConf.async_exec) {
          delete taskConf.timeout_secs;
        } else if (taskConf.timeout_secs && !isNaN(taskConf.timeout_secs)) {
          taskConf.timeout_secs = parseInt(taskConf.timeout_secs, 10);
        } else {
          delete taskConf.timeout_secs;
        }
        if (Array.isArray(taskConf.permissions) && taskConf.permissions.length > 0) {
          taskConf.permissions = taskConf.permissions.map((r: any) => ({
            tool_id: r.tool_pattern,
            ...(r.scope && r.scope !== '*' ? { resource: r.scope } : {}),
            approval: r.decision,
          }));
        } else {
          delete taskConf.permissions;
        }
      }
      if (node.subtype === 'invoke_prompt') {
        // Handle target_agent_id — use null check, not truthiness (empty string = user in existing mode)
        const hasTarget = taskConf.target_agent_id != null;
        if (hasTarget) {
          // Existing-agent mode with auto_create: keep agent settings for fallback
          if (taskConf.auto_create) {
            if (taskConf.async_exec) {
              delete taskConf.timeout_secs;
            } else if (taskConf.timeout_secs && !isNaN(taskConf.timeout_secs)) {
              taskConf.timeout_secs = parseInt(taskConf.timeout_secs, 10);
            } else {
              delete taskConf.timeout_secs;
            }
            if (Array.isArray(taskConf.permissions) && taskConf.permissions.length > 0) {
              taskConf.permissions = taskConf.permissions.map((r: any) => ({
                tool_id: r.tool_pattern,
                ...(r.scope && r.scope !== '*' ? { resource: r.scope } : {}),
                approval: r.decision,
              }));
            } else {
              delete taskConf.permissions;
            }
          } else {
            // Existing-agent mode without auto_create: clean up agent-only fields
            delete taskConf.auto_create;
            delete taskConf.async_exec;
            delete taskConf.timeout_secs;
            delete taskConf.permissions;
          }
        } else {
          delete taskConf.target_agent_id;
          delete taskConf.auto_create;
          if (taskConf.async_exec) {
            delete taskConf.timeout_secs;
          } else if (taskConf.timeout_secs && !isNaN(taskConf.timeout_secs)) {
            taskConf.timeout_secs = parseInt(taskConf.timeout_secs, 10);
          } else {
            delete taskConf.timeout_secs;
          }
          if (Array.isArray(taskConf.permissions) && taskConf.permissions.length > 0) {
            taskConf.permissions = taskConf.permissions.map((r: any) => ({
              tool_id: r.tool_pattern,
              ...(r.scope && r.scope !== '*' ? { resource: r.scope } : {}),
              approval: r.decision,
            }));
          } else {
            delete taskConf.permissions;
          }
        }
        // Remove phantom attachments (not supported by backend)
        delete taskConf.attachments;
        // Serialize parameters: drop empty entries
        if (taskConf.parameters && typeof taskConf.parameters === 'object') {
          const cleaned: Record<string, string> = {};
          for (const [k, v] of Object.entries(taskConf.parameters)) {
            if (k && v) cleaned[k] = v as string;
          }
          if (Object.keys(cleaned).length > 0) {
            taskConf.parameters = cleaned;
          } else {
            delete taskConf.parameters;
          }
        }
      }
      step.task = { kind: node.subtype, ...taskConf };
    } else if (node.type === 'control_flow') {
      const ctrlConf = { ...node.config };
      if (node.subtype === 'branch') {
        const thenEdges = outEdges.filter(e => e.edgeType === 'then' || (!e.edgeType && outEdges.indexOf(e) === 0));
        const elseEdges = outEdges.filter(e => e.edgeType === 'else');
        ctrlConf.then = thenEdges.map(e => e.target);
        ctrlConf.else = elseEdges.map(e => e.target);
      } else if (node.subtype === 'for_each' || node.subtype === 'while') {
        const bodyEdges = outEdges.filter(e => e.edgeType === 'body');
        ctrlConf.body = bodyEdges.map(e => e.target);
        nextSteps = outEdges.filter(e => e.edgeType !== 'body').map(e => e.target);
      }
      step.control = { kind: node.subtype, ...ctrlConf };
    }

    // Outputs
    const effectiveOutputs: Record<string, string> = { ...node.outputs };
    if (node.type === 'trigger' && node.subtype === 'manual') {
      const triggerInputSchema: WorkflowVariable[] = node.config?.input_schema ?? [];
      for (const v of triggerInputSchema) {
        if (!effectiveOutputs[v.name]) {
          effectiveOutputs[v.name] = `{{result.${v.name}}}`;
        }
      }
    }
    if (Object.keys(effectiveOutputs).length > 0) step.outputs = effectiveOutputs;

    // Error handling
    if (node.onError) {
      const onError: Record<string, any> = { strategy: node.onError.strategy };
      if (node.onError.max_retries !== undefined) onError.max_retries = node.onError.max_retries;
      if (node.onError.delay_secs !== undefined) onError.delay_secs = node.onError.delay_secs;
      if (node.onError.fallback_step) onError.step_id = node.onError.fallback_step;
      step.on_error = onError;
    }

    // Next steps
    if (nextSteps.length > 0) step.next = nextSteps;

    steps.push(step);
  }
  doc.steps = steps;

  return yaml.dump(doc, { lineWidth: -1, noRefs: true, quotingType: "'", forceQuotes: false });
}

function parseSimpleYaml(text: string): any {
  try {
    // Try JSON first (from store responses)
    return JSON.parse(text);
  } catch { /* fall through */ }
  return yaml.load(text) ?? {};
}

function fromYaml(yamlStr: string): {
  nodes: DesignerNode[];
  edges: DesignerEdge[];
  id: string;
  name: string;
  version: string;
  description: string;
  mode: string;
  resultMessage: string;
  variables: WorkflowVariable[];
  permissions: WfPermissionRule[];
  attachments: WorkflowAttachment[];
} {
  const def = parseSimpleYaml(yamlStr);
  const nodes: DesignerNode[] = [];
  const edges: DesignerEdge[] = [];
  const variables: WorkflowVariable[] = [];

  const id = def.id || crypto.randomUUID();
  const name = def.name || 'untitled';
  const version = def.version || '1';
  const description = def.description || '';
  const mode = def.mode || 'background';
  const resultMessage = def.result_message || '';

  const permissions: WfPermissionRule[] = [];
  if (Array.isArray(def.permissions)) {
    for (const pe of def.permissions) {
      permissions.push({
        tool_pattern: pe.tool_id ?? pe.tool_pattern ?? '*',
        scope: pe.resource ?? pe.scope ?? '*',
        decision: pe.approval ?? pe.decision ?? 'ask',
      });
    }
  }

  if (def.variables && typeof def.variables === 'object') {
    // Handle JSON Schema format: { type: "object", properties: { ... } }
    const props = def.variables.properties ?? def.variables;
    const requiredList: string[] = def.variables.required || [];
    for (const [vName, vDef] of Object.entries(props as Record<string, any>)) {
      if (vName === 'type' || vName === 'properties' || vName === 'required') continue;
      const v: WorkflowVariable = {
        name: vName,
        varType: (vDef?.type || 'string') as WorkflowVariable['varType'],
        description: vDef?.description || '',
        required: requiredList.includes(vName),
        defaultValue: vDef?.default != null ? String(vDef.default) : '',
        enumValues: Array.isArray(vDef?.enum) ? vDef.enum.map(String) : [],
        minLength: vDef?.minLength,
        maxLength: vDef?.maxLength,
        pattern: vDef?.pattern,
        minimum: vDef?.minimum,
        maximum: vDef?.maximum,
        itemsType: vDef?.items?.type,
        itemProperties: [],
        properties: [],
        xUi: vDef?.['x-ui'] && typeof vDef['x-ui'] === 'object' ? vDef['x-ui'] : undefined,
      };
      // Parse array item properties (list of objects)
      if (vDef?.items?.properties && typeof vDef.items.properties === 'object') {
        for (const [pName, pDef] of Object.entries(vDef.items.properties as Record<string, any>)) {
          v.itemProperties!.push({
            name: pName,
            varType: ((pDef as any)?.type || 'string') as WorkflowVariable['varType'],
            description: (pDef as any)?.description || '',
            required: false,
            defaultValue: (pDef as any)?.default != null ? String((pDef as any).default) : '',
            enumValues: Array.isArray((pDef as any)?.enum) ? (pDef as any).enum.map(String) : [],
          });
        }
      }
      if (vDef?.properties && typeof vDef.properties === 'object') {
        for (const [pName, pDef] of Object.entries(vDef.properties as Record<string, any>)) {
          v.properties!.push({
            name: pName,
            varType: ((pDef as any)?.type || 'string') as WorkflowVariable['varType'],
            description: (pDef as any)?.description || '',
            required: false,
            defaultValue: (pDef as any)?.default != null ? String((pDef as any).default) : '',
            enumValues: Array.isArray((pDef as any)?.enum) ? (pDef as any).enum.map(String) : [],
          });
        }
      }
      variables.push(v);
    }
  }

  const steps: any[] = def.steps || [];
  for (const step of steps) {
    let node_type: 'trigger' | 'task' | 'control_flow' = 'task';
    let subtype = 'call_tool';
    const config: Record<string, any> = {};

    if (step.type === 'trigger') {
      node_type = 'trigger';
      const trigDef = step.trigger;
      if (trigDef?.type === 'incoming_message') {
        subtype = 'incoming_message';
        config.connector_id = trigDef.channel_id ?? '';
        config.listen_channel_id = trigDef.listen_channel_id ?? '';
        config.from_filter = trigDef.from_filter ?? '';
        config.subject_filter = trigDef.subject_filter ?? '';
        config.body_filter = trigDef.body_filter ?? '';
        config.mark_as_read = trigDef.mark_as_read ?? false;
        config.ignore_replies = trigDef.ignore_replies ?? false;
      } else if (trigDef?.type === 'event_pattern') {
        subtype = 'event';
        config.topic = trigDef.topic ?? '';
        config.filter = trigDef.filter ?? '';
      } else if (trigDef?.type === 'schedule') {
        subtype = 'schedule';
        config.cron = trigDef.cron ?? '';
      } else {
        subtype = 'manual';
      }
      // Load input_schema from the corresponding trigger definition
      if (trigDef && trigDef.type === 'manual' && trigDef.input_schema && typeof trigDef.input_schema === 'object') {
        const schema = trigDef.input_schema;
        const schemaProps = schema.properties ?? schema;
        const schemaRequired: string[] = schema.required || [];
        const inputVars: WorkflowVariable[] = [];
        for (const [vName, vDef] of Object.entries(schemaProps as Record<string, any>)) {
          if (vName === 'type' || vName === 'properties' || vName === 'required') continue;
          const v: WorkflowVariable = {
            name: vName,
            varType: (vDef?.type || 'string') as WorkflowVariable['varType'],
            description: vDef?.description || '',
            required: schemaRequired.includes(vName),
            defaultValue: vDef?.default != null ? String(vDef.default) : '',
            enumValues: Array.isArray(vDef?.enum) ? vDef.enum.map(String) : [],
            minLength: vDef?.minLength,
            maxLength: vDef?.maxLength,
            pattern: vDef?.pattern,
            minimum: vDef?.minimum,
            maximum: vDef?.maximum,
            itemsType: vDef?.items?.type,
            itemProperties: [],
            properties: [],
            xUi: vDef?.['x-ui'] && typeof vDef['x-ui'] === 'object' ? vDef['x-ui'] : undefined,
          };
          if (vDef?.items?.properties && typeof vDef.items.properties === 'object') {
            for (const [pName, pDef] of Object.entries(vDef.items.properties as Record<string, any>)) {
              v.itemProperties!.push({
                name: pName,
                varType: ((pDef as any)?.type || 'string') as WorkflowVariable['varType'],
                description: (pDef as any)?.description || '',
                required: false,
                defaultValue: (pDef as any)?.default != null ? String((pDef as any).default) : '',
                enumValues: Array.isArray((pDef as any)?.enum) ? (pDef as any).enum.map(String) : [],
              });
            }
          }
          if (vDef?.properties && typeof vDef.properties === 'object') {
            for (const [pName, pDef] of Object.entries(vDef.properties as Record<string, any>)) {
              v.properties!.push({
                name: pName,
                varType: ((pDef as any)?.type || 'string') as WorkflowVariable['varType'],
                description: (pDef as any)?.description || '',
                required: false,
                defaultValue: (pDef as any)?.default != null ? String((pDef as any).default) : '',
                enumValues: Array.isArray((pDef as any)?.enum) ? (pDef as any).enum.map(String) : [],
              });
            }
          }
          inputVars.push(v);
        }
        config.input_schema = inputVars;
      } else {
        config.input_schema = [];
      }
    }else if (step.type === 'task' && step.task) {
      node_type = 'task';
      subtype = step.task.kind || 'call_tool';
      Object.assign(config, step.task);
      delete config.kind;
      // Normalize event_gate fields
      if (subtype === 'event_gate') {
        config.topic = config.topic || '';
        config.filter = config.filter || '';
        config.timeout_secs = config.timeout_secs != null ? String(config.timeout_secs) : '';
      }
      // Normalize schedule_task: flatten nested schedule object into config fields
      if (subtype === 'schedule_task' && config.schedule && typeof config.schedule === 'object') {
        const sched = config.schedule as any;
        config.task_name = sched.name || '';
        config.task_description = sched.description || '';
        const cronExpr = sched.schedule || '';
        const action = sched.action || {};
        const actionType = action.type || 'emit_event';
        config.action_type = actionType;
        config.schedule = cronExpr;
        // Flatten action fields by type
        switch (actionType) {
          case 'emit_event':
            config.action_topic = action.topic || '';
            config.action_payload = action.payload ? JSON.stringify(action.payload) : '{}';
            break;
          case 'send_message':
            config.action_session_id = action.session_id || '';
            config.action_content = action.content || '';
            break;
          case 'http_webhook':
            config.action_url = action.url || '';
            config.action_method = action.method || 'POST';
            config.action_body = action.body || '';
            break;
          case 'invoke_agent':
            config.action_persona_id = action.persona_id || '';
            config.action_task = action.task || '';
            config.action_friendly_name = action.friendly_name || '';
            config.action_timeout_secs = action.timeout_secs || 300;
            break;
          case 'call_tool':
            config.action_tool_id = action.tool_id || '';
            config.action_arguments = action.arguments || {};
            break;
          case 'launch_workflow':
            config.action_workflow = action.definition || '';
            config.action_workflow_version = action.version || '';
            config.action_workflow_inputs = action.inputs ? JSON.stringify(action.inputs) : '{}';
            break;
        }
      }
      // Normalize invoke_agent permissions from YAML PermissionEntry → UI format
      if (subtype === 'invoke_agent' && Array.isArray(config.permissions)) {
        config.permissions = config.permissions.map((pe: any) => ({
          tool_pattern: pe.tool_id || pe.tool_pattern || '*',
          scope: pe.resource || pe.scope || '*',
          decision: pe.approval || pe.decision || 'ask',
        }));
      }
      // Normalize invoke_prompt permissions and parameters from YAML
      if (subtype === 'invoke_prompt') {
        if (Array.isArray(config.permissions)) {
          config.permissions = config.permissions.map((pe: any) => ({
            tool_pattern: pe.tool_id || pe.tool_pattern || '*',
            scope: pe.resource || pe.scope || '*',
            decision: pe.approval || pe.decision || 'ask',
          }));
        }
        if (!config.parameters || typeof config.parameters !== 'object') {
          config.parameters = {};
        }
      }
    } else if (step.type === 'control_flow' && step.control) {
      node_type = 'control_flow';
      subtype = step.control.kind || 'branch';
      Object.assign(config, step.control);
      delete config.kind;
      // Structural fields (then/else/body) are derived from edges, not stored in config
      delete config.then;
      delete config.else;
      delete config.body;
    }

    const outputs: Record<string, string> = {};
    if (step.outputs && typeof step.outputs === 'object') {
      for (const [k, v] of Object.entries(step.outputs)) {
        outputs[k] = String(v);
      }
    }

    let onError: DesignerNode['onError'] = null;
    if (step.on_error) {
      onError = {
        strategy: step.on_error.strategy || 'fail_workflow',
        max_retries: step.on_error.max_retries,
        delay_secs: step.on_error.delay_secs,
        fallback_step: step.on_error.fallback_step,
      };
    }

    nodes.push({
      id: step.id,
      type: node_type,
      subtype,
      x: 0,
      y: 0,
      config,
      outputs,
      onError,
    });

    // Build edges from `next` and branch then/else and loop body
    if (step.type === 'control_flow' && step.control?.kind === 'branch') {
      const thenTargets: string[] = step.control.then || [];
      const elseTargets: string[] = step.control.else || [];
      for (const target of thenTargets) {
        edges.push({ id: `e_${step.id}_${target}`, source: step.id, target, edgeType: 'then' });
      }
      for (const target of elseTargets) {
        edges.push({ id: `e_${step.id}_${target}`, source: step.id, target, edgeType: 'else' });
      }
      // Also include any `next` targets that weren't covered by then/else
      if (step.next && Array.isArray(step.next)) {
        const covered = new Set([...thenTargets, ...elseTargets]);
        for (const target of step.next) {
          if (!covered.has(target)) {
            edges.push({ id: `e_${step.id}_${target}`, source: step.id, target });
          }
        }
      }
    } else if (step.type === 'control_flow' && (step.control?.kind === 'for_each' || step.control?.kind === 'while')) {
      const bodyTargets: string[] = step.control.body || [];
      for (const target of bodyTargets) {
        edges.push({ id: `e_${step.id}_${target}_body`, source: step.id, target, edgeType: 'body' });
      }
      if (step.next && Array.isArray(step.next)) {
        for (const target of step.next) {
          edges.push({ id: `e_${step.id}_${target}`, source: step.id, target });
        }
      }
    } else if (step.next && Array.isArray(step.next)) {
      for (const target of step.next) {
        edges.push({ id: `e_${step.id}_${target}`, source: step.id, target });
      }
    }
  }

  // Update counter to avoid conflicts
  idCounter = Math.max(idCounter, nodes.length);

  const attachments: WorkflowAttachment[] = Array.isArray(def.attachments)
    ? def.attachments.map((a: any) => ({
        id: a.id ?? crypto.randomUUID(),
        filename: a.filename ?? '',
        description: a.description ?? '',
        media_type: a.media_type,
        size_bytes: a.size_bytes,
      }))
    : [];

  // Auto-layout
  autoLayout(nodes, edges);

  return { nodes, edges, id, name, version, description, mode, resultMessage, variables, permissions, attachments };
}

function autoLayout(nodes: DesignerNode[], edges: DesignerEdge[]): void {
  if (nodes.length === 0) return;

  const incoming = new Map<string, Set<string>>();
  const outgoing = new Map<string, Set<string>>();
  for (const n of nodes) {
    incoming.set(n.id, new Set());
    outgoing.set(n.id, new Set());
  }
  for (const e of edges) {
    incoming.get(e.target)?.add(e.source);
    outgoing.get(e.source)?.add(e.target);
  }

  // Phase 1: Layer assignment via Kahn's topological sort
  const inDegree = new Map<string, number>();
  for (const n of nodes) inDegree.set(n.id, incoming.get(n.id)?.size ?? 0);

  const queue: string[] = [];
  for (const n of nodes) {
    if ((inDegree.get(n.id) ?? 0) === 0) queue.push(n.id);
  }

  const layers: string[][] = [];
  const visited = new Set<string>();

  while (queue.length > 0) {
    const layer = [...queue];
    layers.push(layer);
    queue.length = 0;
    for (const id of layer) {
      visited.add(id);
      for (const target of outgoing.get(id) ?? []) {
        const d = (inDegree.get(target) ?? 1) - 1;
        inDegree.set(target, d);
        if (d === 0 && !visited.has(target)) queue.push(target);
      }
    }
  }

  for (const n of nodes) {
    if (!visited.has(n.id)) {
      if (layers.length === 0) layers.push([]);
      layers[layers.length - 1].push(n.id);
    }
  }

  // Build layer-index lookup: nodeId → [layerIndex, positionInLayer]
  const layerOf = new Map<string, number>();
  for (let li = 0; li < layers.length; li++) {
    for (const id of layers[li]) layerOf.set(id, li);
  }

  // Phase 2: Barycenter cross-minimization (Sugiyama method)
  // Reorder nodes within each layer to minimize edge crossings.
  const posInLayer = new Map<string, number>();
  function updatePositions(): void {
    for (const layer of layers) {
      for (let i = 0; i < layer.length; i++) posInLayer.set(layer[i], i);
    }
  }
  updatePositions();

  function barycenter(nodeId: string, neighborSet: Set<string>): number {
    if (neighborSet.size === 0) return posInLayer.get(nodeId) ?? 0;
    let sum = 0;
    for (const nid of neighborSet) sum += posInLayer.get(nid) ?? 0;
    return sum / neighborSet.size;
  }

  // Run 4 sweeps (down-up-down-up) for good convergence
  for (let sweep = 0; sweep < 4; sweep++) {
    if (sweep % 2 === 0) {
      // Top-down: order each layer by barycenter of its parents (incoming neighbors)
      for (let li = 1; li < layers.length; li++) {
        layers[li].sort((a, b) => barycenter(a, incoming.get(a)!) - barycenter(b, incoming.get(b)!));
        for (let i = 0; i < layers[li].length; i++) posInLayer.set(layers[li][i], i);
      }
    } else {
      // Bottom-up: order each layer by barycenter of its children (outgoing neighbors)
      for (let li = layers.length - 2; li >= 0; li--) {
        layers[li].sort((a, b) => barycenter(a, outgoing.get(a)!) - barycenter(b, outgoing.get(b)!));
        for (let i = 0; i < layers[li].length; i++) posInLayer.set(layers[li][i], i);
      }
    }
  }

  // Phase 3: Coordinate assignment with median-parent alignment
  const nodeMap = new Map(nodes.map(n => [n.id, n]));
  const LAYER_GAP = 160;
  const NODE_GAP = 180;

  // First pass: assign initial x via median of parents
  const xPos = new Map<string, number>();
  for (let li = 0; li < layers.length; li++) {
    const layer = layers[li];
    for (let ni = 0; ni < layer.length; ni++) {
      const id = layer[ni];
      const parents = incoming.get(id)!;
      if (li === 0 || parents.size === 0) {
        // Root or orphan: use layer position
        xPos.set(id, ni);
      } else {
        // Median of parent positions
        const parentXs = [...parents].map(p => xPos.get(p) ?? 0).sort((a, b) => a - b);
        const mid = Math.floor(parentXs.length / 2);
        xPos.set(id, parentXs.length % 2 === 1
          ? parentXs[mid]
          : (parentXs[mid - 1] + parentXs[mid]) / 2);
      }
    }

    // Resolve overlaps: ensure minimum spacing within each layer
    const sorted = [...layer].sort((a, b) => (xPos.get(a) ?? 0) - (xPos.get(b) ?? 0));
    for (let i = 1; i < sorted.length; i++) {
      const prev = xPos.get(sorted[i - 1])!;
      const curr = xPos.get(sorted[i])!;
      if (curr - prev < 1) xPos.set(sorted[i], prev + 1);
    }
  }

  // Second pass (bottom-up): nudge parents toward median of children
  for (let li = layers.length - 2; li >= 0; li--) {
    const layer = layers[li];
    for (const id of layer) {
      const children = outgoing.get(id)!;
      if (children.size === 0) continue;
      const childXs = [...children].map(c => xPos.get(c) ?? 0).sort((a, b) => a - b);
      const mid = Math.floor(childXs.length / 2);
      const median = childXs.length % 2 === 1
        ? childXs[mid]
        : (childXs[mid - 1] + childXs[mid]) / 2;
      xPos.set(id, (xPos.get(id)! + median) / 2);
    }
    // Re-resolve overlaps
    const sorted = [...layer].sort((a, b) => (xPos.get(a) ?? 0) - (xPos.get(b) ?? 0));
    for (let i = 1; i < sorted.length; i++) {
      const prev = xPos.get(sorted[i - 1])!;
      const curr = xPos.get(sorted[i])!;
      if (curr - prev < 1) xPos.set(sorted[i], prev + 1);
    }
  }

  // Center the graph around x=0 and apply pixel coordinates
  let minX = Infinity, maxX = -Infinity;
  for (const v of xPos.values()) { minX = Math.min(minX, v); maxX = Math.max(maxX, v); }
  const centerOffset = (minX + maxX) / 2;

  for (let li = 0; li < layers.length; li++) {
    for (const id of layers[li]) {
      const node = nodeMap.get(id);
      if (node) {
        node.x = (xPos.get(id)! - centerOffset) * NODE_GAP;
        node.y = li * LAYER_GAP;
      }
    }
  }
}

// ── Component ──────────────────────────────────────────────────────────

const WorkflowDesigner: Component<WorkflowDesignerProps> = (props) => {
  const { safeTimeout } = useTimerCleanup();

  // ── Canvas state ──
  const [nodes, setNodes] = createSignal<DesignerNode[]>([]);
  const [edges, setEdges] = createSignal<DesignerEdge[]>([]);
  const [selectedNodes, setSelectedNodes] = createSignal<Set<string>>(new Set());
  const [wfId, setWfId] = createSignal<string>(crypto.randomUUID());
  const [wfName, setWfName] = createSignal('untitled');
  const [wfVersion, setWfVersion] = createSignal('1');
  const [wfDescription, setWfDescription] = createSignal('');
  const [variables, setVariables] = createSignal<WorkflowVariable[]>([]);
  const [wfPermissions, setWfPermissions] = createSignal<WfPermissionRule[]>([]);
  const [wfAttachments, setWfAttachments] = createSignal<WorkflowAttachment[]>([]);
  const [wfMode, setWfMode] = createSignal<string>('background');
  const [wfResultMessage, setWfResultMessage] = createSignal<string>('');

  // ── Viewport (synced from GraphCanvas for UI display) ──
  const [panX, setPanX] = createSignal(0);
  const [panY, setPanY] = createSignal(0);
  const [zoom, setZoom] = createSignal(1);

  // ── UI state (not related to canvas rendering) ──
  const [showYaml, setShowYaml] = createSignal(false);
  const [showAttachments, setShowAttachments] = createSignal(false);
  const [showDescPopup, setShowDescPopup] = createSignal(false);
  const [toast, setToast] = createSignal<{ text: string; type: 'success' | 'error' } | null>(null);
  const [snapEnabled, setSnapEnabled] = createSignal(true);
  const [paletteDrag, setPaletteDrag] = createSignal<PaletteItem | null>(null);
  const [paletteDragPos, setPaletteDragPos] = createSignal<{ x: number; y: number } | null>(null);

  // Single shared signal for expression helper popups — only one can be open at a time
  const [openExprHelperId, setOpenExprHelperId] = createSignal<string | null>(null);

  // ── AI Assist state ──
  const [aiAssistOpen, setAiAssistOpen] = createSignal(true);
  const [aiAssistPrompt, setAiAssistPrompt] = createSignal('');
  const [aiAssistResponse, setAiAssistResponse] = createSignal('');
  const [aiAssistLoading, setAiAssistLoading] = createSignal(false);
  const [aiAssistAgentId, setAiAssistAgentId] = createSignal<string | null>(null);
  const [aiAssistPanelHeight, setAiAssistPanelHeight] = createSignal(240);
  let aiAssistUnsubscribe: (() => void) | null = null;
  let aiAssistResponseEl: HTMLDivElement | undefined;

  // Question state for the AI assistant's ask_user calls (AiQuestion type imported from AiAssistPanel)
  const [aiPendingQuestion, setAiPendingQuestion] = createSignal<AiQuestion | null>(null);
  const [aiQuestionFreeform, setAiQuestionFreeform] = createSignal('');
  const [aiQuestionSubmitting, setAiQuestionSubmitting] = createSignal(false);

  // Auto-scroll AI assist response area when content changes
  createEffect(() => {
    aiAssistResponse();
    aiPendingQuestion();
    if (aiAssistResponseEl) aiAssistResponseEl.scrollTop = aiAssistResponseEl.scrollHeight;
  });

  async function answerAiQuestion(selected_choice?: number, text?: string, selected_choices?: number[]) {
    const q = aiPendingQuestion();
    if (!q || aiQuestionSubmitting()) return;
    setAiQuestionSubmitting(true);
    let answerLabel: string;
    if (selected_choices && selected_choices.length > 0) {
      answerLabel = selected_choices.map((i) => q.choices[i]).join(', ');
    } else {
      answerLabel = text || (selected_choice !== undefined ? q.choices[selected_choice] : '');
    }
    try {
      await invoke('bot_interaction', {
        agent_id: q.agent_id,
        response: {
          request_id: q.request_id,
          payload: {
            type: 'answer' as const,
            ...(selected_choice !== undefined ? { selected_choice } : {}),
            ...(selected_choices !== undefined ? { selected_choices } : {}),
            ...(text ? { text } : {}),
          },
        },
      });
      setAiAssistResponse((prev) => prev + `\n✓ Answered: ${answerLabel}\n`);
      setAiPendingQuestion(null);
      setAiQuestionFreeform('');
    } catch (error) {
      console.error('Failed to answer AI question:', error);
      setAiAssistResponse((prev) => prev + `\n✗ Failed to send answer\n`);
    } finally {
      setAiQuestionSubmitting(false);
    }
  }

  function cleanupAiAssistAgent() {
    const agent_id = aiAssistAgentId();
    if (agent_id && props.onAiAssistCleanup) {
      props.onAiAssistCleanup(agent_id);
    }
    setAiAssistAgentId(null);
    setAiPendingQuestion(null);
    setAiQuestionFreeform('');
    if (aiAssistUnsubscribe) { aiAssistUnsubscribe(); aiAssistUnsubscribe = null; }
  }

  onCleanup(() => {
    cleanupAiAssistAgent();
  });

  async function handleAiAssist() {
    const prompt = aiAssistPrompt().trim();
    if (!prompt || !props.onAiAssist) return;

    const existingAgentId = aiAssistAgentId();
    const isFollowUp = !!existingAgentId;

    setAiAssistLoading(true);
    if (isFollowUp) {
      setAiAssistResponse((prev) => prev + `\n\n> ${prompt}\n\n`);
    } else {
      setAiAssistResponse('Launching AI assistant...\n');
    }
    setAiAssistPrompt('');

    try {
      const { agent_id } = await props.onAiAssist(yamlOutput(), prompt, existingAgentId);

      if (!isFollowUp || agent_id !== existingAgentId) {
        // New agent was created — set up subscription
        setAiAssistAgentId(agent_id);
        setAiAssistResponse((prev) => prev + `✓ Agent ${agent_id} launched\n`);

        if (props.onAiAssistSubscribe) {
          if (aiAssistUnsubscribe) { aiAssistUnsubscribe(); }
          let yamlApplied = false;
          aiAssistUnsubscribe = props.onAiAssistSubscribe(agent_id, {
            onToken: (delta) => {
              setAiAssistResponse((prev) => prev + delta);
            },
            onToolCall: (tool_id, rawInput) => {
              // Show progress for discovery tools
              const toolLabels: Record<string, string> = {
                'workflow_author.list_available_tools': '🔍 Discovering available tools...',
                'workflow_author.get_tool_details': '🔍 Getting tool details...',
                'workflow_author.suggest_tools': '🔍 Finding relevant tools...',
                'workflow_author.list_connectors': '🔌 Checking connectors...',
                'workflow_author.list_personas': '🤖 Listing personas...',
                'workflow_author.list_event_topics': '📡 Discovering event topics...',
                'workflow_author.list_workflows': '📋 Checking existing workflows...',
                'workflow_author.get_template': '📄 Loading workflow template...',
                'workflow_author.lint_workflow': '🔎 Checking workflow quality...',
                'core.ask_user': '', // handled separately via onQuestion
              };
              if (tool_id in toolLabels && toolLabels[tool_id]) {
                setAiAssistResponse((prev) => prev + toolLabels[tool_id] + '\n');
              }

              if (tool_id === 'workflow_author.submit_workflow' && rawInput) {
                // Input may be a JSON string or an object
                let input = rawInput;
                if (typeof input === 'string') {
                  try { input = JSON.parse(input); } catch { /* keep as-is */ }
                }
                const yaml = typeof input === 'string'
                  ? input
                  : (input.yaml || '');
                const message = typeof input === 'string'
                  ? ''
                  : (input.message || '');

                if (yaml) {
                  yamlApplied = true;
                  const result = fromYaml(yaml);
                  if (result) {
                    pushHistory();
                    batch(() => {
                      setNodes(result.nodes);
                      setEdges(result.edges);
                      setWfId(result.id);
                      setWfName(result.name);
                      setWfVersion(result.version);
                      setWfDescription(result.description);
                      setWfMode(result.mode ?? 'background');
                      setWfResultMessage(result.resultMessage ?? '');
                      if (result.variables) setVariables(result.variables);
                      if (result.permissions) setWfPermissions(result.permissions);
                      if (result.attachments) setWfAttachments(result.attachments);
                    });
                    setAiAssistResponse((prev) => prev + '\n✓ Workflow updated!\n');
                    showToast('Workflow updated by AI', 'success');
                  } else {
                    setAiAssistResponse((prev) => prev + '\n✗ AI returned invalid YAML\n');
                    showToast('AI returned invalid YAML', 'error');
                  }
                }
                if (message) {
                  setAiAssistResponse((prev) => prev + '\n' + message + '\n');
                }
              }
            },
            onDone: () => {
              setAiAssistLoading(false);
              // Keep aiAssistAgentId — the agent is still alive (IdleAfterTask)
              if (!aiAssistResponse().includes('✓ Workflow updated') && !aiAssistResponse().includes('✗')) {
                setAiAssistResponse((prev) => prev + '\n✓ Done\n');
              }
            },
            onError: (error) => {
              setAiAssistLoading(false);
              setAiAssistAgentId(null);
              setAiAssistResponse((prev) => prev + `\n✗ Error: ${error}\n`);
            },
            onQuestion: (request_id, agent_id, text, choices, allow_freeform, message) => {
              if (message) {
                setAiAssistResponse((prev) => prev + '\n' + message + '\n');
              }
              setAiPendingQuestion({ request_id, agent_id, text, choices, allow_freeform, message });
              setAiQuestionFreeform('');
            },
          });
        }
      } else {
        // Follow-up to existing agent — subscription already active, just reset yamlApplied
      }
    } catch (e: any) {
      setAiAssistLoading(false);
      setAiAssistResponse((prev) => prev + `\n✗ Error: ${e?.message || e?.toString() || 'Unknown error'}\n`);
    }
  }

  // Auto-trigger AI assist when initialAiPrompt is provided
  let aiAutoTriggered = false;
  createEffect(() => {
    const prompt = props.initialAiPrompt;
    if (prompt && !aiAutoTriggered && props.onAiAssist) {
      aiAutoTriggered = true;
      setAiAssistOpen(true);
      setAiAssistPrompt(prompt);
      // Delay to let the designer fully mount before triggering
      safeTimeout(() => void handleAiAssist(), 300);
    }
  });

  // ── Undo/Redo ──
  let historyStack: HistoryEntry[] = [];
  let historyIndex = -1;
  let suppressHistory = false;

  function captureState(): HistoryEntry {
    return {
      nodes: JSON.parse(JSON.stringify(nodes())),
      edges: JSON.parse(JSON.stringify(edges())),
      variables: JSON.parse(JSON.stringify(variables())),
      permissions: JSON.parse(JSON.stringify(wfPermissions())),
      attachments: JSON.parse(JSON.stringify(wfAttachments())),
      wfName: wfName(),
      wfVersion: wfVersion(),
      wfDescription: wfDescription(),
      wfMode: wfMode(),
      wfResultMessage: wfResultMessage(),
    };
  }

  function pushHistory(): void {
    if (suppressHistory) return;
    const entry = captureState();
    // Simple deduplicate: compare serialized state (defer expensive comparison)
    const entryJson = JSON.stringify(entry);
    if (historyIndex >= 0 && historyStack.length > historyIndex) {
      if (historyStack[historyIndex]._json === entryJson) return;
    }
    entry._json = entryJson;
    historyStack = historyStack.slice(0, historyIndex + 1);
    historyStack.push(entry);
    if (historyStack.length > MAX_HISTORY) historyStack.shift();
    historyIndex = historyStack.length - 1;
  }

  function restoreState(entry: HistoryEntry): void {
    suppressHistory = true;
    batch(() => {
      setNodes(entry.nodes);
      setEdges(entry.edges);
      setVariables(entry.variables);
      setWfPermissions(entry.permissions ?? []);
      setWfAttachments(entry.attachments ?? []);
      setWfName(entry.wfName);
      setWfVersion(entry.wfVersion);
      setWfDescription(entry.wfDescription);
      setWfMode(entry.wfMode ?? 'background');
      setWfResultMessage(entry.wfResultMessage ?? '');
    });
    suppressHistory = false;
  }

  function undo(): void {
    if (historyIndex <= 0) return;
    historyIndex--;
    restoreState(historyStack[historyIndex]);
  }

  function redo(): void {
    if (historyIndex >= historyStack.length - 1) return;
    historyIndex++;
    restoreState(historyStack[historyIndex]);
  }

  let canvasContainerRef: HTMLDivElement | undefined;
  let containerRef: HTMLDivElement | undefined;
  let resultMsgInputRef: HTMLInputElement | undefined;
  let graphCanvas: GraphCanvas | null = null;

  // ── Derived ──
  const selectedNodeId = createMemo(() => {
    const sel = selectedNodes();
    if (sel.size !== 1) return null;
    return sel.values().next().value ?? null;
  });

  // O(1) node lookups — used by right panel and dialogs
  const nodeMap = createMemo(() => {
    const map = new Map<string, DesignerNode>();
    for (const n of nodes()) map.set(n.id, n);
    return map;
  });

  // ── Debounced YAML generation ──
  const [yamlOutput, setYamlOutput] = createSignal('');
  const [savedYamlSnapshot, setSavedYamlSnapshot] = createSignal('');
  const isDirty = createMemo(() => {
    const current = yamlOutput();
    const saved = savedYamlSnapshot();
    return current !== '' && saved !== '' && current !== saved;
  });
  let yamlGenTimer: ReturnType<typeof setTimeout> | null = null;
  createEffect(() => {
    nodes(); edges(); wfId(); wfName(); wfVersion(); wfDescription(); wfMode(); wfResultMessage(); variables(); wfPermissions(); wfAttachments();
    if (yamlGenTimer) clearTimeout(yamlGenTimer);
    yamlGenTimer = setTimeout(() => {
      try {
        const n = untrack(nodes);
        const e = untrack(edges);
        if (n.length === 0 && !props.initialYaml) { setYamlOutput(''); return; }
        const yaml = toYaml(n, e, untrack(wfId), untrack(wfName), untrack(wfVersion), untrack(wfDescription), untrack(wfMode), untrack(wfResultMessage), untrack(variables), untrack(wfPermissions), untrack(wfAttachments));
        setYamlOutput(yaml);
      } catch (err) {
        console.error('[WorkflowDesigner] toYaml error:', err);
        setYamlOutput('# Error generating YAML');
      }
    }, 250);
  });
  onCleanup(() => { if (yamlGenTimer) clearTimeout(yamlGenTimer); });

  // ── Debounced validation ──
  const [validationErrors, setValidationErrors] = createSignal<Record<string, string[]>>({});
  let validationTimer: ReturnType<typeof setTimeout> | null = null;
  createEffect(() => {
    nodes(); // track
    if (validationTimer) clearTimeout(validationTimer);
    validationTimer = setTimeout(() => {
      try {
        const errors: Record<string, string[]> = {};
        for (const node of untrack(nodes)) {
          const fields = requiredFields(node.subtype);
          const missing = fields.filter(f => {
            const val = node.config[f];
            return val === undefined || val === null || val === '';
          });
          if (missing.length > 0) errors[node.id] = missing;
        }
        setValidationErrors(errors);
      } catch (err) {
        console.error('[WorkflowDesigner] validationErrors error:', err);
      }
    }, 100);
  });
  onCleanup(() => { if (validationTimer) clearTimeout(validationTimer); });

  function edgesForNode(nodeId: string): { incoming: DesignerEdge[]; outgoing: DesignerEdge[] } {
    const all = edges();
    return {
      incoming: all.filter(e => e.target === nodeId),
      outgoing: all.filter(e => e.source === nodeId),
    };
  }

  // ── Initialize from YAML ──
  onMount(() => {
    if (props.initialYaml) {
      try {
        const parsed = fromYaml(props.initialYaml);
        setNodes(parsed.nodes);
        setEdges(parsed.edges);
        setWfId(parsed.id);
        setWfName(parsed.name);
        setWfVersion(parsed.version);
        setWfDescription(parsed.description);
        setWfMode(parsed.mode ?? 'background');
        setWfResultMessage(parsed.resultMessage ?? '');
        setVariables(parsed.variables);
        setWfPermissions(parsed.permissions);
        setWfAttachments((parsed.attachments ?? []).map((a: any) => ({
          id: a.id ?? crypto.randomUUID(),
          filename: a.filename ?? '',
          description: a.description ?? '',
          media_type: a.media_type,
          size_bytes: a.size_bytes,
        })));
      } catch { /* start empty */ }
    }
    pushHistory();
    // Snapshot initial YAML for dirty tracking (after a tick so YAML generation runs)
    safeTimeout(() => setSavedYamlSnapshot(yamlOutput()), 350);
  });

  // ── GraphCanvas initialization (runs after DOM mount) ──
  onMount(() => {
    if (!canvasContainerRef) return;
    graphCanvas = new GraphCanvas(canvasContainerRef, {
      onNodeClick: (nodeId, shiftKey) => {
        if (shiftKey) {
          const sel = new Set(selectedNodes());
          if (sel.has(nodeId)) sel.delete(nodeId);
          else sel.add(nodeId);
          setSelectedNodes(sel);
        } else {
          if (!selectedNodes().has(nodeId)) setSelectedNodes(new Set([nodeId]));
          else setSelectedNodes(new Set([nodeId]));
        }
      },
      onNodeDoubleClick: (nodeId) => {
        setStepConfigNodeId(nodeId);
        setShowStepConfigDialog(true);
      },
      onBackgroundClick: () => {
        setSelectedNodes(new Set<string>());
      },
      onNodesMove: (updates) => {
        const uMap = new Map(updates.map(u => [u.id, u]));
        setNodes(prev => prev.map(n => {
          const u = uMap.get(n.id);
          return u ? { ...n, x: u.x, y: u.y } : n;
        }));
        pushHistory();
      },
      onEdgeCreate: (source_id, targetId, edgeType) => {
        addEdge(source_id, targetId, edgeType as any);
      },
      onEdgeDoubleClick: (edgeId) => {
        removeEdge(edgeId);
      },
      onSelectionRect: (nodeIds) => {
        setSelectedNodes(new Set(nodeIds));
      },
      onViewChange: (px, py, z) => {
        setPanX(px);
        setPanY(py);
        setZoom(z);
      },
    });

    // Push initial data
    graphCanvas.setNodes(nodes() as CanvasNode[]);
    graphCanvas.setEdges(edges() as CanvasEdge[]);
  });
  onCleanup(() => {
    graphCanvas?.destroy();
    graphCanvas = null;
  });

  // ── Push data to GraphCanvas when signals change ──
  createEffect(() => { graphCanvas?.setNodes(nodes() as CanvasNode[]); });
  createEffect(() => { graphCanvas?.setEdges(edges() as CanvasEdge[]); });
  createEffect(() => { graphCanvas?.setSelectedNodes(selectedNodes()); });
  createEffect(() => { graphCanvas?.setSnapEnabled(snapEnabled()); });
  createEffect(() => { graphCanvas?.setReadOnly(props.readOnly ?? false); });
  createEffect(() => { graphCanvas?.setStepStates(props.instanceStepStates ?? {}); });

  // ── Emit YAML to parent when yamlOutput signal updates ──
  let yamlEmitTimer: ReturnType<typeof setTimeout> | null = null;
  createEffect(() => {
    const yaml = yamlOutput();
    if (!yaml) return;
    // Small debounce to coalesce rapid updates
    if (yamlEmitTimer) clearTimeout(yamlEmitTimer);
    yamlEmitTimer = setTimeout(() => {
      try { props.onYamlChange?.(yaml); }
      catch (err) { console.error('[WorkflowDesigner] onYamlChange error:', err); }
    }, 50);
  });
  onCleanup(() => { if (yamlEmitTimer) clearTimeout(yamlEmitTimer); });

  // ── Keyboard handler ──
  function bumpVersion(): void {
    const v = wfVersion();
    const parts = v.split('.');
    for (let i = parts.length - 1; i >= 0; i--) {
      const n = parseInt(parts[i], 10);
      if (!isNaN(n)) {
        parts[i] = String(n + 1);
        setWfVersion(parts.join('.'));
        return;
      }
    }
    setWfVersion(v + '.1');
  }

  /** Flush YAML generation synchronously (bypasses the 250ms debounce) and emit to parent. */
  function flushYaml(): void {
    if (yamlGenTimer) clearTimeout(yamlGenTimer);
    try {
      const n = untrack(nodes);
      const e = untrack(edges);
      if (n.length === 0 && !props.initialYaml) { setYamlOutput(''); return; }
      const y = toYaml(n, e, untrack(wfId), untrack(wfName), untrack(wfVersion), untrack(wfDescription), untrack(wfMode), untrack(wfResultMessage), untrack(variables), untrack(wfPermissions), untrack(wfAttachments));
      setYamlOutput(y);
      // Also flush the emit debounce so the parent has the latest YAML
      if (yamlEmitTimer) clearTimeout(yamlEmitTimer);
      props.onYamlChange?.(y);
    } catch { /* ignore */ }
  }

  function triggerSave(): void {
    if (props.readOnly || !props.onSave) return;
    bumpVersion();
    flushYaml();
    setSavedYamlSnapshot(yamlOutput());
    props.onSave();
  }

  function handleKeyDown(e: KeyboardEvent): void {
    // Ctrl+S: save (works even from inputs)
    if ((e.ctrlKey || e.metaKey) && e.key === 's' && !e.shiftKey) {
      e.preventDefault();
      triggerSave();
      return;
    }

    if ((e.target as HTMLElement)?.tagName === 'INPUT' || (e.target as HTMLElement)?.tagName === 'TEXTAREA' || (e.target as HTMLElement)?.tagName === 'SELECT') return;

    if ((e.ctrlKey || e.metaKey) && e.key === 'z' && !e.shiftKey) {
      e.preventDefault();
      undo();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && (e.key === 'y' || (e.key === 'z' && e.shiftKey))) {
      e.preventDefault();
      redo();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key === 'a') {
      e.preventDefault();
      setSelectedNodes(new Set(nodes().map(n => n.id)));
      return;
    }
    // Ctrl+Shift+L: auto-layout
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key === 'L') {
      e.preventDefault();
      applyAutoLayout();
      return;
    }

    if (props.readOnly) return;

    if (e.key === 'Delete' || e.key === 'Backspace') {
      const sel = selectedNodes();
      if (sel.size > 0) {
        batch(() => {
          setNodes(prev => prev.filter(n => !sel.has(n.id)));
          setEdges(prev => prev.filter(e => !sel.has(e.source) && !sel.has(e.target)));
          setSelectedNodes(new Set<string>());
        });
        pushHistory();
      }
      return;
    }

    // Arrow key nudge
    const NUDGE = snapEnabled() ? GRID_SIZE : 5;
    const sel = selectedNodes();
    if (sel.size === 0) return;
    let dx = 0, dy = 0;
    if (e.key === 'ArrowUp') dy = -NUDGE;
    else if (e.key === 'ArrowDown') dy = NUDGE;
    else if (e.key === 'ArrowLeft') dx = -NUDGE;
    else if (e.key === 'ArrowRight') dx = NUDGE;
    if (dx || dy) {
      e.preventDefault();
      batch(() => {
        setNodes(prev => prev.map(n => sel.has(n.id) ? { ...n, x: n.x + dx, y: n.y + dy } : n));
      });
      pushHistory();
    }
  }

  onMount(() => {
    document.addEventListener('keydown', handleKeyDown);
    onCleanup(() => document.removeEventListener('keydown', handleKeyDown));

    // Global error handlers to diagnose freezes/black screens
    const onGlobalError = (event: ErrorEvent) => {
      console.error('[WorkflowDesigner] Uncaught error:', event.error);
    };
    const onUnhandledRejection = (event: PromiseRejectionEvent) => {
      console.error('[WorkflowDesigner] Unhandled rejection:', event.reason);
    };
    window.addEventListener('error', onGlobalError);
    window.addEventListener('unhandledrejection', onUnhandledRejection);
    onCleanup(() => {
      window.removeEventListener('error', onGlobalError);
      window.removeEventListener('unhandledrejection', onUnhandledRejection);
    });
  });

  // ── Fit to view (delegates to canvas) ──
  function fitToView(): void {
    graphCanvas?.fitToView();
  }

  // ── Node operations ──
  function addNode(item: PaletteItem, x?: number, y?: number): void {
    if (props.readOnly) return;
    const existing = nodes();
    const id = nextId(item.subtype);
    const maxY = existing.length > 0 ? Math.max(...existing.map(n => n.y)) + 120 : 0;
    const posX = x !== undefined ? (snapEnabled() ? snapToGrid(x) : x) : panX() - NODE_MIN_W / 2;
    const posY = y !== undefined ? (snapEnabled() ? snapToGrid(y) : y) : (existing.length === 0 ? panY() : maxY);
    const node: DesignerNode = {
      id,
      type: item.type,
      subtype: item.subtype,
      x: posX,
      y: posY,
      config: defaultConfig(item.subtype),
      outputs: {},
      onError: null,
    };
    batch(() => {
      setNodes(prev => [...prev, node]);
      setSelectedNodes(new Set([id]));
    });
    pushHistory();
  }

  function deleteNode(nodeId: string): void {
    if (props.readOnly) return;
    batch(() => {
      setNodes(prev => prev.filter(n => n.id !== nodeId));
      setEdges(prev => prev.filter(e => e.source !== nodeId && e.target !== nodeId));
      const sel = selectedNodes();
      if (sel.has(nodeId)) {
        const next = new Set(sel);
        next.delete(nodeId);
        setSelectedNodes(next);
      }
    });
    pushHistory();
  }

  function updateNode(nodeId: string, updates: Partial<DesignerNode>): void {
    setNodes(prev => prev.map(n => n.id === nodeId ? { ...n, ...updates } : n));
  }

  function updateNodeConfig(nodeId: string, key: string, value: any): void {
    setNodes(prev => prev.map(n => {
      if (n.id !== nodeId) return n;
      return { ...n, config: { ...n.config, [key]: value } };
    }));
  }

  function renameNode(oldId: string, newId: string): void {
    if (props.readOnly) return;
    if (!newId || newId === oldId) return;
    if (nodes().some(n => n.id === newId)) return;
    batch(() => {
      setNodes(prev => prev.map(n => n.id === oldId ? { ...n, id: newId } : n));
      setEdges(prev => prev.map(e => ({
        ...e,
        id: e.id.replace(oldId, newId),
        source: e.source === oldId ? newId : e.source,
        target: e.target === oldId ? newId : e.target,
      })));
      const sel = selectedNodes();
      if (sel.has(oldId)) {
        const next = new Set(sel);
        next.delete(oldId);
        next.add(newId);
        setSelectedNodes(next);
      }
    });
    pushHistory();
  }

  function addEdge(source: string, target: string, edgeType?: 'then' | 'else' | 'body'): void {
    if (props.readOnly) return;
    if (source === target) return;
    if (edges().some(e => e.source === source && e.target === target)) return;
    setEdges(prev => [...prev, { id: `e_${source}_${target}`, source, target, edgeType: edgeType ?? 'default' }]);
    pushHistory();
  }

  function removeEdge(edgeId: string): void {
    if (props.readOnly) return;
    setEdges(prev => prev.filter(e => e.id !== edgeId));
    pushHistory();
  }

  function applyAutoLayout(): void {
    if (props.readOnly) return;
    const ns = [...nodes()];
    autoLayout(ns, edges());
    setNodes(ns);
    pushHistory();
  }

  // ── Toast helper ──
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  onCleanup(() => { if (toastTimer) clearTimeout(toastTimer); });
  function showToast(text: string, type: 'success' | 'error'): void {
    setToast({ text, type });
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => setToast(null), 3000);
  }

  // ── Palette drag handlers ──
  let dragCleanup: (() => void) | null = null;
  onCleanup(() => { if (dragCleanup) dragCleanup(); });

  function handlePaletteDragStart(e: MouseEvent, item: PaletteItem): void {
    if (props.readOnly) return;
    e.preventDefault();
    setPaletteDrag(item);
    setPaletteDragPos({ x: e.clientX, y: e.clientY });

    const handleMove = (ev: MouseEvent) => {
      setPaletteDragPos({ x: ev.clientX, y: ev.clientY });
    };
    const cleanup = () => {
      document.removeEventListener('mousemove', handleMove);
      document.removeEventListener('mouseup', handleUp);
      dragCleanup = null;
    };
    const handleUp = (ev: MouseEvent) => {
      cleanup();
      const dragItem = paletteDrag();
      setPaletteDrag(null);
      setPaletteDragPos(null);
      if (!dragItem || !graphCanvas) return;
      const canvas = graphCanvas.getCanvas();
      const rect = canvas.getBoundingClientRect();
      if (ev.clientX >= rect.left && ev.clientX <= rect.right &&
          ev.clientY >= rect.top && ev.clientY <= rect.bottom) {
        const pt = graphCanvas.screenToGraph(ev.clientX, ev.clientY);
        addNode(dragItem, pt.x - NODE_MIN_W / 2, pt.y - NODE_H / 2);
      }
    };
    document.addEventListener('mousemove', handleMove);
    document.addEventListener('mouseup', handleUp);
    dragCleanup = cleanup;
  }

  // ── Styles ──
  const sectionHeaderStyle = {
    padding: '8px 10px',
    'font-size': '0.85em',
    'font-weight': '600',
    'text-transform': 'uppercase' as const,
    'letter-spacing': '0.05em',
    color: 'hsl(var(--muted-foreground))',
    'border-bottom': '1px solid hsl(var(--border))',
    cursor: 'default',
    'user-select': 'none' as const,
  };

  const paletteItemStyle = {
    padding: '5px 10px',
    'font-size': '0.8em',
    color: 'hsl(var(--foreground))',
    cursor: 'grab',
    display: 'flex',
    'align-items': 'center',
    gap: '6px',
    'border-bottom': '1px solid hsl(var(--border))',
    transition: 'background 0.15s',
    'user-select': 'none' as const,
  };

  // inputStyle, labelStyle imported from workflow/types

  // ── Variable operations ──
  function addVariable(): void {
    setVariables(prev => [...prev, {
      name: `var_${prev.length + 1}`,
      varType: 'string' as const,
      description: '',
      required: false,
      defaultValue: '',
      enumValues: [],
    }]);
    pushHistory();
  }

  function removeVariable(idx: number): void {
    setVariables(prev => prev.filter((_, i) => i !== idx));
    pushHistory();
  }

  function updateVariable(idx: number, field: keyof WorkflowVariable, value: any): void {
    setVariables(prev => prev.map((v, i) => i === idx ? { ...v, [field]: value } : v));
  }

  // ── Output mapping helpers ──
  function addOutputMapping(nodeId: string): void {
    setNodes(prev => prev.map(n => {
      if (n.id !== nodeId) return n;
      const key = `out_${Object.keys(n.outputs).length + 1}`;
      return { ...n, outputs: { ...n.outputs, [key]: '' } };
    }));
  }

  function removeOutputMapping(nodeId: string, key: string): void {
    setNodes(prev => prev.map(n => {
      if (n.id !== nodeId) return n;
      const newOut = { ...n.outputs };
      delete newOut[key];
      return { ...n, outputs: newOut };
    }));
  }

  function updateOutputMapping(nodeId: string, oldKey: string, newKey: string, value: string): void {
    setNodes(prev => prev.map(n => {
      if (n.id !== nodeId) return n;
      const newOut = { ...n.outputs };
      if (oldKey !== newKey) delete newOut[oldKey];
      newOut[newKey] = value;
      return { ...n, outputs: newOut };
    }));
  }

  // ── Render: Left Panel ──
  function renderPalette() {
    return (
      <div style={{ flex: '1', 'overflow-y': 'auto' }}>
        <For each={PALETTE_ITEMS}>
          {(group) => (
            <>
              <div style={sectionHeaderStyle}>{group.category}</div>
              <For each={group.items.filter(item => item.type !== 'trigger' || item.subtype === 'manual' || wfMode() !== 'chat')}>
                {(item) => (
                  <div
                    style={paletteItemStyle}
                    onMouseDown={(e) => handlePaletteDragStart(e, item)}
                    onClick={() => addNode(item)}
                    onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.background = 'hsl(var(--background))'; }}
                    onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.background = 'transparent'; }}
                    title={`Drag or click to add ${item.label}`}
                  >
                    <SubtypeIcon subtype={item.subtype} size={16} />
                    <span>{item.label}</span>
                  </div>
                )}
              </For>
            </>
          )}
        </For>
      </div>
    );
  }

  const [showVarDialog, setShowVarDialog] = createSignal(false);
  const [showStepConfigDialog, setShowStepConfigDialog] = createSignal(false);
  const [stepConfigNodeId, setStepConfigNodeId] = createSignal<string | null>(null);

  const TYPE_LABELS: Record<string, string> = {
    'string': 'Text',
    'number': 'Number',
    'boolean': 'Boolean',
    'object': 'Object',
    'array': 'List',
  };
  const TYPE_VALUES = ['string', 'number', 'boolean', 'object', 'array'] as const;

  // Available expression references for the helper dropdown
  function getExpressionReferences(): { label: string; value: string; group: string }[] {
    const refs: { label: string; value: string; group: string }[] = [];
    // Workflow variables
    for (const v of variables()) {
      refs.push({ label: v.name, value: `{{variables.${v.name}}}`, group: 'Variables' });
    }
    // Step outputs — include typed output fields based on step type
    for (const n of nodes()) {
      // For manual triggers, expose auto-generated outputs from input_schema
      if (n.type === 'trigger' && n.subtype === 'manual') {
        const triggerInputSchema: WorkflowVariable[] = n.config?.input_schema ?? [];
        for (const v of triggerInputSchema) {
          refs.push({ label: `${n.id} → ${v.name}`, value: `{{steps.${n.id}.outputs.${v.name}}}`, group: 'Trigger Inputs' });
        }
        continue;
      }
      // For incoming_message triggers, expose message fields
      if (n.type === 'trigger' && n.subtype === 'incoming_message') {
        const msgFields = [
          { name: 'from', description: 'Sender address' },
          { name: 'to', description: 'Recipients (array)' },
          { name: 'subject', description: 'Message subject' },
          { name: 'body', description: 'Message body' },
          { name: 'external_id', description: 'Provider message ID' },
          { name: 'channel_id', description: 'Connector ID' },
          { name: 'provider', description: 'Provider type' },
          { name: 'timestamp_ms', description: 'Timestamp (ms)' },
        ];
        for (const f of msgFields) {
          refs.push({ label: `${n.id} → ${f.name}`, value: `{{steps.${n.id}.outputs.${f.name}}}`, group: 'Message Fields' });
        }
        continue;
      }
      // For event triggers, expose event payload
      if (n.type === 'trigger' && n.subtype === 'event') {
        refs.push({ label: `${n.id} → payload`, value: `{{steps.${n.id}.outputs}}`, group: 'Event Data' });
        continue;
      }
      if (n.type === 'trigger') continue;
      // User-defined output mappings
      if (Object.keys(n.outputs).length > 0) {
        for (const key of Object.keys(n.outputs)) {
          refs.push({ label: `${n.id} → ${key}`, value: `{{steps.${n.id}.outputs.${key}}}`, group: 'Step Outputs' });
        }
      }
      // Typed output fields from step schema
      const hints = getStepOutputHints(n);
      if (hints.fields.length > 0) {
        for (const f of hints.fields) {
          refs.push({ label: `${n.id} → ${f.name}`, value: `{{steps.${n.id}.outputs.${f.name}}}`, group: 'Step Results' });
        }
      } else {
        refs.push({ label: n.id, value: `{{steps.${n.id}.outputs.result}}`, group: 'Step Results' });
      }
    }
    // Trigger data
    refs.push({ label: 'trigger (all)', value: '{{trigger}}', group: 'Trigger' });
    // Current result/error
    refs.push({ label: 'result', value: '{{result}}', group: 'Current' });
    refs.push({ label: 'error', value: '{{error}}', group: 'Current' });
    return refs;
  }

  function renderExpressionHelper(
    onInsert: (text: string) => void,
    inputEl?: () => HTMLInputElement | HTMLTextAreaElement | undefined,
  ) {
    // Each instance gets a stable ID — the shared signal ensures only one popup is open at a time
    // Module-level counter ensures IDs are stable across re-renders.
    const myId = `expr_${_exprHelperNextId++}`;
    const isOpen = () => openExprHelperId() === myId;

    const refs = () => {
      const raw = getExpressionReferences();
      const grouped: Record<string, { label: string; value: string }[]> = {};
      for (const r of raw) {
        if (!grouped[r.group]) grouped[r.group] = [];
        grouped[r.group].push(r);
      }
      return Object.entries(grouped);
    };

    return (
      <Popover
        placement="bottom-end"
        open={isOpen()}
        onOpenChange={(open) => setOpenExprHelperId(open ? myId : null)}
      >
        <PopoverTrigger
          as="button"
          style={{
            background: 'none', border: '1px solid hsl(var(--border))',
            color: 'hsl(var(--primary))', cursor: 'pointer', 'border-radius': '3px',
            padding: '1px 5px', 'font-size': '0.7em', 'margin-left': '4px',
          }}
          title="Insert variable reference"
        >{'{{}}'}</PopoverTrigger>
        <PopoverContent class="w-auto p-0" style={{
          'z-index': '10000',
          background: 'hsl(var(--card))', border: '1px solid hsl(var(--border))',
          'border-radius': '6px', 'box-shadow': '0 4px 12px hsl(var(--foreground) / 0.15)',
          'min-width': '220px', 'max-height': '260px', 'overflow-y': 'auto',
          padding: '4px 0',
        }}>
        <For each={refs()}>
          {([group, items]) => (<>
            <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', padding: '4px 10px 2px', 'font-weight': '600', 'text-transform': 'uppercase', 'letter-spacing': '0.5px' }}>
              {group}
            </div>
            <For each={items}>
              {(item) => (
                <button
                  onMouseDown={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    const el = inputEl?.();
                    if (el) {
                      const start = el.selectionStart ?? el.value.length;
                      const end = el.selectionEnd ?? start;
                      const before = el.value.slice(0, start);
                      const after = el.value.slice(end);
                      const newVal = before + item.value + after;
                      onInsert(newVal);
                      requestAnimationFrame(() => {
                        el.focus();
                        const pos = start + item.value.length;
                        el.setSelectionRange(pos, pos);
                      });
                    } else {
                      onInsert(item.value);
                    }
                    setOpenExprHelperId(null);
                  }}
                  style={{
                    display: 'block', width: '100%', 'text-align': 'left',
                    background: 'none', border: 'none', padding: '4px 10px',
                    color: 'hsl(var(--foreground))', cursor: 'pointer',
                    'font-size': '0.85em', 'font-family': 'monospace',
                  }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.1)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'none')}
                >{item.label}<span style={{ color: 'hsl(var(--muted-foreground))', 'margin-left': '6px', 'font-size': '0.85em' }}>{item.value}</span></button>
              )}
            </For>
          </>)}
        </For>
        </PopoverContent>
      </Popover>
    );
  }


  function openStepConfigDialog(nodeId: string) {
    setStepConfigNodeId(nodeId);
    setShowStepConfigDialog(true);
  }

  // Resolve available outputs for a step based on its type
  function getStepOutputHints(node: DesignerNode | undefined): { fields: { name: string; description: string }[]; note?: string } {
    if (!node) return { fields: [], note: 'Select a node to see available outputs.' };

    if (node.type === 'trigger') return { fields: [{ name: 'trigger.*', description: 'Trigger input data' }] };

    const sub = node.subtype;
    switch (sub) {
      case 'call_tool': {
        const tool_id = node.config?.tool_id;
        if (!tool_id) return { fields: [], note: 'Select a tool to see its output schema.' };
        const tool = (props.toolDefinitions ?? []).find(t => t.id === tool_id);
        if (!tool) return { fields: [{ name: 'result', description: 'Tool return value' }], note: `Tool "${tool_id}" not found in registry.` };
        const schema = tool.output_schema as any;
        if (!schema || !schema.properties) return { fields: [{ name: 'result', description: `Output of ${tool.name}` }], note: tool.output_schema ? undefined : 'This tool has no typed output schema.' };
        const fields = Object.entries(schema.properties).map(([k, v]: [string, any]) => ({
          name: `result.${k}`,
          description: v.description || v.type || '',
        }));
        return { fields };
      }
      case 'invoke_agent':
        return { fields: [
          { name: 'result', description: 'Agent response text' },
          { name: 'agent_id', description: 'ID of the spawned agent' },
          { name: 'status', description: '"completed" (sync) or "spawned" (async)' },
        ] };
      case 'invoke_prompt':
        return { fields: [
          { name: 'result', description: 'Agent response text (new agent mode)' },
          { name: 'agent_id', description: 'ID of the spawned agent (new agent mode)' },
          { name: 'status', description: '"completed" or "spawned" (new agent mode)' },
          { name: 'delivered', description: 'Whether message was sent (existing agent mode)' },
        ] };
      case 'feedback_gate':
        return { fields: [
          { name: 'selected', description: 'The choice selected by the user (or freeform text if no choices)' },
          { name: 'text', description: 'Freeform text entered by the user' },
        ] };
      case 'signal_agent':
        return { fields: [], note: 'This step produces no outputs.' };
      case 'launch_workflow':
        return { fields: [{ name: 'result.child_workflow_id', description: 'ID of the launched workflow instance' }] };
      case 'delay':
        return { fields: [], note: 'This step produces no outputs.' };
      case 'event_gate':
        return { fields: [{ name: 'result', description: 'Event payload data' }] };
      case 'set_variable':
        return { fields: [], note: 'This step sets workflow variables directly — no step outputs.' };
      case 'schedule_task':
        return { fields: [{ name: 'result.task_id', description: 'ID of the scheduled task' }] };
      case 'branch':
        return { fields: [{ name: 'result.branch_targets', description: 'Array of selected branch step IDs' }] };
      default:
        return { fields: [{ name: 'result', description: 'Step output value' }] };
    }
  }


  function addNestedProperty(varIdx: number): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx) return v;
      const props = v.properties ? [...v.properties] : [];
      props.push({
        name: `prop_${props.length + 1}`,
        varType: 'string' as const,
        description: '',
        required: false,
        defaultValue: '',
        enumValues: [],
      });
      return { ...v, properties: props };
    }));
  }

  function updateNestedProperty(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.properties) return v;
      const props = v.properties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
      return { ...v, properties: props };
    }));
  }

  function removeNestedProperty(varIdx: number, propIdx: number): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.properties) return v;
      return { ...v, properties: v.properties.filter((_, pi) => pi !== propIdx) };
    }));
  }

  function addItemProperty(varIdx: number): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx) return v;
      const props = v.itemProperties ? [...v.itemProperties] : [];
      props.push({
        name: `prop_${props.length + 1}`,
        varType: 'string' as const,
        description: '',
        required: false,
        defaultValue: '',
        enumValues: [],
      });
      return { ...v, itemProperties: props };
    }));
  }

  function updateItemProperty(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.itemProperties) return v;
      const props = v.itemProperties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
      return { ...v, itemProperties: props };
    }));
  }

  function removeItemProperty(varIdx: number, propIdx: number): void {
    setVariables(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.itemProperties) return v;
      return { ...v, itemProperties: v.itemProperties.filter((_, pi) => pi !== propIdx) };
    }));
  }

  function renderVariableDialog() {
    const ro = props.readOnly;

    const [localVars, setLocalVars] = createSignal<WorkflowVariable[]>(
      JSON.parse(JSON.stringify(variables()))
    );

    function handleOk() {
      try {
        setVariables(localVars());
        pushHistory();
      } catch (err) {
        console.error('[WorkflowDesigner] handleOk variables error:', err);
      }
      setShowVarDialog(false);
    }

    function handleCancel() {
      setShowVarDialog(false);
    }

    function localAddVariable() {
      setLocalVars(prev => [...prev, {
        name: `var_${prev.length + 1}`,
        varType: 'string' as const,
        description: '',
        required: false,
        defaultValue: '',
        enumValues: [],
      }]);
    }

    function localRemoveVariable(idx: number) {
      setLocalVars(prev => prev.filter((_, i) => i !== idx));
    }

    function localUpdateVariable(idx: number, field: keyof WorkflowVariable, value: any) {
      setLocalVars(prev => prev.map((v, i) => i === idx ? { ...v, [field]: value } : v));
    }

    function localAddNestedProperty(varIdx: number) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx) return v;
        const props = v.properties ? [...v.properties] : [];
        props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
        return { ...v, properties: props };
      }));
    }

    function localUpdateNestedProperty(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx || !v.properties) return v;
        const props = v.properties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
        return { ...v, properties: props };
      }));
    }

    function localRemoveNestedProperty(varIdx: number, propIdx: number) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx || !v.properties) return v;
        return { ...v, properties: v.properties.filter((_, pi) => pi !== propIdx) };
      }));
    }

    function localAddItemProperty(varIdx: number) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx) return v;
        const props = v.itemProperties ? [...v.itemProperties] : [];
        props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
        return { ...v, itemProperties: props };
      }));
    }

    function localUpdateItemProperty(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx || !v.itemProperties) return v;
        const props = v.itemProperties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
        return { ...v, itemProperties: props };
      }));
    }

    function localRemoveItemProperty(varIdx: number, propIdx: number) {
      setLocalVars(prev => prev.map((v, i) => {
        if (i !== varIdx || !v.itemProperties) return v;
        return { ...v, itemProperties: v.itemProperties.filter((_, pi) => pi !== propIdx) };
      }));
    }

    return (
        <div>
          {/* Title bar */}
          <DialogHeader>
            <DialogTitle>Workflow Variables</DialogTitle>
          </DialogHeader>

          {/* Variable list */}
          <div style={{ display: 'flex', 'flex-direction': 'column', gap: '12px' }}>
            <For each={localVars()}>
              {(v, idx) => (
                <div class="wf-var-card">
                  {/* Header row */}
                  <div class="wf-var-card-header">
                    <input
                      class="wf-launch-input"
                      value={v.name}
                      onInput={(e) => localUpdateVariable(idx(), 'name', e.currentTarget.value)}
                      placeholder="Variable name"
                      disabled={ro}
                    />
                    <Show when={!ro}>
                      <button
                        onClick={() => localRemoveVariable(idx())}
                        style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '1em', padding: '0 4px' }}
                        title="Delete variable"
                      >✕</button>
                    </Show>
                  </div>

                  {/* Form fields */}
                  <div class="wf-var-grid">
                    <div class="wf-var-field">
                      <label>Type</label>
                      <select
                        class="wf-launch-input"
                        value={v.varType}
                        onChange={(e) => localUpdateVariable(idx(), 'varType', e.currentTarget.value)}
                        disabled={ro}
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
                          checked={v.required}
                          onChange={(e) => localUpdateVariable(idx(), 'required', e.currentTarget.checked)}
                          disabled={ro}
                        />
                        Required
                      </label>
                    </div>
                    <div class="wf-var-field full-width">
                      <label>Description</label>
                      <textarea
                        class="wf-launch-input"
                        value={v.description}
                        onInput={(e) => localUpdateVariable(idx(), 'description', e.currentTarget.value)}
                        placeholder="Variable description"
                        disabled={ro}
                        rows={1}
                        style={{ resize: 'vertical' }}
                      />
                    </div>
                    <div class="wf-var-field full-width">
                      <label>Default value</label>
                      {v.varType === 'boolean' ? (
                        <label style={{ display: 'flex', 'align-items': 'center', gap: '6px', cursor: 'pointer', 'font-size': '0.9em', padding: '4px 0' }}>
                          <input
                            type="checkbox"
                            checked={v.defaultValue === 'true'}
                            onChange={(e) => localUpdateVariable(idx(), 'defaultValue', e.currentTarget.checked ? 'true' : 'false')}
                            disabled={ro}
                          />
                          {v.defaultValue === 'true' ? 'true' : 'false'}
                        </label>
                      ) : v.varType === 'number' ? (
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v.defaultValue}
                          onInput={(e) => localUpdateVariable(idx(), 'defaultValue', e.currentTarget.value)}
                          placeholder="0"
                          disabled={ro}
                        />
                      ) : (
                        <input
                          class="wf-launch-input"
                          type="text"
                          value={v.defaultValue}
                          onInput={(e) => localUpdateVariable(idx(), 'defaultValue', e.currentTarget.value)}
                          placeholder="default"
                          disabled={ro}
                        />
                      )}
                    </div>
                  </div>

                  {/* String constraints */}
                  <Show when={v.varType === 'string'}>
                    <div class="wf-var-field">
                      <label>Allowed values</label>
                      <EnumEditor values={v.enumValues} onUpdate={(vals) => localUpdateVariable(idx(), 'enumValues', vals)} disabled={ro} />
                    </div>
                    <div class="wf-var-grid">
                      <div class="wf-var-field">
                        <label>Min length</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v.minLength ?? ''}
                          onInput={(e) => localUpdateVariable(idx(), 'minLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)}
                          placeholder="—"
                          disabled={ro}
                        />
                      </div>
                      <div class="wf-var-field">
                        <label>Max length</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v.maxLength ?? ''}
                          onInput={(e) => localUpdateVariable(idx(), 'maxLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)}
                          placeholder="—"
                          disabled={ro}
                        />
                      </div>
                    </div>
                    <div class="wf-var-field">
                      <label>Pattern (regex)</label>
                      <input
                        class="wf-launch-input"
                        type="text"
                        value={v.pattern ?? ''}
                        onInput={(e) => localUpdateVariable(idx(), 'pattern', e.currentTarget.value || undefined)}
                        placeholder="^[a-z]+$"
                        disabled={ro}
                      />
                    </div>
                  </Show>

                  {/* Number constraints */}
                  <Show when={v.varType === 'number'}>
                    <div class="wf-var-field">
                      <label>Allowed values</label>
                      <EnumEditor values={v.enumValues} onUpdate={(vals) => localUpdateVariable(idx(), 'enumValues', vals)} disabled={ro} />
                    </div>
                    <div class="wf-var-grid">
                      <div class="wf-var-field">
                        <label>Minimum</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v.minimum ?? ''}
                          onInput={(e) => localUpdateVariable(idx(), 'minimum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)}
                          placeholder="—"
                          disabled={ro}
                        />
                      </div>
                      <div class="wf-var-field">
                        <label>Maximum</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v.maximum ?? ''}
                          onInput={(e) => localUpdateVariable(idx(), 'maximum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)}
                          placeholder="—"
                          disabled={ro}
                        />
                      </div>
                    </div>
                  </Show>

                  {/* Array items type */}
                  <Show when={v.varType === 'array'}>
                    <div class="wf-var-field">
                      <label>Item type</label>
                      <select
                        class="wf-launch-input"
                        value={v.itemsType ?? 'string'}
                        onChange={(e) => localUpdateVariable(idx(), 'itemsType', e.currentTarget.value)}
                        disabled={ro}
                      >
                        <For each={TYPE_VALUES.filter(t => t !== 'array')}>
                          {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                        </For>
                      </select>
                    </div>
                    {/* Array item properties (when item type is Object) */}
                    <Show when={v.itemsType === 'object'}>
                      <div class="wf-var-section-label">
                        Item Properties
                      </div>
                      <div style={{ display: 'flex', 'flex-direction': 'column', gap: '8px' }}>
                        <For each={v.itemProperties ?? []}>
                          {(p, pIdx) => (
                            <div class="wf-nested-prop">
                              <div class="wf-nested-prop-header">
                                <input
                                  class="wf-launch-input"
                                  value={p.name}
                                  onInput={(e) => localUpdateItemProperty(idx(), pIdx(), 'name', e.currentTarget.value)}
                                  placeholder="Property name"
                                  disabled={ro}
                                />
                                <select
                                  class="wf-launch-input"
                                  style={{ width: '100px', flex: 'none' }}
                                  value={p.varType}
                                  onChange={(e) => localUpdateItemProperty(idx(), pIdx(), 'varType', e.currentTarget.value)}
                                  disabled={ro}
                                >
                                  <option value="string">{TYPE_LABELS['string']}</option>
                                  <option value="number">{TYPE_LABELS['number']}</option>
                                  <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                </select>
                                <Show when={!ro}>
                                  <button
                                    onClick={() => localRemoveItemProperty(idx(), pIdx())}
                                    style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.9em', padding: '0 4px' }}
                                    title="Remove property"
                                  >✕</button>
                                </Show>
                              </div>
                              <div class="wf-var-grid">
                                <div class="wf-var-field full-width">
                                  <label>Description</label>
                                  <input
                                    class="wf-launch-input"
                                    value={p.description}
                                    onInput={(e) => localUpdateItemProperty(idx(), pIdx(), 'description', e.currentTarget.value)}
                                    placeholder="Property description"
                                    disabled={ro}
                                  />
                                </div>
                                <div class="wf-var-field full-width">
                                  <label>Default value</label>
                                  <input
                                    class="wf-launch-input"
                                    value={p.defaultValue}
                                    onInput={(e) => localUpdateItemProperty(idx(), pIdx(), 'defaultValue', e.currentTarget.value)}
                                    placeholder="default"
                                    disabled={ro}
                                  />
                                </div>
                              </div>
                              <Show when={p.varType === 'string' || p.varType === 'number'}>
                                <div class="wf-var-field">
                                  <label>Allowed values</label>
                                  <EnumEditor values={p.enumValues} onUpdate={(vals) => localUpdateItemProperty(idx(), pIdx(), 'enumValues', vals)} disabled={ro} />
                                </div>
                              </Show>
                            </div>
                          )}
                        </For>
                        <Show when={!ro}>
                          <button
                            class="wf-btn-secondary"
                            style="align-self:flex-start;padding:4px 12px;font-size:0.82em;"
                            onClick={() => localAddItemProperty(idx())}
                          >+ Add property</button>
                        </Show>
                      </div>
                    </Show>
                  </Show>

                  {/* Object nested properties */}
                  <Show when={v.varType === 'object'}>
                    <div class="wf-var-section-label">
                      Properties
                    </div>
                    <div style={{ display: 'flex', 'flex-direction': 'column', gap: '8px' }}>
                      <For each={v.properties ?? []}>
                        {(p, pIdx) => (
                          <div class="wf-nested-prop">
                            <div class="wf-nested-prop-header">
                              <input
                                class="wf-launch-input"
                                value={p.name}
                                onInput={(e) => localUpdateNestedProperty(idx(), pIdx(), 'name', e.currentTarget.value)}
                                placeholder="Property name"
                                disabled={ro}
                              />
                              <select
                                class="wf-launch-input"
                                style={{ width: '100px', flex: 'none' }}
                                value={p.varType}
                                onChange={(e) => localUpdateNestedProperty(idx(), pIdx(), 'varType', e.currentTarget.value)}
                                disabled={ro}
                              >
                                <option value="string">{TYPE_LABELS['string']}</option>
                                <option value="number">{TYPE_LABELS['number']}</option>
                                <option value="boolean">{TYPE_LABELS['boolean']}</option>
                              </select>
                              <Show when={!ro}>
                                <button
                                  onClick={() => localRemoveNestedProperty(idx(), pIdx())}
                                  style={{ background: 'none', border: 'none', color: 'hsl(var(--muted-foreground))', cursor: 'pointer', 'font-size': '0.9em', padding: '0 4px' }}
                                  title="Remove property"
                                >✕</button>
                              </Show>
                            </div>
                            <div class="wf-var-grid">
                              <div class="wf-var-field full-width">
                                <label>Description</label>
                                <input
                                  class="wf-launch-input"
                                  value={p.description}
                                  onInput={(e) => localUpdateNestedProperty(idx(), pIdx(), 'description', e.currentTarget.value)}
                                  placeholder="Property description"
                                  disabled={ro}
                                />
                              </div>
                              <div class="wf-var-field full-width">
                                <label>Default value</label>
                                <input
                                  class="wf-launch-input"
                                  value={p.defaultValue}
                                  onInput={(e) => localUpdateNestedProperty(idx(), pIdx(), 'defaultValue', e.currentTarget.value)}
                                  placeholder="default"
                                  disabled={ro}
                                />
                              </div>
                            </div>
                            <Show when={p.varType === 'string' || p.varType === 'number'}>
                              <div class="wf-var-field">
                                <label>Allowed values</label>
                                <EnumEditor values={p.enumValues} onUpdate={(vals) => localUpdateNestedProperty(idx(), pIdx(), 'enumValues', vals)} disabled={ro} />
                              </div>
                            </Show>
                          </div>
                        )}
                      </For>
                      <Show when={!ro}>
                        <button
                          class="wf-btn-secondary"
                          style="align-self:flex-start;padding:4px 12px;font-size:0.82em;"
                          onClick={() => localAddNestedProperty(idx())}
                        >+ Add property</button>
                      </Show>
                    </div>
                  </Show>
                </div>
              )}
            </For>
          </div>

          {/* Add variable button */}
          <Show when={!ro}>
            <button
              class="wf-btn-secondary"
              style="align-self:flex-start;padding:6px 14px;font-size:0.85em;"
              onClick={localAddVariable}
            >+ Add variable</button>
          </Show>

          <DialogFooter>
            <Button variant="outline" onClick={handleCancel}>Cancel</Button>
            <Button onClick={handleOk}>OK</Button>
          </DialogFooter>
        </div>
    );
  }

  function renderStepConfigDialog() {
    const nodeId = stepConfigNodeId();
    if (!nodeId) return null;
    const node = nodeMap().get(nodeId);
    if (!node) return null;
    const ro = props.readOnly;

    const [localConfig, setLocalConfig] = createSignal<Record<string, any>>(
      JSON.parse(JSON.stringify(node.config))
    );

    const getNode = () => nodeMap().get(nodeId) ?? node;
    const getErrors = () => validationErrors()[nodeId] ?? [];

    function localUpdateCfg(key: string, value: any) {
      setLocalConfig(prev => ({ ...prev, [key]: value }));
    }

    function localPushHistory() {}

    function handleOk() {
      try {
        setNodes(prev => prev.map(n =>
          n.id === nodeId ? { ...n, config: localConfig() } : n
        ));
        pushHistory();
      } catch (err) {
        console.error('[WorkflowDesigner] handleOk step config error:', err);
      }
      setShowStepConfigDialog(false);
      setStepConfigNodeId(null);
    }

    function handleCancel() {
      setShowStepConfigDialog(false);
      setStepConfigNodeId(null);
    }

    return (
        <div>
          <DialogHeader class="flex flex-row items-center gap-2 mb-3">
            <SubtypeIcon subtype={node.subtype} size={18} />
            <DialogTitle>Edit: {nodeId}</DialogTitle>
          </DialogHeader>

          <Show when={getErrors().length > 0}>
            {(_) => (
              <div style={{ 'margin-bottom': '8px', background: 'hsl(38 92% 50% / 0.1)', border: '1px solid hsl(38 92% 50% / 0.3)', 'border-radius': '4px', padding: '4px 8px', 'font-size': '0.82em', color: 'hsl(38 92% 50%)' }}>
                <AlertTriangle size={14} /> Missing: {getErrors().join(', ')}
              </div>
            )}
          </Show>

          {renderConfigFieldsReactive(nodeId, getNode, () => localConfig(), getErrors, localUpdateCfg, localPushHistory)}

          <DialogFooter>
            <Button variant="outline" onClick={handleCancel}>Cancel</Button>
            <Button onClick={handleOk}>OK</Button>
          </DialogFooter>
        </div>
    );
  }

  // ── Render: Node Editor (right panel) — delegated to NodeEditorPanel ──
  function renderNodeEditorContent(
    nodeId: string,
    getNode: () => DesignerNode,
    getCfg: () => Record<string, any>,
    getErrors: () => string[],
    getConnEdges: () => { incoming: DesignerEdge[]; outgoing: DesignerEdge[] },
    getStepState: () => { status: string; error?: string | null } | undefined,
  ) {
    return (
      <NodeEditorPanel
        nodeId={nodeId}
        getNode={getNode}
        getCfg={getCfg}
        getErrors={getErrors}
        getConnEdges={getConnEdges}
        getStepState={getStepState}
        readOnly={props.readOnly}
        channels={props.channels}
        onRenameNode={renameNode}
        onUpdateNode={updateNode}
        onDeleteNode={deleteNode}
        onRemoveEdge={removeEdge}
        onPushHistory={pushHistory}
        onOpenStepConfig={openStepConfigDialog}
      />
    );
  }

  // ── Render: Config Fields — delegated to StepConfigFields ──
  function renderConfigFieldsReactive(
    nodeId: string,
    getNode: () => DesignerNode,
    getCfg: () => Record<string, any>,
    getErrors: () => string[],
    updateCfgFn?: (key: string, value: any) => void,
    pushHistoryFn?: () => void,
  ) {
    return (
      <StepConfigFields
        nodeId={nodeId}
        getNode={getNode}
        getCfg={getCfg}
        getErrors={getErrors}
        onUpdateConfig={updateCfgFn ?? ((key: string, value: any) => updateNodeConfig(nodeId, key, value))}
        onPushHistory={pushHistoryFn ?? pushHistory}
        readOnly={props.readOnly}
        toolDefinitions={props.toolDefinitions}
        personas={props.personas}
        channels={props.channels}
        eventTopics={props.eventTopics}
        variables={variables}
        wfAttachments={wfAttachments}
        renderExpressionHelper={renderExpressionHelper}
      />
    );
  }


  // ── Main render ──
  return (<ErrorBoundary fallback={(err, reset) => (
    <div style={{
      display: 'flex', 'flex-direction': 'column', 'align-items': 'center', 'justify-content': 'center',
      width: '100%', height: '100%', background: 'hsl(var(--background))', color: 'hsl(var(--foreground))',
      gap: '16px', padding: '32px',
    }}>
      <h3 style="margin:0;color:hsl(var(--destructive));"><AlertTriangle size={14} /> Workflow Designer Error</h3>
      <pre style="max-width:600px;overflow:auto;background:hsl(var(--card));padding:12px;border-radius:8px;font-size:0.85em;color:hsl(40 90% 84%);">
        {String(err)}
      </pre>
      <button
        style="background:hsl(var(--primary));color:hsl(var(--background));border:none;border-radius:6px;padding:8px 20px;cursor:pointer;font-weight:600;"
        onClick={reset}
      >Retry</button>
    </div>
  )}>
    <div ref={containerRef!} style={{
      display: 'flex', 'flex-direction': 'column',
      width: '100%', height: '100%',
      background: 'hsl(var(--background))',
      color: 'hsl(var(--foreground))',
      overflow: 'clip', 'font-family': 'inherit',
    }}>
      {/* ── Unified Toolbar ── */}
      <div class="wf-toolbar-wrapper">
        {/* Row 1: Close + Name + Action buttons */}
        <div class="wf-toolbar">
          {/* Close button */}
          <Show when={props.onClose}>
            <button class="wf-toolbar-close" onClick={() => props.onClose?.()} title="Close designer">
              <ArrowLeft size={16} />
            </button>
            <div class="wf-toolbar-sep" />
          </Show>

          {/* Workflow name */}
          <input
            style={{ ...inputStyle, width: '200px', flex: '1', 'min-width': '120px', 'font-weight': '600', 'font-size': '0.95em' }}
            value={wfName()} onInput={(e) => setWfName(e.currentTarget.value)}
            onBlur={() => pushHistory()} placeholder="Workflow name"
            disabled={props.readOnly} title="Workflow name"
          />

          {/* Action buttons */}
          <div class="wf-toolbar-actions">
            {/* History group */}
            <Show when={!props.readOnly}>
              <div class="wf-toolbar-group">
                <button class="wf-toolbar-btn" onClick={undo} title="Undo (Ctrl+Z)">
                  <RotateCcw size={14} />
                </button>
                <button class="wf-toolbar-btn" onClick={redo} title="Redo (Ctrl+Y)">
                  <RotateCw size={14} />
                </button>
              </div>
              <div class="wf-toolbar-sep" />
            </Show>

            {/* Canvas group */}
            <div class="wf-toolbar-group">
              <Show when={!props.readOnly}>
                <button class="wf-toolbar-btn" onClick={applyAutoLayout} title="Auto-layout (Ctrl+Shift+L)">
                  <LayoutGrid size={14} />
                </button>
              </Show>
              <button class="wf-toolbar-btn" onClick={fitToView} title="Fit to view">
                <Maximize2 size={14} />
              </button>
              <button
                class={`wf-toolbar-btn${snapEnabled() ? ' active' : ''}`}
                onClick={() => setSnapEnabled(!snapEnabled())}
                title="Toggle grid snap"
              >
                <Grid3x3 size={14} />
              </button>
            </div>
            <div class="wf-toolbar-sep" />

            {/* Panels group */}
            <div class="wf-toolbar-group">
              <button
                class={`wf-toolbar-btn${showYaml() ? ' active' : ''}`}
                onClick={() => setShowYaml(!showYaml())}
                title="Toggle YAML preview"
              >
                <Code2 size={14} />
              </button>
              <button
                class={`wf-toolbar-btn${showAttachments() ? ' active' : ''}`}
                onClick={() => setShowAttachments(!showAttachments())}
                title="Manage file attachments"
              >
                <Paperclip size={14} />
                <Show when={wfAttachments().length > 0}>
                  <span class="wf-toolbar-badge">{wfAttachments().length}</span>
                </Show>
              </button>
              <Show when={!props.readOnly && props.onAiAssist}>
                <button
                  class={`wf-toolbar-btn${aiAssistOpen() ? ' active-ai' : ''}`}
                  onClick={() => setAiAssistOpen(!aiAssistOpen())}
                  title="Toggle AI Assist"
                >
                  <Sparkles size={14} />
                </button>
              </Show>
            </div>

            {/* Save */}
            <Show when={props.onSave}>
              <div class="wf-toolbar-sep" />
              <button
                class="wf-toolbar-save"
                onClick={triggerSave}
                disabled={props.saving}
                title="Save (Ctrl+S)"
              >
                <Save size={14} />
                <span class="wf-toolbar-label">{props.saving ? 'Saving…' : 'Save'}</span>
                <Show when={isDirty() && !props.saving}>
                  <span class="wf-toolbar-dirty-dot" />
                </Show>
              </button>
            </Show>
          </div>
        </div>

        {/* Row 2: Version, Mode, Description button */}
        <div class="wf-toolbar-row2">
          <span class="wf-toolbar-row2-label">v</span>
          <span
            style={{ ...inputStyle, width: '50px', 'max-width': '50px', flex: '0 0 50px', display: 'inline-flex', 'align-items': 'center', opacity: '0.7', cursor: 'default' }}
            title="Version (auto-incremented on save)"
          >{wfVersion()}</span>
          <div class="wf-toolbar-sep" />
          <span class="wf-toolbar-row2-label">Mode</span>
          <select
            style={{ ...inputStyle, width: '110px', 'max-width': '110px', flex: '0 0 110px', cursor: 'pointer' }}
            value={wfMode()}
            onChange={(e) => { setWfMode(e.currentTarget.value); pushHistory(); }}
            disabled={props.readOnly || props.lockMode}
            title={props.lockMode ? "Mode cannot be changed after creation" : "Workflow mode — Background runs independently; Chat attaches to a chat session"}
          >
            <option value="background">Background</option>
            <option value="chat">Chat</option>
          </select>
          <div class="wf-toolbar-sep" />
          <button
            class={`wf-toolbar-btn${showDescPopup() ? ' active' : ''}`}
            onClick={() => setShowDescPopup(!showDescPopup())}
            title="Edit description"
            style={{ gap: '5px' }}
          >
            <AlignLeft size={14} />
            <span style={{ 'max-width': '200px', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap', 'font-size': '0.85em' }}>
              {wfDescription() || 'Add description…'}
            </span>
          </button>
        </div>
      </div>

      {/* Description popup */}
      <Dialog
        open={showDescPopup()}
        onOpenChange={(open: boolean) => { if (!open) setShowDescPopup(false); }}
      >
        <DialogContent class="max-w-lg">
          <DialogHeader>
            <DialogTitle>Description</DialogTitle>
          </DialogHeader>
        <textarea
          style={{
            ...inputStyle,
            width: '100%',
            'min-height': '100px',
            resize: 'vertical',
            'font-size': '0.85em',
            'line-height': '1.5',
          }}
          value={wfDescription()}
          onInput={(e) => setWfDescription(e.currentTarget.value)}
          onBlur={() => pushHistory()}
          placeholder="Describe what this workflow does…"
          disabled={props.readOnly}
        />
        </DialogContent>
      </Dialog>

      {/* ── Chat mode: result message template ── */}
      <Show when={wfMode() === 'chat'}>
        <div style={{
          display: 'flex', 'align-items': 'center', gap: '8px',
          padding: '4px 10px', background: 'hsl(var(--card))',
          'border-bottom': '1px solid hsl(var(--border))',
          'flex-shrink': '0',
        }}>
          <span style={{ 'font-size': '0.82em', color: 'hsl(var(--muted-foreground))', 'white-space': 'nowrap' }}>Result message</span>
          <input
            ref={resultMsgInputRef}
            style={{ ...inputStyle, flex: '1' }}
            value={wfResultMessage()}
            onInput={(e) => setWfResultMessage(e.currentTarget.value)}
            onBlur={() => pushHistory()}
            placeholder="e.g. {{steps.final.outputs.summary}} — template shown to user on completion"
            disabled={props.readOnly}
            title="Handlebars template resolved when the workflow completes. Use {{steps.<id>.outputs.<key>}} or {{variables.<name>}}."
          />
          <Show when={!props.readOnly}>
            {renderExpressionHelper((newVal) => {
              setWfResultMessage(newVal);
              pushHistory();
            }, () => resultMsgInputRef)}
          </Show>
        </div>
      </Show>

      {/* ── Main body ── */}
      <div style={{ display: 'flex', flex: '1', overflow: 'hidden' }}>
        {/* Left Panel */}
        <div style="width:180px;min-width:180px;max-width:180px;background:hsl(var(--card));border-right:1px solid hsl(var(--border));display:flex;flex-direction:column;overflow:hidden;">
          <div style={{ ...sectionHeaderStyle, 'font-size': '0.8em', 'letter-spacing': '0' }}>
            Step Palette
          </div>
          {renderPalette()}
          {/* Variables section at bottom of left panel */}
          <div style={{ 'border-top': '1px solid hsl(var(--border))', padding: '8px 10px' }}>
            <button
              class="wf-vars-btn"
              onClick={() => setShowVarDialog(true)}
            >
              <ClipboardList size={14} /> Variables ({variables().length})
            </button>
          </div>
        </div>

        {/* Center: Canvas + AI Assist */}
        <div style={{ flex: '1', display: 'flex', 'flex-direction': 'column', overflow: 'hidden' }}>
          {/* Canvas */}
          <div style={{ flex: '1', position: 'relative', overflow: 'hidden' }}>
            <div ref={canvasContainerRef!} style={{ width: '100%', height: '100%' }} />

            {/* Toast */}
            <Show when={toast()}>
              {(t) => (
                <div style={{
                  position: 'absolute', top: '8px', left: '50%', transform: 'translateX(-50%)',
                  background: t().type === 'success' ? 'hsl(142 71% 45% / 0.15)' : 'hsl(var(--destructive) / 0.15)',
                  border: `1px solid ${t().type === 'success' ? 'hsl(142 71% 45%)' : 'hsl(var(--destructive))'}`,
                  color: t().type === 'success' ? 'hsl(142 71% 45%)' : 'hsl(var(--destructive))',
                  padding: '6px 16px', 'border-radius': '6px', 'font-size': '0.8em',
                  'pointer-events': 'none', 'z-index': '10',
                }}>
                  {t().type === 'success' ? '✓' : '✕'} {t().text}
                </div>
              )}
            </Show>

            {/* Attachments management panel */}
            <Show when={showAttachments()}>
              <AttachmentsPanel
                attachments={wfAttachments()}
                readOnly={props.readOnly}
                wfId={wfId()}
                wfVersion={wfVersion()}
                onClose={() => setShowAttachments(false)}
                onUpdateDescription={(idx, desc) => {
                  const updated = [...wfAttachments()];
                  updated[idx] = { ...updated[idx], description: desc };
                  setWfAttachments(updated);
                }}
                onDelete={async (attachmentId) => {
                  try {
                    await props.onDeleteAttachment?.(wfId(), wfVersion(), attachmentId);
                  } catch (e) { /* ignore cleanup errors */ }
                  setWfAttachments(prev => prev.filter(a => a.id !== attachmentId));
                  pushHistory();
                }}
                onUpload={async (filePath, description) => {
                  try {
                    const att = await props.onUploadAttachment?.(wfId(), wfVersion(), filePath, description);
                    if (att) {
                      setWfAttachments(prev => [...prev, att]);
                      pushHistory();
                    }
                    return att;
                  } catch (e: any) {
                    console.error('Failed to upload attachment:', e);
                    return undefined;
                  }
                }}
                onPushHistory={pushHistory}
              />
            </Show>

            {/* Palette drag ghost */}
            <Show when={paletteDrag() && paletteDragPos()}>
              <div style={{
                position: 'fixed',
                left: `${paletteDragPos()!.x - 40}px`, top: `${paletteDragPos()!.y - 20}px`,
                background: 'hsl(var(--card))',
                border: '2px dashed hsl(var(--primary))', 'border-radius': '8px',
                padding: '6px 12px', 'font-size': '0.8em',
                color: 'hsl(var(--foreground))',
                'pointer-events': 'none', opacity: '0.8', 'z-index': '1000',
                display: 'flex', 'align-items': 'center', gap: '4px',
              }}>
                <span><SubtypeIcon subtype={paletteDrag()!.subtype} /></span>
                <span>{paletteDrag()!.label}</span>
              </div>
            </Show>

            {/* Empty state */}
            <Show when={nodes().length === 0}>
              <div style={{
                position: 'absolute', top: '50%', left: '50%',
                transform: 'translate(-50%, -50%)',
                color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em',
                'text-align': 'center', 'pointer-events': 'none', 'user-select': 'none',
              }}>
                <div style={{ 'font-size': '2em', 'margin-bottom': '8px' }}><Wrench size={32} /></div>
                Drag a step from the palette or click to add
              </div>
            </Show>

            {/* Hidden node list for accessibility/testing */}
            <div data-testid="node-list" style={{ position: 'absolute', left: '-9999px', top: '-9999px', 'pointer-events': 'none' }}>
              <For each={nodes()}>
                {(node) => <div data-nodeid={node.id} data-x={node.x} data-y={node.y}>{node.id}</div>}
              </For>
            </div>
          </div>

          {/* AI Assist Panel */}
          <Show when={aiAssistOpen()}>
            <AiAssistPanel
              response={aiAssistResponse()}
              loading={aiAssistLoading()}
              agent_id={aiAssistAgentId()}
              panelHeight={aiAssistPanelHeight()}
              prompt={aiAssistPrompt()}
              pendingQuestion={aiPendingQuestion()}
              questionFreeform={aiQuestionFreeform()}
              questionSubmitting={aiQuestionSubmitting()}
              onPanelHeightChange={setAiAssistPanelHeight}
              onPromptChange={setAiAssistPrompt}
              onSend={handleAiAssist}
              onClose={() => { cleanupAiAssistAgent(); setAiAssistOpen(false); setAiAssistResponse(''); }}
              onNewConversation={() => { cleanupAiAssistAgent(); setAiAssistResponse(''); }}
              onQuestionFreeformChange={setAiQuestionFreeform}
              onAnswerQuestion={answerAiQuestion}
              responseRef={(el) => { aiAssistResponseEl = el; }}
            />
          </Show>
        </div>

        {/* YAML Preview Panel */}
        <Show when={showYaml()}>
          <YamlEditorPanel yamlOutput={yamlOutput()} sectionHeaderStyle={sectionHeaderStyle} />
        </Show>

        {/* Right Panel: Node Editor */}
        <div style="width:260px;min-width:260px;max-width:260px;background:hsl(var(--card));border-left:1px solid hsl(var(--border));display:flex;flex-direction:column;overflow:hidden;">
          <div style={sectionHeaderStyle}>
            {selectedNodeId() ? `Edit: ${selectedNodeId()}` : selectedNodes().size > 1 ? `${selectedNodes().size} selected` : 'Node Editor'}
          </div>
          <Show when={selectedNodeId()} keyed fallback={
            <div style={{ padding: '20px 12px', color: 'hsl(var(--muted-foreground))', 'font-size': '0.8em', 'text-align': 'center' }}>
              {selectedNodes().size > 1
                ? `${selectedNodes().size} nodes selected`
                : 'Select a node to edit'}
            </div>
          }>
            {(nodeId) => {
              const getNode = () => nodeMap().get(nodeId)!;
              const getCfg = () => getNode().config;
              const getErrors = () => validationErrors()[nodeId] ?? [];
              const getConnEdges = () => edgesForNode(nodeId);
              const getStepState = () => props.instanceStepStates?.[nodeId];
              return renderNodeEditorContent(nodeId, getNode, getCfg, getErrors, getConnEdges, getStepState);
            }}
          </Show>
        </div>
      </div>
    </div>

    <Dialog open={!!showVarDialog()} onOpenChange={(open: boolean) => { if (!open) setShowVarDialog(false); }}>
      <DialogContent class="max-w-2xl max-h-[80vh] overflow-y-auto">
        <Show when={showVarDialog()}>
          {(_) => renderVariableDialog()}
        </Show>
      </DialogContent>
    </Dialog>



    <Dialog open={!!showStepConfigDialog()} onOpenChange={(open: boolean) => { if (!open) { setShowStepConfigDialog(false); setStepConfigNodeId(null); } }}>
      <DialogContent class="max-w-2xl max-h-[80vh] overflow-y-auto">
        <Show when={showStepConfigDialog()}>
          {(_) => renderStepConfigDialog()}
        </Show>
      </DialogContent>
    </Dialog>
  </ErrorBoundary>);
};

export default WorkflowDesigner;
