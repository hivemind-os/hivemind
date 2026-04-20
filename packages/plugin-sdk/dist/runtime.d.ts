/**
 * Plugin runtime — bootstraps the plugin process and wires up the
 * JSON-RPC transport, PluginContext, tool dispatch, and lifecycle hooks.
 *
 * This is the "main loop" of a plugin process. When a plugin calls
 * `definePlugin(...)`, the runtime takes over and handles all host
 * communication automatically.
 */
import type { PluginDefinition } from "./types.js";
import type { ZodRawShape } from "zod";
export declare function startPluginRuntime<TShape extends ZodRawShape>(definition: PluginDefinition<TShape>): void;
//# sourceMappingURL=runtime.d.ts.map