/**
 * @hivemind-os/plugin-sdk — Public API
 *
 * The main entry point for building Hivemind connector plugins.
 */
export { definePlugin } from "./define-plugin.js";
export { z } from "./schema.js";
export { serializeConfigSchema, getFieldMeta, type HivemindFieldMeta, type SerializedConfigSchema, type SerializedFieldSchema, } from "./schema.js";
export type { PluginDefinition, PluginContext, ToolDefinition, ToolResult, ToolAnnotations, Artifact, AuthConfig, OAuth2AuthConfig, TokenAuthConfig, TokenField, IncomingMessage, MessageSender, MessageAttachment, SecretStore, PersistentStore, Logger, NotificationPayload, NotificationAction, PluginStatus, ScheduleOptions, HttpClient, DataDirectory, HostInfo, ConnectorDiscovery, PersonaDiscovery, ConnectorInfo, PersonaInfo, } from "./types.js";
export { JsonRpcTransport, JsonRpcTransportError } from "./transport.js";
export { startPluginRuntime } from "./runtime.js";
//# sourceMappingURL=index.d.ts.map