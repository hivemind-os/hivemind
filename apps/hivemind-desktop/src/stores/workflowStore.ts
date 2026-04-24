import { createSignal, createMemo, untrack } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { WorkflowDefinitionSummary, WorkflowInstanceSummary, WorkflowInstance, WorkflowStatus, WorkflowMode, WorkflowImpactEstimate, InterceptedActionPage, ShadowSummary } from '../types';

/** Query parameters for the workflow_list_instances command. */
interface WorkflowListParams {
  limit: number;
  offset: number;
  status?: string;
  definition?: string;
  include_archived?: boolean;
  [key: string]: unknown;
}

/** Payload for the 'workflow:event' Tauri event. */
interface WorkflowEventPayload {
  topic?: string;
  payload?: WorkflowStepPayload;
}

/** Inner payload carried by workflow SSE events. */
interface WorkflowStepPayload {
  instance_id?: number;
  waiting_type?: string;
}

/** Payload for the 'workflow:error' Tauri event. */
interface WorkflowErrorPayload {
  error?: string;
}

/** Result shape from workflow_check_definition_dependents. */
interface DefinitionDependents {
  triggers: unknown[];
  scheduled_tasks: unknown[];
}

/** Result shape from workflow_get_definition. */
interface WorkflowDefinitionResult {
  definition: Record<string, unknown>;
  yaml: string;
}

const ALL_STATUSES: WorkflowStatus[] = ['pending', 'running', 'paused', 'waiting_on_input', 'waiting_on_event', 'completed', 'failed', 'killed'];
const DEFAULT_CHECKED: Record<string, boolean> = {
  pending: true, running: true, paused: true,
  waiting_on_input: true, waiting_on_event: true,
  completed: false, failed: true, killed: false,
};

export function createWorkflowStore() {
  const [definitions, setDefinitions] = createSignal<WorkflowDefinitionSummary[]>([]);
  const [chatDefinitions, setChatDefinitions] = createSignal<WorkflowDefinitionSummary[]>([]);
  const [instances, setInstances] = createSignal<WorkflowInstanceSummary[]>([]);
  const [selectedInstance, setSelectedInstance] = createSignal<WorkflowInstance | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [statusFilter, setStatusFilter] = createSignal<Record<string, boolean>>({ ...DEFAULT_CHECKED });
  const [definitionFilter, setDefinitionFilter] = createSignal<Record<string, boolean>>({});
  const [yamlEditor, setYamlEditor] = createSignal('');
  const [showEditor, setShowEditor] = createSignal(false);
  const [showDesigner, setShowDesigner] = createSignal(false);
  const [designerYaml, setDesignerYaml] = createSignal('');
  const [viewDefinitions, setViewDefinitions] = createSignal(false);
  const [sidebarSelectedInstanceId, setSidebarSelectedInstanceId] = createSignal<number | null>(null);
  const [sidebarSearchQuery, setSidebarSearchQuery] = createSignal('');
  const [showArchived, setShowArchived] = createSignal(false);

  const [page, setPage]= createSignal(0);
  const [totalCount, setTotalCount] = createSignal(0);
  const pageSize = 20;
  const totalPages = createMemo(() => Math.max(1, Math.ceil(totalCount() / pageSize)));

  // Sequence counters to discard stale async responses
  let instancesSeq = 0;
  let designerSeq = 0;

  const activeCount = createMemo(() =>
    (instances() ?? []).filter(i => ['running', 'paused', 'waiting_on_input', 'waiting_on_event'].includes(i.status)).length
  );

  const waitingCount = createMemo(() =>
    (instances() ?? []).filter(i => i.status === 'waiting_on_input').length
  );

  const sidebarFilteredInstances = createMemo(() => {
    let items = instances() ?? [];
    const query = sidebarSearchQuery().toLowerCase().trim();
    if (query) {
      items = items.filter(i => i.definition_name.toLowerCase().includes(query) || String(i.id).includes(query));
    }
    // Use existing statusFilter for filtering
    const checkedStatuses = Object.entries(statusFilter()).filter(([_, v]) => v).map(([k]) => k);
    if (checkedStatuses.length > 0) {
      items = items.filter(i => checkedStatuses.includes(i.status));
    }
    return items;
  });

  function initDefinitionFilter(defs: WorkflowDefinitionSummary[]) {
    const current = untrack(definitionFilter);
    const updated: Record<string, boolean> = {};
    for (const d of defs) {
      updated[d.name] = current[d.name] ?? true;
    }
    // Only write if values actually changed to avoid re-triggering effects
    const currentKeys = Object.keys(current);
    if (currentKeys.length === defs.length && defs.every(d => current[d.name] === updated[d.name])) {
      return;
    }
    setDefinitionFilter(updated);
  }

  async function loadDefinitions() {
    try {
      const defs = await invoke<WorkflowDefinitionSummary[]>('workflow_list_definitions', {});
      setDefinitions(defs ?? []);
      initDefinitionFilter(defs ?? []);
    } catch (e) {
      // Silently ignore polling errors — don't interrupt the user
      console.warn('Failed to load definitions:', e);
    }
  }

  async function loadChatDefinitions() {
    try {
      const defs = await invoke<WorkflowDefinitionSummary[]>('workflow_list_definitions', { mode: 'chat' });
      setChatDefinitions(defs ?? []);
    } catch (e) {
      console.warn('Failed to load chat definitions:', e);
    }
  }

  async function loadInstances() {
    const seq = ++instancesSeq;
    try {
      // Use untrack to avoid creating reactive dependencies when called from effects
      const checkedStatuses = Object.entries(untrack(statusFilter))
        .filter(([_, v]) => v).map(([k]) => k);
      const checkedDefs = Object.entries(untrack(definitionFilter))
        .filter(([_, v]) => v).map(([k]) => k);

      const params: WorkflowListParams = {
        limit: pageSize,
        offset: untrack(page) * pageSize,
        include_archived: untrack(showArchived) || undefined,
      };
      if (checkedStatuses.length > 0 && checkedStatuses.length < ALL_STATUSES.length) {
        params.status = checkedStatuses.join(',');
      }
      if (checkedDefs.length > 0) {
        const allDefs = untrack(definitions).map(d => d.name);
        if (checkedDefs.length < allDefs.length) {
          params.definition = checkedDefs.join(',');
        }
      }
      const result = await invoke<{ items: WorkflowInstanceSummary[]; total: number }>('workflow_list_instances', params);
      if (seq !== instancesSeq) return; // discard stale response
      setInstances(result?.items ?? []);
      setTotalCount(result?.total ?? 0);
    } catch (e) {
      // Silently ignore polling errors — don't interrupt the user
      console.warn('Failed to load instances:', e);
    }
  }

  function toggleStatus(status: string) {
    setStatusFilter(prev => ({ ...prev, [status]: !prev[status] }));
    setPage(0);
    void loadInstances();
  }

  function toggleDefinition(name: string) {
    setDefinitionFilter(prev => ({ ...prev, [name]: !prev[name] }));
    setPage(0);
    void loadInstances();
  }

  function toggleShowArchived() {
    setShowArchived(prev => !prev);
    setPage(0);
    void loadInstances();
  }

  function nextPage() {
    if (page() < totalPages() - 1) { setPage(p => p + 1); void loadInstances(); }
  }

  function prevPage() {
    if (page() > 0) { setPage(p => p - 1); void loadInstances(); }
  }

  async function loadInstance(instanceId: number) {
    try {
      setLoading(true);
      const inst = await invoke<WorkflowInstance>('workflow_get_instance', { instance_id: instanceId });
      setSelectedInstance(inst);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to load instance');
    } finally {
      setLoading(false);
    }
  }

  async function saveDefinition(yaml: string): Promise<boolean> {
    try {
      const result = await invoke('workflow_save_definition', { yaml });
      console.log('[workflowStore] Save succeeded:', result);
      // Refresh definitions list in the background — don't block save result
      void loadDefinitions();
      setShowEditor(false);
      setYamlEditor('');
      return true;
    } catch (e: unknown) {
      console.error('[workflowStore] Save failed:', e);
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e)) ?? 'Failed to save definition';
      setError(msg);
      return false;
    }
  }

  async function deleteDefinition(name: string, version: string) {
    try {
      await invoke('workflow_delete_definition', { name, version });
      await loadDefinitions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to delete definition');
    }
  }

  async function resetDefinition(name: string) {
    try {
      await invoke('workflow_reset_definition', { name });
      await loadDefinitions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to reset definition');
    }
  }

  async function archiveDefinition(name: string, version: string, archived: boolean = true) {
    try {
      await invoke('workflow_archive_definition', { name, version, archived });
      await loadDefinitions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to archive definition');
    }
  }

  async function setTriggersPaused(name: string, version: string, paused: boolean = true) {
    try {
      await invoke('workflow_set_triggers_paused', { name, version, paused });
      await loadDefinitions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to toggle triggers');
    }
  }

  async function checkDefinitionDependents(name: string, version: string): Promise<DefinitionDependents | null> {
    try {
      return await invoke<DefinitionDependents>('workflow_check_definition_dependents', { name, version });
    } catch (e) {
      console.warn('Failed to check definition dependents:', e);
      return null;
    }
  }

  async function launchWorkflow(definition: string, inputs: Record<string, unknown> = {}, parentSessionId: string, triggerStepId?: string, executionMode?: string) {
    try {
      const result = await invoke<{ instance_id: number }>('workflow_launch', {
        definition,
        inputs,
        parent_session_id: parentSessionId,
        trigger_step_id: triggerStepId ?? null,
        execution_mode: executionMode ?? null,
      });
      await loadInstances();
      return result.instance_id;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to launch workflow');
      return null;
    }
  }

  async function launchChatWorkflow(
    definition: string,
    inputs: Record<string, unknown> = {},
    parentSessionId: string,
    workspace_path: string,
    permissions: unknown[] = [],
    triggerStepId?: string,
  ): Promise<number | null> {
    try {
      const result = await invoke<{ instance_id: number }>('workflow_launch', {
        definition,
        inputs,
        parent_session_id: parentSessionId,
        trigger_step_id: triggerStepId ?? null,
        workspace_path: workspace_path,
        permissions: permissions.length > 0 ? permissions : null,
      });
      return result.instance_id;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to launch chat workflow');
      return null;
    }
  }

  async function analyzeWorkflow(definitionName: string, version?: string): Promise<WorkflowImpactEstimate | null> {
    try {
      const result = await invoke<WorkflowImpactEstimate>('workflow_analyze', {
        definition_name: definitionName,
        version: version ?? null,
      });
      return result;
    } catch (e) {
      console.warn('Failed to analyze workflow:', e);
      return null;
    }
  }

  async function runTests(
    definitionName: string,
    version?: string,
    testNames?: string[],
  ): Promise<{ results: WorkflowTestResult[]; all_passed: boolean } | null> {
    try {
      return await invoke<{ results: WorkflowTestResult[]; all_passed: boolean }>('workflow_run_tests', {
        definition_name: definitionName,
        version: version ?? null,
        test_names: testNames ?? null,
      });
    } catch (e) {
      console.warn('Failed to run workflow tests:', e);
      return null;
    }
  }

  async function simulateTrigger(
    definitionName: string,
    triggerStepId: string,
    payload: Record<string, unknown>,
    version?: string,
    mode?: 'shadow' | 'normal',
  ): Promise<{ instance_id: number } | null> {
    try {
      const result = await invoke<{ instance_id: number }>('workflow_simulate_trigger', {
        definition_name: definitionName,
        trigger_step_id: triggerStepId,
        payload,
        version: version ?? null,
        mode: mode ?? null,
      });
      await loadInstances();
      return result;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to simulate trigger');
      return null;
    }
  }

  async function fetchInterceptedActions(
    instanceId: number,
    limit?: number,
    offset?: number,
  ): Promise<InterceptedActionPage | null> {
    try {
      return await invoke<InterceptedActionPage>('workflow_intercepted_actions', {
        instance_id: instanceId,
        limit: limit ?? 50,
        offset: offset ?? 0,
      });
    } catch (e) {
      console.warn('Failed to fetch intercepted actions:', e);
      return null;
    }
  }

  async function fetchShadowSummary(instanceId: number): Promise<ShadowSummary | null> {
    try {
      return await invoke<ShadowSummary>('workflow_shadow_summary', { instance_id: instanceId });
    } catch (e) {
      console.warn('Failed to fetch shadow summary:', e);
      return null;
    }
  }

  async function pauseInstance(instanceId: number) {
    try {
      await invoke('workflow_pause', { instance_id: instanceId });
      await loadInstances();
      if (selectedInstance()?.id === instanceId) await loadInstance(instanceId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to pause');
    }
  }

  async function resumeInstance(instanceId: number) {
    try {
      await invoke('workflow_resume', { instance_id: instanceId });
      await loadInstances();
      if (selectedInstance()?.id === instanceId) await loadInstance(instanceId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to resume');
    }
  }

  async function killInstance(instanceId: number) {
    try {
      await invoke('workflow_kill', { instance_id: instanceId });
      await loadInstances();
      if (selectedInstance()?.id === instanceId) await loadInstance(instanceId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to kill');
    }
  }

  async function archiveInstance(instanceId: number, archived: boolean = true) {
    try {
      await invoke('workflow_archive_instance', { instance_id: instanceId, archived });
      await loadInstances();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to archive instance');
    }
  }

  async function respondToGate(instanceId: number, stepId: string, response: Record<string, unknown>) {
    try {
      await invoke('workflow_respond_gate', { instance_id: instanceId, step_id: stepId, response });
      await loadInstance(instanceId);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to respond');
    }
  }

  async function getDefinitionYaml(name: string, version?: string): Promise<string | null> {
    try {
      const result = await invoke<WorkflowDefinitionResult>('workflow_get_definition', {
        name,
        version: version ?? null,
      });
      return result.yaml;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to load definition');
      return null;
    }
  }

  async function getDefinitionParsed(name: string, version?: string): Promise<WorkflowDefinitionResult | null> {
    try {
      return await invoke<WorkflowDefinitionResult>('workflow_get_definition', {
        name,
        version: version ?? null,
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e) || 'Failed to load definition');
      return null;
    }
  }

  async function openDesigner(name?: string, version?: string) {
    const seq = ++designerSeq;
    if (name) {
      const yaml = await getDefinitionYaml(name, version);
      if (seq !== designerSeq) return; // user opened a different definition
      setDesignerYaml(yaml ?? '');
    } else {
      setDesignerYaml('');
    }
    setShowDesigner(true);
  }

  async function saveFromDesigner(yaml: string) {
    console.log('[workflowStore] saveFromDesigner called, yaml length:', yaml.length);
    const ok = await saveDefinition(yaml);
    console.log('[workflowStore] saveFromDesigner result:', ok);
    if (ok) {
      setShowDesigner(false);
      setDesignerYaml('');
    }
  }

  async function copyDefinition(source_name: string, sourceVersion: string | undefined, newName: string): Promise<boolean> {
    try {
      await invoke('workflow_copy_definition', {
        source_name,
        source_version: sourceVersion ?? null,
        new_name: newName,
      });
      await loadDefinitions();
      return true;
    } catch (e: unknown) {
      setError(typeof e === 'string' ? e : (e instanceof Error ? e.message : 'Copy failed'));
      return false;
    }
  }

  async function refresh() {
    await Promise.all([loadDefinitions(), loadInstances()]);
  }

  /** Launch AI assist agent for workflow authoring. Returns agent_id. */
  async function aiAssist(yaml: string, prompt: string): Promise<string> {
    const result = await invoke<{ agent_id: string }>('workflow_ai_assist', { yaml, prompt });
    return result.agent_id;
  }

  // ── Attachment management ──

  interface WorkflowAttachment {
    id: string;
    filename: string;
    description: string;
    media_type?: string;
    size_bytes?: number;
  }

  async function uploadAttachment(
    workflowId: string,
    version: string,
    filePath: string,
    description: string,
  ): Promise<WorkflowAttachment> {
    return invoke<WorkflowAttachment>('workflow_upload_attachment', {
      workflow_id: workflowId,
      version,
      file_path: filePath,
      description,
    });
  }

  async function deleteAttachment(
    workflowId: string,
    version: string,
    attachmentId: string,
  ): Promise<void> {
    await invoke('workflow_delete_attachment', { workflow_id: workflowId, version, attachment_id: attachmentId });
  }

  async function copyAttachments(
    workflowId: string,
    fromVersion: string,
    toVersion: string,
  ): Promise<void> {
    await invoke('workflow_copy_attachments', { workflow_id: workflowId, from_version: fromVersion, to_version: toVersion });
  }

  // ── SSE subscription for real-time workflow events ──

  let eventUnlisten: UnlistenFn | null = null;
  let errorUnlisten: UnlistenFn | null = null;
  let sseConnected = false;
  let disposed = false;

  // Debounce helper: coalesce rapid-fire events into a single refresh
  let instanceDebounce: ReturnType<typeof setTimeout> | undefined;
  function debouncedLoadInstances() {
    if (instanceDebounce) clearTimeout(instanceDebounce);
    instanceDebounce = setTimeout(() => void loadInstances(), 500);
  }

  let defDebounce: ReturnType<typeof setTimeout> | undefined;
  function debouncedLoadDefinitions() {
    if (defDebounce) clearTimeout(defDebounce);
    defDebounce = setTimeout(() => void loadDefinitions(), 200);
  }

  /** Apply an SSE event directly to the in-memory instance list for instant
   *  badge/status updates — no API round-trip required. A debounced
   *  `loadInstances()` follows for full reconciliation. */
  function applyEventToInstances(topic: string, payload: WorkflowStepPayload | undefined) {
    const instanceId: number | undefined = payload?.instance_id;
    if (instanceId == null) return;

    setInstances(prev => {
      const idx = prev.findIndex(i => i.id === instanceId);
      if (idx < 0) return prev;
      const updated = [...prev];
      const inst = { ...updated[idx] };

      if (topic === 'workflow.interaction.requested') {
        inst.pending_agent_questions = (inst.pending_agent_questions ?? 0) + 1;
      } else if (topic === 'workflow.interaction.responded') {
        inst.pending_agent_questions = Math.max(0, (inst.pending_agent_questions ?? 0) - 1);
      } else if (topic === 'workflow.step.waiting') {
        const wt = payload?.waiting_type;
        if (wt === 'input' || wt === 'waiting_on_input') {
          inst.status = 'waiting_on_input';
        } else if (wt === 'event' || wt === 'waiting_on_event') {
          inst.status = 'waiting_on_event';
        }
      } else if (topic === 'workflow.event_gate.resolved') {
        if (inst.status === 'waiting_on_event') inst.status = 'running';
      } else if (topic === 'workflow.instance.started' || topic === 'workflow.instance.resumed') {
        inst.status = 'running';
      } else if (topic === 'workflow.instance.paused') {
        inst.status = 'paused';
      } else if (topic === 'workflow.instance.completed') {
        inst.status = 'completed';
        inst.pending_agent_questions = 0;
      } else if (topic === 'workflow.instance.failed') {
        inst.status = 'failed';
        inst.pending_agent_questions = 0;
      } else if (topic === 'workflow.instance.killed') {
        inst.status = 'killed';
        inst.pending_agent_questions = 0;
      } else if (topic === 'workflow.step.started') {
        if (inst.status === 'waiting_on_input' || inst.status === 'waiting_on_event') {
          inst.status = 'running';
        }
        inst.pending_agent_questions = 0;
      } else if (topic === 'workflow.step.completed') {
        inst.steps_completed = (inst.steps_completed ?? 0) + 1;
      }

      updated[idx] = inst;
      return updated;
    });
  }

  async function subscribeEvents() {
    if (sseConnected || disposed) return;

    try {
      await invoke('workflow_subscribe_events');
    } catch (e) {
      console.warn('[workflowStore] Failed to start workflow event stream:', e);
      return;
    }

    if (disposed) return;

    try {
      const [eventUl, errorUl] = await Promise.all([
        listen<WorkflowEventPayload>('workflow:event', (e) => {
          if (disposed) return;
          const topic: string = e.payload?.topic ?? '';
          const payload = e.payload?.payload;

          if (topic.startsWith('workflow.definition.')) {
            debouncedLoadDefinitions();
          } else if (topic.startsWith('workflow.instance.') || topic.startsWith('workflow.step.') || topic.startsWith('workflow.interaction.') || topic.startsWith('workflow.event_gate.')) {
            // Immediately apply event to in-memory state for instant badge updates
            applyEventToInstances(topic, payload);

            // Reconcile with backend (catches anything the optimistic update missed)
            debouncedLoadInstances();

            // If the event is for the currently expanded instance, refresh its detail too
            const instanceId = payload?.instance_id;
            const sel = selectedInstance();
            if (instanceId != null && sel && sel.id === instanceId) {
              void loadInstance(instanceId);
            }
          }
        }),
        listen<WorkflowErrorPayload>('workflow:error', (e) => {
          console.warn('[workflowStore] Workflow event stream error:', e.payload?.error);
        }),
      ]);

      if (disposed) {
        eventUl();
        errorUl();
        return;
      }

      eventUnlisten = eventUl;
      errorUnlisten = errorUl;
      sseConnected = true;
    } catch (e) {
      console.warn('[workflowStore] Failed to listen for workflow events:', e);
    }
  }

  function unsubscribeEvents() {
    disposed = true;
    if (eventUnlisten) { eventUnlisten(); eventUnlisten = null; }
    if (errorUnlisten) { errorUnlisten(); errorUnlisten = null; }
    if (instanceDebounce) clearTimeout(instanceDebounce);
    if (defDebounce) clearTimeout(defDebounce);
    sseConnected = false;
  }

  return {
    definitions, chatDefinitions, instances, selectedInstance, loading, error,
    statusFilter, definitionFilter, toggleStatus, toggleDefinition,
    page, totalCount, totalPages, nextPage, prevPage,
    activeCount, waitingCount,
    yamlEditor, setYamlEditor, showEditor, setShowEditor,
    showDesigner, setShowDesigner, designerYaml, setDesignerYaml,
    viewDefinitions, setViewDefinitions,
    loadDefinitions, loadChatDefinitions, loadInstances, loadInstance,
    saveDefinition, deleteDefinition, resetDefinition, archiveDefinition, setTriggersPaused, checkDefinitionDependents, getDefinitionYaml, getDefinitionParsed,
    openDesigner, saveFromDesigner, copyDefinition, aiAssist,
    uploadAttachment, deleteAttachment, copyAttachments,
    launchWorkflow, launchChatWorkflow, analyzeWorkflow, runTests, simulateTrigger,
    fetchInterceptedActions, fetchShadowSummary,
    pauseInstance, resumeInstance, killInstance, archiveInstance,
    respondToGate, refresh,
    subscribeEvents, unsubscribeEvents,
    setSelectedInstance, setError,
    sidebarSelectedInstanceId, setSidebarSelectedInstanceId,
    sidebarSearchQuery, setSidebarSearchQuery,
    sidebarFilteredInstances,
    showArchived, toggleShowArchived,
  };
}

export type WorkflowStore = ReturnType<typeof createWorkflowStore>;
