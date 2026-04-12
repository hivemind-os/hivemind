/**
 * Mock layer for @tauri-apps/api used by the full-app test harness.
 * Intercepts invoke() and listen() so the SolidJS app can render
 * in a plain browser without a real Tauri backend.
 */

// ── Mock Data ────────────────────────────────────────────────────────

export const MOCK_SESSIONS = [
  {
    id: 'session-1',
    title: 'Test Chat Session',
    modality: 'linear' as const,
    state: 'idle' as const,
    queued_count: 0,
    updated_at_ms: Date.now() - 60_000,
    last_message_preview: 'Hello, can you help me?',
    workspace_path: '/tmp/workspace',
    workspace_linked: true,
  },
  {
    id: 'session-2',
    title: 'Spatial Session',
    modality: 'spatial' as const,
    state: 'idle' as const,
    queued_count: 0,
    updated_at_ms: Date.now() - 120_000,
    last_message_preview: null,
    workspace_path: '',
    workspace_linked: false,
  },
  {
    id: 'session-3',
    title: 'Code Review',
    modality: 'linear' as const,
    state: 'idle' as const,
    queued_count: 0,
    updated_at_ms: Date.now() - 30_000,
    last_message_preview: 'Review my PR please',
    workspace_path: '/tmp/code-review',
    workspace_linked: true,
  },
];

export const MOCK_SESSION_SNAPSHOT = {
  id: 'session-1',
  title: 'Test Chat Session',
  modality: 'linear',
  state: 'idle',
  queued_count: 0,
  active_stage: null,
  active_intent: null,
  active_thinking: null,
  last_error: null,
  recalled_memories: [],
  workspace_path: '/tmp/workspace',
  workspace_linked: true,
  messages: [
    { id: 'msg-1', role: 'user', content: 'Hello, can you help me?', created_at_ms: Date.now() - 120_000, attachments: [] },
    { id: 'msg-2', role: 'assistant', content: 'Of course! How can I assist you today?', created_at_ms: Date.now() - 110_000, attachments: [] },
    { id: 'msg-3', role: 'user', content: 'Write a Rust function to sort a vector', created_at_ms: Date.now() - 60_000, attachments: [] },
    { id: 'msg-4', role: 'assistant', content: '```rust\nfn sort_vec(mut v: Vec<i32>) -> Vec<i32> {\n    v.sort();\n    v\n}\n```', created_at_ms: Date.now() - 50_000, attachments: [] },
  ],
  created_at_ms: Date.now() - 3600_000,
  updated_at_ms: Date.now() - 50_000,
};

export const MOCK_DAEMON_STATUS = {
  version: '0.1.0-test',
  uptime_secs: 3600,
  pid: 12345,
  platform: 'windows',
  bind: 'http://127.0.0.1:9090',
};

export const MOCK_CONTEXT = {
  daemon_url: 'http://127.0.0.1:9090',
  config_path: '/tmp/hivemind-config',
  knowledge_graph_path: '/tmp/hivemind-kg',
  risk_ledger_path: '/tmp/hivemind-risk',
};

export const MOCK_PERSONAS = [
  { id: 'system/general', name: 'Default', description: 'General purpose agent', system_prompt: 'You are helpful.', loop_strategy: 'react', preferred_models: ['gpt-4'], allowed_tools: ['*'], mcp_servers: [] },
  { id: 'user/coder', name: 'Coder', description: 'Code-focused agent', system_prompt: `# Code Assistant

You are an expert **coding assistant** specializing in:

- TypeScript and JavaScript
- Rust and systems programming
- Python for scripting

## Rules

1. Always write \`clean\`, well-documented code
2. Follow the project's existing conventions
3. Include error handling for all operations

## Output Format

Use fenced code blocks with language tags:

\`\`\`typescript
function example(): string {
  return "hello";
}
\`\`\`

> Important: Never modify files without asking first.`, loop_strategy: 'react', preferred_models: ['gpt-4'], allowed_tools: ['fs.*', 'shell.*'], mcp_servers: [] },
  { id: 'user/reviewer', name: 'Reviewer', description: 'Code reviewer', system_prompt: 'You review code.', loop_strategy: 'plan_then_execute', preferred_models: ['claude-3'], allowed_tools: ['fs.read_file'], mcp_servers: [] },
];

export const MOCK_TOOLS = [
  { id: 'fs.read_file', name: 'Read File', description: 'Read the contents of a file from disk', category: 'filesystem', input_schema: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } },
  { id: 'fs.write_file', name: 'Write File', description: 'Write content to a file on disk', category: 'filesystem', input_schema: { type: 'object', properties: { path: { type: 'string' }, content: { type: 'string' } }, required: ['path', 'content'] } },
  { id: 'fs.list_dir', name: 'List Directory', description: 'List files and directories in a path', category: 'filesystem', input_schema: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } },
  { id: 'fs.delete', name: 'Delete', description: 'Delete a file or directory', category: 'filesystem', input_schema: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } },
  { id: 'shell.execute', name: 'Execute Shell', description: 'Run a shell command and return output', category: 'shell', input_schema: { type: 'object', properties: { command: { type: 'string' } }, required: ['command'] } },
  { id: 'shell.process_start', name: 'Process Start', description: 'Start a long-running background process', category: 'shell', input_schema: { type: 'object', properties: { command: { type: 'string' } }, required: ['command'] } },
  { id: 'shell.process_write', name: 'Process Write', description: 'Write input to a running process', category: 'shell', input_schema: { type: 'object', properties: { pid: { type: 'number' }, input: { type: 'string' } }, required: ['pid', 'input'] } },
  { id: 'http.request', name: 'HTTP Request', description: 'Make an HTTP request to a URL', category: 'http', input_schema: { type: 'object', properties: { url: { type: 'string' }, method: { type: 'string' } }, required: ['url'] } },
  { id: 'http.download', name: 'Download', description: 'Download a file from a URL', category: 'http', input_schema: { type: 'object', properties: { url: { type: 'string' }, dest: { type: 'string' } }, required: ['url', 'dest'] } },
  { id: 'search.grep', name: 'Grep', description: 'Search file contents with regex patterns', category: 'search', input_schema: { type: 'object', properties: { pattern: { type: 'string' } }, required: ['pattern'] } },
  { id: 'search.glob', name: 'Glob', description: 'Find files matching a glob pattern', category: 'search', input_schema: { type: 'object', properties: { pattern: { type: 'string' } }, required: ['pattern'] } },
  { id: 'mcp.filesystem.read', name: 'read', description: 'MCP filesystem server read operation', category: 'mcp', input_schema: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } },
  { id: 'mcp.filesystem.write', name: 'write', description: 'MCP filesystem server write operation', category: 'mcp', input_schema: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } },
  { id: 'mcp.github.create_issue', name: 'create_issue', description: 'Create a GitHub issue', category: 'mcp', input_schema: { type: 'object', properties: { title: { type: 'string' } }, required: ['title'] } },
  { id: 'mcp.github.search_repos', name: 'search_repos', description: 'Search GitHub repositories', category: 'mcp', input_schema: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] } },
  { id: 'workspace.index', name: 'Index', description: 'Index workspace files for search', category: 'workspace', input_schema: { type: 'object', properties: {} } },
  { id: 'workspace.search', name: 'Search', description: 'Search indexed workspace files', category: 'workspace', input_schema: { type: 'object', properties: { query: { type: 'string' } }, required: ['query'] } },
];

export const MOCK_CONFIG: Record<string, unknown> = {
  setup_completed: true,
  daemon: { log_level: 'info', event_bus_capacity: 256 },
  api: { bind: '127.0.0.1:9090', http_enabled: true },
  security: {
    override_policy: { internal: 'warn', confidential: 'block', restricted: 'block' },
    prompt_injection: { enabled: true, action_on_detection: 'warn', confidence_threshold: 0.8, cache_ttl_secs: 300, scanner_models: [] },
    default_permissions: [{ tool_pattern: '*', scope: 'session', decision: 'ask' }],
  },
  models: {
    providers: [
      { id: 'a1b2c3d4-e5f6-7890-abcd-ef1234567890', name: 'OpenAI', kind: 'openai', base_url: 'https://api.openai.com/v1', auth: 'api_key', models: ['gpt-4', 'gpt-3.5-turbo'], model_capabilities: {}, channel_class: 'public', priority: 1, enabled: true, options: {} },
      { id: 'b2c3d4e5-f6a7-8901-bcde-f12345678901', name: 'Anthropic', kind: 'anthropic', base_url: 'https://api.anthropic.com', auth: 'api_key', models: ['claude-3-opus', 'claude-3-sonnet'], model_capabilities: {}, channel_class: 'public', priority: 2, enabled: true, options: {} },
    ],
  },
  local_models: { enabled: false, storage_path: null, max_loaded_models: 2, max_download_concurrent: 1, auto_evict: true },
  compaction: { enabled: true, max_tokens: 8000, strategy: 'summarize-only', summary_max_tokens: 2000, extraction_model: null, max_summaries_in_context: 3, trigger_threshold: 0.75, keep_recent_turns: 10 },
  hf_token: null,
};

export const MOCK_MODEL_ROUTER = {
  providers: [
    { id: 'openai', apiType: 'openai', channel_class: 'public', capabilities: ['chat', 'vision', 'tool-use'], models: ['gpt-4', 'gpt-3.5-turbo'], priority: 1, available: true },
    { id: 'anthropic', apiType: 'anthropic', channel_class: 'public', capabilities: ['chat', 'vision'], models: ['claude-3-opus'], priority: 2, available: true },
  ],
};

export const MOCK_WORKSPACE_FILES = [
  { path: 'src', name: 'src', isDir: true, classification: null, children: [
    { path: 'src/main.rs', name: 'main.rs', isDir: false, classification: 'INTERNAL', size: 1024, auditStatus: 'clean' },
    { path: 'src/lib.rs', name: 'lib.rs', isDir: false, classification: 'INTERNAL', size: 2048 },
    { path: 'src/utils.rs', name: 'utils.rs', isDir: false, classification: 'PUBLIC', size: 512, auditStatus: 'suspicious' },
    { path: 'src/config', name: 'config', isDir: true, classification: null, children: [
      { path: 'src/config/mod.rs', name: 'mod.rs', isDir: false, classification: 'INTERNAL', size: 300 },
      { path: 'src/config/settings.rs', name: 'settings.rs', isDir: false, classification: 'INTERNAL', size: 1200 },
      { path: 'src/config/env.rs', name: 'env.rs', isDir: false, classification: 'PUBLIC', size: 420 },
    ]},
    { path: 'src/api', name: 'api', isDir: true, classification: null, children: [
      { path: 'src/api/mod.rs', name: 'mod.rs', isDir: false, classification: 'INTERNAL', size: 200 },
      { path: 'src/api/routes.rs', name: 'routes.rs', isDir: false, classification: 'INTERNAL', size: 3500, auditStatus: 'clean' },
      { path: 'src/api/handlers.rs', name: 'handlers.rs', isDir: false, classification: 'INTERNAL', size: 5200 },
      { path: 'src/api/middleware.rs', name: 'middleware.rs', isDir: false, classification: 'SECRET', size: 1800 },
    ]},
    { path: 'src/models', name: 'models', isDir: true, classification: null, children: [
      { path: 'src/models/user.rs', name: 'user.rs', isDir: false, classification: 'INTERNAL', size: 900 },
      { path: 'src/models/session.rs', name: 'session.rs', isDir: false, classification: 'INTERNAL', size: 750 },
    ]},
  ]},
  { path: 'tests', name: 'tests', isDir: true, classification: null, children: [
    { path: 'tests/integration.rs', name: 'integration.rs', isDir: false, classification: 'PUBLIC', size: 4096 },
    { path: 'tests/unit.rs', name: 'unit.rs', isDir: false, classification: 'PUBLIC', size: 2048 },
  ]},
  { path: 'docs', name: 'docs', isDir: true, classification: null, children: [
    { path: 'docs/README.md', name: 'README.md', isDir: false, classification: 'PUBLIC', size: 3200 },
    { path: 'docs/API.md', name: 'API.md', isDir: false, classification: 'PUBLIC', size: 5400 },
  ]},
  { path: 'Cargo.toml', name: 'Cargo.toml', isDir: false, classification: 'PUBLIC', size: 256 },
  { path: 'Cargo.lock', name: 'Cargo.lock', isDir: false, classification: 'PUBLIC', size: 45000 },
  { path: 'README.md', name: 'README.md', isDir: false, classification: 'PUBLIC', size: 1500 },
  { path: '.gitignore', name: '.gitignore', isDir: false, classification: null, size: 85 },
];

export const MOCK_WORKFLOW_DEFINITIONS = [
  { name: 'ci-pipeline', version: '1', description: 'CI/CD Pipeline', mode: 'standard', trigger_types: ['manual'], step_count: 4, created_at_ms: Date.now() - 86400_000, updated_at_ms: Date.now() - 3600_000, bundled: false, archived: false },
  { name: 'code-review', version: '1', description: 'Automated code review', mode: 'chat', trigger_types: ['manual', 'event'], step_count: 3, created_at_ms: Date.now() - 172800_000, updated_at_ms: Date.now() - 7200_000, bundled: true, archived: false },
  { name: 'data-pipeline', version: '2', description: 'Data ETL workflow', mode: 'standard', trigger_types: ['schedule'], step_count: 6, created_at_ms: Date.now() - 259200_000, updated_at_ms: Date.now() - 86400_000, bundled: false, archived: false },
];

export const MOCK_WORKFLOW_INSTANCES = [
  { id: 'wf-inst-1', definition_name: 'ci-pipeline', definition_version: '1', status: 'running', parent_session_id: 'mock-session-1', created_at_ms: Date.now() - 300_000, completed_at_ms: null, step_count: 4, completed_step_count: 2 },
  { id: 'wf-inst-2', definition_name: 'code-review', definition_version: '1', status: 'completed', parent_session_id: 'mock-session-1', created_at_ms: Date.now() - 7200_000, completed_at_ms: Date.now() - 3600_000, step_count: 3, completed_step_count: 3 },
  { id: 'wf-inst-3', definition_name: 'ci-pipeline', definition_version: '1', status: 'failed', parent_session_id: 'mock-session-2', created_at_ms: Date.now() - 14400_000, completed_at_ms: Date.now() - 14000_000, step_count: 4, completed_step_count: 2 },
];

export const MOCK_SCHEDULED_TASKS = [
  { id: 'task-1', name: 'Daily Report', description: 'Generate daily status report', schedule: { type: 'cron', expression: '0 0 9 * * * *' }, action: { type: 'invoke_agent', persona_id: 'default', task: 'Generate report' }, status: 'pending', created_at_ms: Date.now() - 86400_000, updated_at_ms: Date.now() - 86400_000, last_run_ms: Date.now() - 86400_000, next_run_ms: Date.now() + 86400_000, run_count: 5 },
  { id: 'task-2', name: 'Health Check', description: 'System health check', schedule: { type: 'cron', expression: '0 */5 * * * * *' }, action: { type: 'http_webhook', url: 'http://localhost:8080/health', method: 'GET' }, status: 'pending', created_at_ms: Date.now() - 300_000, updated_at_ms: Date.now() - 300_000, last_run_ms: Date.now() - 300_000, next_run_ms: Date.now() + 300_000, run_count: 100 },
];

export const MOCK_KG_STATS = {
  node_count: 42,
  edge_count: 67,
  categories: ['function', 'module', 'file', 'concept'],
};

export const MOCK_MCP_SERVERS = [
  { id: 'filesystem', transport: 'stdio', channel_class: 'local-only', enabled: true, auto_connect: true, reactive: false, status: 'connected', last_error: null, tool_count: 2, resource_count: 3, prompt_count: 0 },
  { id: 'github', transport: 'sse', channel_class: 'public', enabled: true, auto_connect: false, reactive: false, status: 'connected', last_error: null, tool_count: 5, resource_count: 0, prompt_count: 1 },
  { id: 'postgres-db', transport: 'stdio', channel_class: 'local-only', enabled: true, auto_connect: true, reactive: true, status: 'disconnected', last_error: 'Connection refused: localhost:5432', tool_count: 0, resource_count: 0, prompt_count: 0 },
  { id: 'web-browser', transport: 'sse', channel_class: 'public', enabled: false, auto_connect: false, reactive: false, status: 'disconnected', last_error: null, tool_count: 0, resource_count: 0, prompt_count: 0 },
  { id: 'kubernetes', transport: 'stdio', channel_class: 'restricted', enabled: true, auto_connect: true, reactive: false, status: 'connected', last_error: null, tool_count: 8, resource_count: 2, prompt_count: 0 },
];

export const MOCK_INSTALLED_MODELS = [
  { id: 'llama-3-8b', name: 'Llama 3 8B', filename: 'llama-3-8b-q4_k_m.gguf', hub_repo: 'meta-llama/Llama-3-8B-GGUF', runtime: 'llama.cpp', loaded: true, size_bytes: 4_800_000_000, quantization: 'Q4_K_M', capabilities: { tasks: ['text-generation'] }, inference_params: {} },
  { id: 'mistral-7b', name: 'Mistral 7B', filename: 'mistral-7b-q5_k_m.gguf', hub_repo: 'TheBloke/Mistral-7B-GGUF', runtime: 'llama.cpp', loaded: false, size_bytes: 4_100_000_000, quantization: 'Q5_K_M', capabilities: { tasks: ['text-generation'] }, inference_params: {} },
];

// ── Invoke Handler Registry ──────────────────────────────────────────

type InvokeHandler = (args: Record<string, unknown>) => unknown;

const invokeHandlers: Record<string, InvokeHandler> = {
  'status_heartbeat': () => ({ status: 'active' }),
  'get_user_status': () => ({ status: 'active' }),
  'daemon_start': () => MOCK_DAEMON_STATUS,
  'daemon_status': () => MOCK_DAEMON_STATUS,
  'app_context': () => MOCK_CONTEXT,
  'config_show': () => 'daemon:\n  bind: "127.0.0.1:9090"\n',
  'config_get': () => MOCK_CONFIG,
  'config_save': () => ({ saved: true, config_path: '/mock/config.yaml', message: 'Saved successfully.' }),
  'tools_list': () => MOCK_TOOLS,
  'list_personas': () => MOCK_PERSONAS,

  'chat_list_sessions': () => MOCK_SESSIONS,
  'chat_create_session': (args) => ({
    id: `session-${Date.now()}`,
    title: 'New Session',
    modality: args.modality ?? 'linear',
    state: 'idle',
    queued_count: 0,
    active_stage: null,
    active_intent: null,
    active_thinking: null,
    last_error: null,
    recalled_memories: [],
    messages: [],
    updated_at_ms: Date.now(),
    created_at_ms: Date.now(),
    last_message_preview: null,
    workspace_path: (args.workspace as string) ?? '',
    workspace_linked: !!args.workspace,
  }),
  'chat_get_session': (args) => ({
    ...MOCK_SESSION_SNAPSHOT,
    id: (args.session_id as string) ?? MOCK_SESSION_SNAPSHOT.id,
  }),
  'chat_send_message': () => ({ kind: 'queued', session: { ...MOCK_SESSION_SNAPSHOT, updated_at_ms: Date.now() } }),
  'chat_interrupt': () => ({}),
  'chat_resume': () => ({}),
  'chat_delete_session': () => ({}),
  'chat_subscribe_stream': () => ({}),
  'chat_upload_file': () => ({ path: '/uploaded/file.txt' }),
  'chat_link_workspace': (args) => ({
    id: args.session_id ?? 'session-1',
    title: 'Test Chat Session',
    modality: 'linear',
    state: 'idle',
    queued_count: 0,
    updated_at_ms: Date.now(),
    last_message_preview: null,
    workspace_path: (args.path as string) ?? '/tmp/workspace',
    workspace_linked: true,
  }),
  'chat_approve_tool': () => ({}),
  'chat_respond_interaction': () => ({}),

  'workspace_list_files': () => MOCK_WORKSPACE_FILES,
  'workspace_read_file': () => ({ path: 'src/main.rs', content: '// file content\nfn main() {}\n', is_binary: false, mime_type: 'text/x-rust', size: 1024, read_only: false }),
  'workspace_save_file': () => ({}),
  'workspace_delete_entry': () => ({}),
  'workspace_move_entry': () => ({}),
  'workspace_create_directory': () => ({}),
  'workspace_reindex_file': () => ({}),
  'workspace_set_classification_override': () => ({}),
  'workspace_clear_classification_override': () => ({}),
  'workspace_audit_file': () => ({ classifications: [], risks: [] }),
  'workspace_subscribe_index_status': () => ({}),

  'clipboard_copy_files': () => ({}),
  'clipboard_paste_files': () => ({}),
  'clipboard_read_file_paths': () => [],
  'clipboard_cancel_paste': () => ({}),
  'clipboard_resolve_conflict': () => ({}),

  'model_router_snapshot': () => MOCK_MODEL_ROUTER,
  'list_connectors': () => [],
  'load_secret': () => '',
  'save_secret': () => ({}),
  'delete_secret': () => ({}),
  'open_url': () => ({}),
  'write_frontend_log': () => ({}),
  'save_personas': () => ({}),
  'reset_persona': () => ({ id: 'system/general', name: 'General Agent', description: '', system_prompt: '', loop_strategy: 'react', allowed_tools: ['*'], mcp_servers: [], bundled: true }),

  'set_session_permissions': () => ({}),
  'get_session_permissions': () => ({
    allowed_tools: ['*'],
    denied_tools: [],
    auto_approve: false,
    max_auto_approve_risk: 'low',
  }),

  'launch_bot': () => ({ agent_id: `bot-${Date.now()}` }),
  'message_bot': () => ({}),
  'bot_interaction': () => ({}),
  'bot_subscribe': () => ({}),
  'ensure_bot_stream': () => ({}),
  'activate_bot': () => ({}),
  'deactivate_bot': () => ({}),
  'delete_bot': () => ({}),
  'pause_session_agent': () => ({}),
  'resume_session_agent': () => ({}),
  'restart_session_agent': () => ({}),
  'kill_session_agent': () => ({}),
  'agent_stage_subscribe': () => ({}),
  'list_session_agents': () => [],
  'get_agent_telemetry': () => null,
  'list_bots': () => [],
  'get_bot_telemetry': () => null,
  'agent_respond_interaction': () => ({}),
  'set_bot_permissions': () => ({}),
  'subscribe_approval_stream': () => ({}),
  'list_session_pending_questions': () => [],

  'local_models_list': () => ({ models: MOCK_INSTALLED_MODELS, total_size_bytes: 8_900_000_000 }),
  'local_models_update_params': () => ({}),
  'local_models_downloads': () => [],
  'local_models_hardware': () => ({ cpu: 'Test CPU', ram_bytes: 16_000_000_000, gpu: null }),
  'local_models_resource_usage': () => ({ cpu_percent: 25, ram_bytes: 8_000_000_000 }),
  'local_models_storage': () => ({ total_bytes: 100_000_000_000, free_bytes: 50_000_000_000 }),
  'local_models_search': () => ({ models: [], total: 0 }),
  'local_models_hub_files': () => ({ files: [] }),
  'local_models_install': () => ({}),
  'local_models_remove': () => ({}),
  'local_models_remove_download': () => ({}),
  'local_model_load': () => ({}),
  'local_model_unload': () => ({}),
  'lookup_model_metadata': () => null,
  'fetch_provider_models': () => [],

  'workflow_list_definitions': () => MOCK_WORKFLOW_DEFINITIONS,
  'workflow_list_instances': () => ({ items: MOCK_WORKFLOW_INSTANCES, total: MOCK_WORKFLOW_INSTANCES.length }),
  'workflow_get_instance': () => ({ id: 'wf-inst-1', definition_name: 'ci-pipeline', definition_version: '1', status: 'running', steps: {}, created_at_ms: Date.now() - 300_000, completed_at_ms: null }),
  'workflow_save_definition': () => ({}),
  'workflow_delete_definition': () => ({}),
  'workflow_reset_definition': () => ({}),
  'workflow_archive_definition': () => ({ archived: true }),
  'workflow_check_definition_dependents': () => ({ dependents: [] }),
  'workflow_launch': () => ({ instance_id: `wf-${Date.now()}` }),
  'workflow_pause': () => ({}),
  'workflow_resume': () => ({}),
  'workflow_kill': () => ({}),
  'workflow_respond_gate': () => ({}),
  'workflow_get_definition': () => ({ name: 'test', version: '1', yaml: 'name: test\nversion: "1"\n' }),
  'workflow_subscribe_events': () => ({}),
  'workflow_copy_attachments': () => ({}),
  'workflow_delete_attachment': () => ({}),
  'workflow_ai_assist': () => ({ agent_id: `wf-ai-${Date.now()}` }),

  'mcp_list_servers': () => MOCK_MCP_SERVERS,
  'mcp_list_notifications': () => [],
  'mcp_list_tools': () => [],
  'mcp_connect_server': () => ({}),
  'mcp_disconnect_server': () => ({}),

  'skills_list_installed': () => [],
  'skills_list_installed_for_persona': () => [],
  'skills_set_enabled': () => ({}),
  'skills_set_enabled_for_persona': () => ({}),
  'skills_uninstall': () => ({}),
  'skills_uninstall_for_persona': () => ({}),
  'skills_rebuild_index': () => ({}),
  'skills_install': () => ({}),
  'skills_install_for_persona': () => ({}),
  'skills_set_sources': () => ({}),
  'skills_get_sources': () => [{ owner: 'test-org', repo: 'test-skills' }],
  'skills_discover': () => [
    {
      source_id: 'test-org/test-skills',
      source_path: 'skills/test-skill',
      manifest: {
        name: 'test-skill',
        description: 'A test skill for E2E testing',
        version: '1.0.0',
      },
    },
  ],
  'skills_audit': () => ({
    summary: 'Found 1 potential risk.',
    risks: [
      {
        id: 'RISK-001',
        severity: 'medium',
        probability: 0.5,
        description: 'Skill accesses filesystem',
        evidence: 'Uses read_file tool',
      },
    ],
  }),

  'services_list': () => [
    { id: 'scheduler', display_name: 'Scheduler', category: 'core', status: 'running', last_error: null },
    { id: 'trigger_manager', display_name: 'Trigger Manager', category: 'core', status: 'running', last_error: null },
    { id: 'event_log', display_name: 'Event Log', category: 'core', status: 'running', last_error: null },
    { id: 'chat', display_name: 'Chat Service', category: 'core', status: 'running', last_error: null },
    { id: 'connector', display_name: 'Connector Service', category: 'connector', status: 'stopped', last_error: null },
    { id: 'bot_supervisor', display_name: 'Bot Supervisor', category: 'agents', status: 'running', last_error: null },
    { id: 'inference', display_name: 'Inference Engine', category: 'inference', status: 'error', last_error: 'Model not loaded' },
  ],
  'services_get_logs': () => [
    { timestamp_ms: Date.now() - 5000, level: 'INFO', service: 'scheduler', message: 'Scheduler tick completed', fields: {} },
    { timestamp_ms: Date.now() - 3000, level: 'WARN', service: 'scheduler', message: 'Slow tick detected (2.1s)', fields: {} },
    { timestamp_ms: Date.now() - 1000, level: 'ERROR', service: 'scheduler', message: 'Failed to run task: timeout', fields: {} },
  ],
  'services_restart': () => ({}),
  'services_subscribe_events': () => ({}),

  'kg_update_node': () => ({}),

  'get_session_events': () => ({ events: [], total: 0 }),
  'list_session_processes': () => ({ processes: [] }),
  'kill_process': () => ({}),
  'get_process_status': () => ({ info: { id: 'proc-1', pid: 0, command: '', working_dir: null, status: { state: 'exited', code: 0 }, uptime_secs: 0, owner: { kind: 'unknown' } }, output: '' }),

  'propose_layout': () => ({ nodes: [] }),
  'recluster_canvas': () => ({ clusters: [] }),
};

// ── Global Mock Installation ─────────────────────────────────────────

/** Call log for test assertions */
export const invokeCalls: Array<{ command: string; args: unknown }> = [];

/** Override a specific invoke handler for testing */
export function mockInvoke(command: string, handler: InvokeHandler) {
  invokeHandlers[command] = handler;
}

/** Reset all mocks to defaults */
export function resetMocks() {
  invokeCalls.length = 0;
  const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
  if (listeners) listeners.clear();
}

/** Install mocks on the global window object */
export function installTauriMocks() {
  // Event ID counter for deterministic listener IDs
  let nextEventId = 1;
  // Map from eventId → { event, handlerCallbackId } for unlisten support
  const eventIdMap = new Map<number, { event: string; callbackId: number }>();

  // Mock @tauri-apps/api/core invoke
  (window as any).__TAURI_INTERNALS__ = {
    invoke: async (command: string, args: Record<string, unknown> = {}) => {
      // Route plugin:* commands to the plugin handler
      if (command.startsWith('plugin:')) {
        const [pluginPart, cmd] = command.slice(7).split('|');
        if ((window as any).__TAURI_INTERNALS__.plugin) {
          return (window as any).__TAURI_INTERNALS__.plugin(pluginPart, cmd, args);
        }
      }
      invokeCalls.push({ command, args });
      const handler = invokeHandlers[command];
      if (handler) {
        const result = handler(args);
        return result instanceof Promise ? result : Promise.resolve(result);
      }
      console.warn(`[tauri-mock] Unhandled invoke: ${command}`, args);
      return {};
    },
    convertFileSrc: (path: string) => path,
    metadata: () => ({}),
    transformCallback: (callback?: Function, _once?: boolean) => {
      const id = nextEventId++;
      if (callback) (window as any).__TAURI_INTERNALS__[`_${id}`] = callback;
      return id;
    },
  };

  // Expose invoke call log for Playwright tests
  (window as any).__TAURI_TEST_INVOKE_CALLS__ = invokeCalls;

  // Mock @tauri-apps/api/event listen — stores both our Set-based listeners and Tauri callback IDs
  const listeners = new Map<string, Set<Function>>();
  (window as any).__TAURI_EVENT_LISTENERS__ = listeners;

  // Mock __TAURI_EVENT_PLUGIN_INTERNALS__ (used by @tauri-apps/api/event _unlisten)
  (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event: string, eventId: number) => {
      const entry = eventIdMap.get(eventId);
      if (entry) {
        // Remove the callback from TAURI_INTERNALS
        delete (window as any).__TAURI_INTERNALS__[`_${entry.callbackId}`];
        eventIdMap.delete(eventId);
      }
    },
  };

  // Patch the Tauri event system
  (window as any).__TAURI_INTERNALS__.plugin = async (plugin: string, command: string, args: any) => {
    if (plugin === 'event' && command === 'listen') {
      const event = args.event;
      const handlerCallbackId = args.handler; // numeric ID from transformCallback
      if (!listeners.has(event)) listeners.set(event, new Set());

      // Resolve the actual callback function from TAURI_INTERNALS
      const callbackFn = (window as any).__TAURI_INTERNALS__[`_${handlerCallbackId}`];
      if (callbackFn) {
        listeners.get(event)!.add(callbackFn);
      }

      // Return a unique event ID (used by _unlisten)
      const eventId = nextEventId++;
      eventIdMap.set(eventId, { event, callbackId: handlerCallbackId });
      return eventId;
    }
    if (plugin === 'event' && command === 'unlisten') {
      return;
    }
    if (plugin === 'dialog') {
      if (command === 'open') return '/tmp/mock-folder';
      return null;
    }
    console.warn(`[tauri-mock] Unhandled plugin: ${plugin}/${command}`, args);
    return null;
  };

  // Mock fetch for daemon HTTP API
  const originalFetch = window.fetch;
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;

    if (url.includes('/api/v1/config/get')) {
      return new Response(JSON.stringify(MOCK_CONFIG), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/config') && init?.method === 'PUT') {
      return new Response('{}', { status: 200 });
    }
    if (url.includes('/api/v1/tools')) {
      return new Response(JSON.stringify(MOCK_TOOLS), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/knowledge/stats')) {
      return new Response(JSON.stringify(MOCK_KG_STATS), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/knowledge/nodes')) {
      return new Response(JSON.stringify([]), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/knowledge/search')) {
      return new Response(JSON.stringify([]), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/auth/github/status')) {
      return new Response(JSON.stringify({ connected: false }), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/config/connectors')) {
      return new Response(JSON.stringify([]), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/comms/audit')) {
      return new Response(JSON.stringify({ entries: [], total: 0 }), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/workflows/topics')) {
      return new Response(JSON.stringify([{ topic: 'build.completed', description: 'Build completed' }]), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/scheduler')) {
      return new Response(JSON.stringify(MOCK_SCHEDULED_TASKS), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/mcp/servers') && url.includes('/logs')) {
      return new Response(JSON.stringify([
        { timestamp: Date.now() - 60000, level: 'info', message: 'Server started successfully' },
        { timestamp: Date.now() - 30000, level: 'info', message: 'Connected to client' },
      ]), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/mcp/servers') && (url.includes('/connect') || url.includes('/disconnect') || url.includes('/install-runtime'))) {
      return new Response('{}', { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.match(/\/api\/v1\/sessions\/[^/]+\/mcp\/servers/)) {
      return new Response(JSON.stringify(MOCK_MCP_SERVERS), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    if (url.includes('/api/v1/mcp/servers')) {
      return new Response(JSON.stringify(MOCK_MCP_SERVERS), { status: 200, headers: { 'Content-Type': 'application/json' } });
    }

    // Fall through to original fetch for non-API requests
    return originalFetch(input, init);
  };
}

/** Emit a mock Tauri event */
export function emitTauriEvent(event: string, payload: unknown) {
  const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
  if (listeners?.has(event)) {
    for (const handler of listeners.get(event)!) {
      handler({ event, payload, id: Date.now() });
    }
  }
}

/** Get the count of registered listeners for an event */
export function listenerCount(event: string): number {
  const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
  return listeners?.get(event)?.size ?? 0;
}

/** Get all registered event names */
export function registeredEvents(): string[] {
  const listeners = (window as any).__TAURI_EVENT_LISTENERS__;
  return listeners ? Array.from(listeners.keys()) : [];
}

// ── Streaming Simulation Helpers ─────────────────────────────────────

/** Simulate a chat streaming token event */
export function emitChatToken(session_id: string, delta: string) {
  emitTauriEvent('chat:event', {
    session_id,
    event: { Token: { delta } },
  });
}

/** Simulate a chat stream done event */
export function emitChatDone(session_id: string) {
  emitTauriEvent('chat:done', { session_id });
}

/** Simulate a chat stream error event */
export function emitChatError(session_id: string, error: string) {
  emitTauriEvent('chat:error', { session_id, error });
}

/** Simulate a complete chat streaming sequence: tokens → done */
export function emitChatStreamSequence(session_id: string, tokens: string[], delayMs = 50): Promise<void> {
  return new Promise((resolve) => {
    let i = 0;
    const interval = setInterval(() => {
      if (i < tokens.length) {
        emitChatToken(session_id, tokens[i]);
        i++;
      } else {
        clearInterval(interval);
        emitChatDone(session_id);
        resolve();
      }
    }, delayMs);
  });
}

/** Simulate a tool call start event */
export function emitToolCallStart(session_id: string, tool_id: string, input?: unknown) {
  emitTauriEvent('chat:event', {
    session_id,
    event: { ToolCallStart: { tool_id: tool_id, input: input ?? {} } },
  });
}

/** Simulate a tool call result event */
export function emitToolCallResult(session_id: string, tool_id: string, output: string, isError = false) {
  emitTauriEvent('chat:event', {
    session_id,
    event: { ToolCallResult: { tool_id: tool_id, output, is_error: isError } },
  });
}

/** Simulate a user interaction request (tool approval) */
export function emitToolApprovalRequest(session_id: string, request_id: string, tool_id: string, input: string, reason: string) {
  emitTauriEvent('chat:event', {
    session_id,
    event: {
      UserInteractionRequired: {
        request_id: request_id,
        kind: { type: 'tool_approval', tool_id, input, reason },
      },
    },
  });
}

/** Simulate an approval event (for AgentApprovalToast) */
export function emitApprovalEvent(type: 'added' | 'resolved', data: Record<string, unknown>) {
  emitTauriEvent('approval:event', { type, ...data });
}

/** Simulate a workflow event */
export function emitWorkflowEvent(topic: string, payload?: Record<string, unknown>) {
  emitTauriEvent('workflow:event', { topic, payload: payload ?? {} });
}

/** Simulate an agent stage event */
export function emitStageEvent(session_id: string, event: Record<string, unknown>) {
  emitTauriEvent('stage:event', { session_id, event });
}
