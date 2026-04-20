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
import { startPluginRuntime } from "./runtime.js";
export function definePlugin(definition) {
    // Auto-start the runtime when this module is loaded as the main entry point.
    // In test environments, the caller can import the definition without starting the runtime.
    if (isMainModule()) {
        startPluginRuntime(definition);
    }
    return definition;
}
function isMainModule() {
    // Check if we're running as the main process entry point
    // vs being imported by a test harness
    return !process.env.HIVEMIND_PLUGIN_TEST_MODE;
}
//# sourceMappingURL=define-plugin.js.map