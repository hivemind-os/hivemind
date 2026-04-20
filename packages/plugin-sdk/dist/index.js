/**
 * @hivemind-os/plugin-sdk — Public API
 *
 * The main entry point for building Hivemind connector plugins.
 */
// Core API
export { definePlugin } from "./define-plugin.js";
export { z } from "./schema.js";
// Schema utilities
export { serializeConfigSchema, getFieldMeta, } from "./schema.js";
// Transport (for advanced use cases)
export { JsonRpcTransport, JsonRpcTransportError } from "./transport.js";
// Runtime (for advanced use cases)
export { startPluginRuntime } from "./runtime.js";
//# sourceMappingURL=index.js.map