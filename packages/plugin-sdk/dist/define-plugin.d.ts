/**
 * definePlugin() — the main entry point for creating a Hivemind plugin.
 *
 * Call this as the default export of your plugin's index.ts:
 *
 * ```typescript
 * import { definePlugin, z } from '@hivemind-os/plugin-sdk';
 *
 * export default definePlugin({
 *   configSchema: z.object({ ... }),
 *   tools: [ ... ],
 *   loop: async (ctx) => { ... },
 * });
 * ```
 *
 * When the plugin process starts, `definePlugin()` automatically:
 * 1. Sets up the JSON-RPC transport (stdio)
 * 2. Registers all method handlers
 * 3. Starts listening for host commands
 */
import type { PluginDefinition } from "./types.js";
import type { ZodRawShape } from "zod";
export declare function definePlugin<TShape extends ZodRawShape>(definition: PluginDefinition<TShape>): PluginDefinition<TShape>;
//# sourceMappingURL=define-plugin.d.ts.map