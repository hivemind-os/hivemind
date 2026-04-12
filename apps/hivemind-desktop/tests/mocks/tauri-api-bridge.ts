/**
 * Tauri API Bridge for integration tests.
 *
 * Replaces `tauri-mock.ts` with real HTTP calls to a hivemind daemon.
 * Every `invoke()` command is routed to the daemon's REST API, and
 * Tauri event subscriptions are backed by SSE streams.
 */

// ── Types ──────────────────────────────────────────────────────────────

type EventCallback = (event: { event: string; payload: unknown; id: number }) => void;

interface BridgeConfig {
  daemon_url: string;
  authToken: string;
}

// ── State ──────────────────────────────────────────────────────────────

let _config: BridgeConfig = { daemon_url: '', authToken: '' };
const _eventListeners = new Map<string, Set<EventCallback>>();
const _activeSources: EventSource[] = [];
let _callbackCounter = 0;
const _callbackMap = new Map<number, Function>();

// ── Helpers ────────────────────────────────────────────────────────────

function apiUrl(path: string): string {
  return `${_config.daemon_url}${path}`;
}

function authHeaders(): Record<string, string> {
  return { Authorization: `Bearer ${_config.authToken}`, 'Content-Type': 'application/json' };
}

async function apiGet<T = unknown>(path: string): Promise<T> {
  const resp = await fetch(apiUrl(path), { headers: authHeaders() });
  if (!resp.ok) throw new Error(`GET ${path}: ${resp.status} ${await resp.text()}`);
  return resp.json();
}

async function apiPost<T = unknown>(path: string, body?: unknown): Promise<T> {
  const resp = await fetch(apiUrl(path), {
    method: 'POST',
    headers: authHeaders(),
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!resp.ok) throw new Error(`POST ${path}: ${resp.status} ${await resp.text()}`);
  const text = await resp.text();
  return text ? JSON.parse(text) : ({} as T);
}

async function apiPut<T = unknown>(path: string, body?: unknown): Promise<T> {
  const resp = await fetch(apiUrl(path), {
    method: 'PUT',
    headers: authHeaders(),
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!resp.ok) throw new Error(`PUT ${path}: ${resp.status} ${await resp.text()}`);
  const text = await resp.text();
  return text ? JSON.parse(text) : ({} as T);
}

async function apiDelete<T = unknown>(path: string): Promise<T> {
  const resp = await fetch(apiUrl(path), { method: 'DELETE', headers: authHeaders() });
  if (!resp.ok) throw new Error(`DELETE ${path}: ${resp.status} ${await resp.text()}`);
  const text = await resp.text();
  return text ? JSON.parse(text) : ({} as T);
}

// ── Event dispatch ─────────────────────────────────────────────────────

function dispatchTauriEvent(eventName: string, payload: unknown) {
  const listeners = _eventListeners.get(eventName);
  if (listeners) {
    const envelope = { event: eventName, payload, id: Date.now() };
    for (const cb of listeners) {
      try { cb(envelope); } catch (e) { console.error(`[bridge] event callback error for ${eventName}:`, e); }
    }
  }
}

// ── SSE stream helpers ─────────────────────────────────────────────────

function connectSSE(path: string, eventName: string): EventSource {
  const url = apiUrl(path);
  const src = new EventSource(url);
  src.onmessage = (e) => {
    try {
      const data = JSON.parse(e.data);
      dispatchTauriEvent(eventName, data);
    } catch { /* ignore non-JSON */ }
  };
  src.onerror = () => { /* SSE will auto-reconnect */ };
  _activeSources.push(src);
  return src;
}

// ── Invoke command router ──────────────────────────────────────────────

async function bridgeInvoke(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
  switch (command) {
    // ── Status / config ──
    case 'status_heartbeat':
      return apiPost('/api/v1/status/heartbeat');
    case 'get_user_status':
      return apiGet('/api/v1/status');
    case 'daemon_status':
    case 'daemon_start':
      return { status: 'running' };
    case 'app_context':
      return {
        appDir: '/tmp/hivemind-test',
        hivemindHome: '/tmp/hivemind-test',
        platform: 'test',
        version: '0.0.0-test',
      };
    case 'config_show':
      // config_show reads local config file; return minimal YAML for tests
      return 'daemon:\n  log_level: info\n';
    case 'config_get': {
      const cfg = await apiGet('/api/v1/config/get') as Record<string, unknown>;
      // Force setup_completed so the app doesn't show the setup wizard in tests
      cfg.setup_completed = true;
      return cfg;
    }
    case 'config_save':
      return apiPut('/api/v1/config', args.config ?? args);

    // ── Chat / sessions ──
    case 'chat_create_session':
      return apiPost('/api/v1/chat/sessions', {
        modality: args.modality || 'linear',
        persona_id: args.persona_id,
      });
    case 'chat_list_sessions':
      return apiGet('/api/v1/chat/sessions');
    case 'chat_get_session':
      return apiGet(`/api/v1/chat/sessions/${args.session_id}`);
    case 'chat_send_message': {
      const body: Record<string, unknown> = { content: args.content };
      if (args.attachment_ids) body.attachment_ids = args.attachment_ids;
      return apiPost(`/api/v1/chat/sessions/${args.session_id}/messages`, body);
    }
    case 'chat_interrupt':
      return apiPost(`/api/v1/chat/sessions/${args.session_id}/interrupt`);
    case 'chat_resume':
      return apiPost(`/api/v1/chat/sessions/${args.session_id}/resume`);
    case 'chat_delete_session':
      return apiDelete(`/api/v1/chat/sessions/${args.session_id}`);
    case 'chat_rename_session':
      return apiPut(`/api/v1/chat/sessions/${args.session_id}/name`, { name: args.name });
    case 'chat_respond_interaction':
      return apiPost(`/api/v1/chat/sessions/${args.session_id}/interaction`, args.response);
    case 'chat_approve_tool':
      return apiPost(`/api/v1/chat/sessions/${args.session_id}/agents/${args.agent_id}/interaction`, {
        request_id: args.request_id,
        payload: { type: 'tool_approval', approved: args.approved ?? true, allow_session: args.allow_session ?? false, allow_agent: args.allow_agent ?? false },
      });

    // ── Chat streams (SSE subscriptions) ──
    case 'chat_subscribe_stream':
      connectSSE(`/api/v1/chat/sessions/${args.session_id}/stream`, 'chat:event');
      return null;

    // ── Agents ──
    case 'list_session_agents':
      return apiGet(`/api/v1/chat/sessions/${args.session_id}/agents`);
    case 'get_agent_telemetry':
      return apiGet(`/api/v1/chat/sessions/${args.session_id}/agents/telemetry`);
    case 'agent_respond_interaction':
      return apiPost(
        `/api/v1/chat/sessions/${args.session_id}/agents/${args.agent_id}/interaction`,
        args.response,
      );
    case 'agent_stage_subscribe':
      connectSSE(`/api/v1/chat/sessions/${args.session_id}/agents/stream`, 'stage:event');
      return null;
    case 'list_session_pending_questions':
      return apiGet(`/api/v1/chat/sessions/${args.session_id}/pending-questions`);
    case 'list_all_pending_questions':
      return apiGet('/api/v1/pending-questions');
    case 'list_pending_approvals':
      return apiGet('/api/v1/pending-approvals');
    case 'list_pending_interactions':
      return apiGet('/api/v1/pending-interactions');
    case 'get_pending_interaction_counts':
      return apiGet('/api/v1/pending-interaction-counts');

    // ── Approval stream ──
    case 'subscribe_approval_stream':
      connectSSE('/api/v1/approval-events', 'approval:event');
      return null;

    // ── Workflows ──
    case 'workflow_list_definitions':
      return apiGet('/api/v1/workflows/definitions');
    case 'workflow_get_definition':
      return apiGet(`/api/v1/workflows/definitions/${encodeURIComponent(args.name as string)}`);
    case 'workflow_save_definition':
      return apiPost('/api/v1/workflows/definitions', { yaml: args.yaml });
    case 'workflow_delete_definition':
      return apiDelete(`/api/v1/workflows/definitions/${encodeURIComponent(args.name as string)}`);
    case 'workflow_reset_definition':
      return apiPost(`/api/v1/workflows/definitions/${encodeURIComponent(args.name as string)}/reset`);
    case 'workflow_launch':
      return apiPost('/api/v1/workflows/instances', {
        definition: args.definition,
        definition_id: args.definition_id,
        parent_session_id: args.parent_session_id,
        inputs: args.inputs || {},
      });
    case 'workflow_list_instances':
      return apiGet(`/api/v1/workflows/instances${args.session_id ? `?session_id=${args.session_id}` : ''}`);
    case 'workflow_get_instance':
      return apiGet(`/api/v1/workflows/instances/${args.instance_id}`);
    case 'workflow_pause':
      return apiPost(`/api/v1/workflows/instances/${args.instance_id}/pause`);
    case 'workflow_resume':
      return apiPost(`/api/v1/workflows/instances/${args.instance_id}/resume`);
    case 'workflow_kill':
      return apiPost(`/api/v1/workflows/instances/${args.instance_id}/kill`);
    case 'workflow_respond_gate':
      return apiPost(
        `/api/v1/workflows/instances/${args.instance_id}/steps/${args.step_id}/respond`,
        { response: args.response },
      );
    case 'workflow_subscribe_events':
      connectSSE('/api/v1/workflows/events', 'workflow:event');
      return null;

    // ── Models ──
    case 'model_router_snapshot':
      return apiGet('/api/v1/model/router');

    // ── MCP ──
    case 'mcp_list_servers':
      return apiGet('/api/v1/mcp/servers');
    case 'mcp_list_tools':
      return apiGet(`/api/v1/mcp/servers/${args.server_id}/tools`);
    case 'mcp_list_resources':
      return apiGet(`/api/v1/mcp/servers/${args.server_id}/resources`);
    case 'mcp_list_prompts':
      return apiGet(`/api/v1/mcp/servers/${args.server_id}/prompts`);
    case 'mcp_list_notifications':
      return apiGet(`/api/v1/mcp/notifications${args.limit ? `?limit=${args.limit}` : ''}`);

    // ── Tools ──
    case 'tools_list':
      return apiGet('/api/v1/tools');

    // ── Skills ──
    case 'skills_list_installed':
      return apiGet('/api/v1/skills/sources');
    case 'skills_list_installed_for_persona':
      return apiGet(`/api/v1/personas/${encodeURIComponent(args.persona_id as string || args.persona_id as string)}/skills`);

    // ── Personas ──
    case 'list_personas':
      return apiGet(`/api/v1/config/personas${args.include_archived ? '?include_archived=true' : ''}`);
    case 'save_personas':
      return apiPut('/api/v1/config/personas', args.personas ?? args);
    case 'copy_persona':
      return apiPost('/api/v1/config/personas/copy', args);

    // ── Connectors ──
    case 'list_connectors':
      return apiGet('/api/v1/config/connectors');

    // ── Bots ──
    case 'list_bots':
      return apiGet('/api/v1/bots');

    // ── SSE subscriptions ──
    case 'interactions_subscribe':
    case 'event_bus_subscribe':
    case 'services_subscribe_events':
    case 'scheduler_subscribe_events':
    case 'workspace_subscribe_index_status':
    case 'bot_subscribe':
    case 'ensure_bot_stream':
      return null;

    // ── Misc / no-ops ──
    case 'open_url':
    case 'write_frontend_log':
    case 'load_secret':
    case 'save_secret':
    case 'delete_secret':
    case 'set_user_status':
    case 'set_session_permissions':
    case 'chat_link_workspace':
    case 'fetch_provider_models':
      return null;

    default:
      console.warn(`[tauri-api-bridge] unmapped command: ${command}`, args);
      return null;
  }
}

// ── Plugin handler ─────────────────────────────────────────────────────

function bridgePlugin(plugin: string, command: string, args: Record<string, unknown> = {}): unknown {
  if (plugin === 'event') {
    if (command === 'listen') {
      const eventName = args.event as string;
      const handler = args.handler as number;
      const cb = _callbackMap.get(handler);
      if (cb) {
        if (!_eventListeners.has(eventName)) _eventListeners.set(eventName, new Set());
        _eventListeners.get(eventName)!.add(cb as EventCallback);
      }
      return handler;
    }
    if (command === 'unlisten') {
      return undefined;
    }
  }
  if (plugin === 'dialog') {
    if (command === 'open') return '/tmp/mock-folder';
  }
  console.warn(`[tauri-api-bridge] unmapped plugin: ${plugin}.${command}`, args);
  return undefined;
}

// ── Install ────────────────────────────────────────────────────────────

export function installTauriBridge(daemon_url: string, authToken: string) {
  _config = { daemon_url: daemon_url.replace(/\/$/, ''), authToken };

  (window as any).__TAURI_INTERNALS__ = {
    invoke: bridgeInvoke,
    convertFileSrc: (path: string) => path,
    metadata: () => ({}),
    transformCallback(callback?: Function, once?: boolean): number {
      const id = _callbackCounter++;
      if (callback) {
        if (once) {
          _callbackMap.set(id, (...args: unknown[]) => {
            _callbackMap.delete(id);
            callback(...args);
          });
        } else {
          _callbackMap.set(id, callback);
        }
      }
      return id;
    },
    plugin: bridgePlugin,
  };

  // Event listener infrastructure (same shape as tauri-mock)
  (window as any).__TAURI_EVENT_LISTENERS__ = _eventListeners;
  (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => {},
  };

  // Intercept fetch to add auth headers for daemon requests
  const originalFetch = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : (input as Request).url;
    if (url.startsWith(_config.daemon_url) || url.startsWith('/api/')) {
      const fullUrl = url.startsWith('/api/') ? apiUrl(url) : url;
      const headers = new Headers(init?.headers);
      headers.set('Authorization', `Bearer ${_config.authToken}`);
      return originalFetch(fullUrl, { ...init, headers });
    }
    return originalFetch(input, init);
  };
}

/** Close all SSE connections and clean up. */
export function teardownBridge() {
  for (const src of _activeSources) {
    src.close();
  }
  _activeSources.length = 0;
  _eventListeners.clear();
  _callbackMap.clear();
}
