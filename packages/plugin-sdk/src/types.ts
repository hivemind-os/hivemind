/**
 * @hivemind/plugin-sdk — Core types for the Hivemind plugin system.
 *
 * These types define the contract between a plugin process and the Hivemind host.
 * Under the hood, all communication happens via JSON-RPC over stdio.
 */

// ─── Plugin Definition ──────────────────────────────────────────────────────

import type { ZodObject, ZodRawShape } from "zod";

export interface PluginDefinition<
  TShape extends ZodRawShape = ZodRawShape,
> {
  /** Zod schema describing the plugin's configuration. Rendered as a form in the host UI. */
  configSchema: ZodObject<TShape>;

  /** Auth flow configuration (optional — for OAuth/token-based connectors). */
  auth?: AuthConfig;

  /** Tools provided to the AI agent. */
  tools: ToolDefinition[];

  /** Optional background loop function. Called when the host starts the plugin's loop. */
  loop?: (ctx: PluginContext) => Promise<void>;

  /** Called when the plugin is enabled. Use to validate config, warm caches. */
  onActivate?: (ctx: PluginContext) => Promise<void>;

  /** Called when the plugin is disabled. Use to clean up resources. */
  onDeactivate?: (ctx: PluginContext) => Promise<void>;
}

// ─── Auth ───────────────────────────────────────────────────────────────────

export type AuthConfig = OAuth2AuthConfig | TokenAuthConfig;

export interface OAuth2AuthConfig {
  type: "oauth2";
  authorizationUrl: string;
  tokenUrl: string;
  scopes: string[];
  clientId?: string;
  pkce?: boolean;
}

export interface TokenAuthConfig {
  type: "token";
  fields: TokenField[];
}

export interface TokenField {
  key: string;
  label: string;
  helpUrl?: string;
  helpText?: string;
}

// ─── Tools ──────────────────────────────────────────────────────────────────

import type { ZodType } from "zod";

export interface ToolDefinition<TParams = any> {
  /** Tool name — must be unique within the plugin. Used as `plugin.<pluginId>.<name>`. */
  name: string;

  /** Human-readable description shown to the AI agent. */
  description: string;

  /** Zod schema for the tool's input parameters. */
  parameters: ZodType<TParams>;

  /** Tool annotations (side effects, approval requirements, etc.). */
  annotations?: ToolAnnotations;

  /** The function that executes when the agent calls this tool. */
  execute: (params: TParams, ctx: PluginContext) => Promise<ToolResult>;
}

export interface ToolAnnotations {
  /** Whether this tool has side effects (creates/modifies external state). */
  sideEffects?: boolean;
  /** Approval mode: 'always' requires user approval, 'suggest' suggests approval. */
  approval?: "always" | "suggest" | "never";
  /** Whether the tool is read-only (no side effects). Overrides sideEffects if set. */
  readOnly?: boolean;
}

export interface ToolResult {
  /** The result content — string for simple results, object for structured data. */
  content: string | Record<string, unknown> | Array<unknown>;
  /** If true, the result represents an error. */
  isError?: boolean;
  /** Optional file/data artifacts attached to the result. */
  artifacts?: Artifact[];
}

export interface Artifact {
  name: string;
  mimeType: string;
  /** Base64-encoded content. */
  content?: string;
  /** URL to the artifact. */
  url?: string;
}

// ─── Plugin Context (Host API) ──────────────────────────────────────────────

export interface PluginContext<TConfig = Record<string, unknown>> {
  /** Plugin's unique ID (from package.json name). */
  pluginId: string;

  /** Resolved config values (from the config schema + user input). */
  config: TConfig;

  /** AbortSignal — fires when the host wants the plugin to stop. */
  signal: AbortSignal;

  // ─── Messaging ──────────────────────────────────────────────

  /**
   * Emit an incoming message into the host's connector pipeline.
   * The message flows through: dedup → classification → persona routing
   * → workflow triggers → notification → agent inbox.
   */
  emitMessage(msg: IncomingMessage): Promise<void>;

  /** Emit a batch of messages at once (more efficient for bulk imports). */
  emitMessages(msgs: IncomingMessage[]): Promise<void>;

  // ─── Secret Storage ─────────────────────────────────────────

  /** Plugin-scoped access to the host's OS keyring. */
  secrets: SecretStore;

  // ─── Persistent Key-Value Storage ───────────────────────────

  /** Plugin-scoped persistent storage for non-secret data (cursors, cache, state). */
  store: PersistentStore;

  // ─── Logging ────────────────────────────────────────────────

  /** Structured logger that forwards to the host's log system. */
  logger: Logger;

  // ─── Desktop Notifications ──────────────────────────────────

  /** Show a desktop notification to the user. */
  notify(notification: NotificationPayload): Promise<void>;

  // ─── Events ─────────────────────────────────────────────────

  /** Emit a custom event that workflow automations can trigger on. */
  emitEvent(eventType: string, payload: Record<string, unknown>): Promise<void>;

  // ─── Status ─────────────────────────────────────────────────

  /** Update the plugin's status displayed in the Settings UI. */
  updateStatus(status: PluginStatus): Promise<void>;

  // ─── Timers ─────────────────────────────────────────────────

  /** Cancellation-aware sleep. Rejects if the plugin is being stopped. */
  sleep(ms: number): Promise<void>;

  /** Register a recurring task with the host's scheduler. */
  schedule(opts: ScheduleOptions): Promise<void>;

  /** Unregister a scheduled task. */
  unschedule(id: string): Promise<void>;

  // ─── HTTP ───────────────────────────────────────────────────

  /** HTTP client proxied through the host (for future sandboxing support). */
  http: HttpClient;

  // ─── File System ────────────────────────────────────────────

  /** Access the plugin's private data directory (~/.hivemind/plugins/<id>/data/). */
  dataDir: DataDirectory;

  // ─── Host Info ──────────────────────────────────────────────

  /** Information about the host environment. */
  host: HostInfo;

  // ─── Discovery ──────────────────────────────────────────────

  /** Read-only access to connectors configured in the host. */
  connectors: ConnectorDiscovery;

  /** Read-only access to personas configured in the host. */
  personas: PersonaDiscovery;
}

// ─── Sub-interfaces for PluginContext ────────────────────────────────────────

export interface SecretStore {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
  has(key: string): Promise<boolean>;
}

export interface PersistentStore {
  get<T = unknown>(key: string): Promise<T | null>;
  set<T = unknown>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<void>;
  keys(): Promise<string[]>;
}

export interface Logger {
  debug(msg: string, data?: Record<string, unknown>): void;
  info(msg: string, data?: Record<string, unknown>): void;
  warn(msg: string, data?: Record<string, unknown>): void;
  error(msg: string, data?: Record<string, unknown>): void;
}

export interface NotificationPayload {
  title: string;
  body: string;
  action?: NotificationAction;
}

export interface NotificationAction {
  type: "open_session" | "open_settings" | "open_url";
  target: string;
}

export interface PluginStatus {
  state: "connected" | "connecting" | "disconnected" | "error" | "syncing";
  message?: string;
  progress?: number;
}

export interface ScheduleOptions {
  id: string;
  intervalSeconds: number;
  handler: () => Promise<void>;
}

export interface HttpClient {
  fetch(url: string, init?: RequestInit): Promise<Response>;
}

export interface DataDirectory {
  resolve(path: string): Promise<string>;
  readFile(path: string): Promise<string>;
  writeFile(path: string, content: string | Uint8Array): Promise<void>;
  readDir(path: string): Promise<string[]>;
  exists(path: string): Promise<boolean>;
  mkdir(path: string): Promise<void>;
  remove(path: string): Promise<void>;
}

export interface HostInfo {
  version: string;
  platform: "windows" | "macos" | "linux";
  capabilities: string[];
}

export interface ConnectorDiscovery {
  list(): Promise<ConnectorInfo[]>;
}

export interface PersonaDiscovery {
  list(): Promise<PersonaInfo[]>;
}

// ─── Messages ───────────────────────────────────────────────────────────────

export interface IncomingMessage {
  /** Unique dedup key — same source = same message (won't duplicate). */
  source: string;
  /** Channel/feed name — used for routing and display. */
  channel: string;
  /** Human-readable message content. */
  content: string;
  /** Sender information. */
  sender?: MessageSender;
  /** Structured metadata for workflow triggers and filtering. */
  metadata?: Record<string, unknown>;
  /** Override classification (otherwise host auto-classifies). */
  classification?: "personal" | "work" | "automated" | "spam";
  /** Thread ID for conversation threading. */
  threadId?: string;
  /** Timestamp (ISO 8601, defaults to now). */
  timestamp?: string;
  /** Attachments. */
  attachments?: MessageAttachment[];
}

export interface MessageSender {
  id: string;
  name: string;
  avatarUrl?: string;
}

export interface MessageAttachment {
  name: string;
  mimeType: string;
  url?: string;
  /** Base64-encoded content. */
  content?: string;
}

// ─── Discovery Types ────────────────────────────────────────────────────────

export interface ConnectorInfo {
  id: string;
  displayName: string;
  provider: string;
  enabled: boolean;
  services: string[];
  status: "connected" | "disconnected" | "error";
}

export interface PersonaInfo {
  id: string;
  name: string;
  description: string;
  isActive: boolean;
}

// ─── JSON-RPC Protocol Types (internal) ─────────────────────────────────────

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id?: string | number;
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: string | number;
  result?: unknown;
  error?: JsonRpcError;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}
