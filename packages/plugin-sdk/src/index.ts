/**
 * @hivemind-os/plugin-sdk — Public API
 *
 * The main entry point for building Hivemind connector plugins.
 */

// Core API
export { definePlugin } from "./define-plugin.js";
export { z } from "./schema.js";

// Schema utilities
export {
  serializeConfigSchema,
  getFieldMeta,
  type HivemindFieldMeta,
  type SerializedConfigSchema,
  type SerializedFieldSchema,
} from "./schema.js";

// Types
export type {
  PluginDefinition,
  PluginContext,
  ToolDefinition,
  ToolResult,
  ToolAnnotations,
  Artifact,
  AuthConfig,
  OAuth2AuthConfig,
  TokenAuthConfig,
  TokenField,
  IncomingMessage,
  MessageSender,
  MessageAttachment,
  SecretStore,
  PersistentStore,
  Logger,
  NotificationPayload,
  NotificationAction,
  PluginStatus,
  ScheduleOptions,
  HttpClient,
  DataDirectory,
  HostInfo,
  ConnectorDiscovery,
  PersonaDiscovery,
  ConnectorInfo,
  PersonaInfo,
} from "./types.js";

// Transport (for advanced use cases)
export { JsonRpcTransport, JsonRpcTransportError } from "./transport.js";

// Runtime (for advanced use cases)
export { startPluginRuntime } from "./runtime.js";
