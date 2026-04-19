/**
 * Test harness for Hivemind plugins.
 *
 * Provides a simulated host environment for unit-testing plugins
 * without running the full Hivemind host.
 *
 * Usage:
 * ```typescript
 * import { createTestHarness } from '@hivemind-os/plugin-sdk/testing';
 * import myPlugin from '../src/index';
 *
 * const harness = createTestHarness(myPlugin, {
 *   config: { apiKey: 'test-key' },
 * });
 *
 * const result = await harness.callTool('my_tool', { param: 'value' });
 * ```
 */
import type { PluginDefinition, ToolResult, IncomingMessage, PluginStatus, NotificationPayload } from "./types.js";
import type { ZodRawShape } from "zod";
import { type SerializedConfigSchema } from "./schema.js";
export interface TestHarnessOptions {
    /** Plugin config values. */
    config?: Record<string, unknown>;
    /** Pre-populated secrets. */
    secrets?: Record<string, string>;
    /** Simulated host info. */
    hostInfo?: {
        version?: string;
        platform?: "windows" | "macos" | "linux";
        capabilities?: string[];
    };
}
export interface TestHarness {
    /** Call a tool by name with the given arguments. */
    callTool(name: string, args?: Record<string, unknown>): Promise<ToolResult>;
    /** Run the loop once (starts it, waits for the first emit, then stops). */
    runLoopUntil(opts?: {
        messageCount?: number;
        timeoutMs?: number;
    }): Promise<void>;
    /** Trigger onActivate. */
    activate(): Promise<void>;
    /** Trigger onDeactivate. */
    deactivate(): Promise<void>;
    /** Validate a config object against the plugin's schema. */
    validateConfig(config: unknown): {
        valid: boolean;
        errors?: Array<{
            path: string;
            message: string;
        }>;
    };
    /** Get the serialized config schema (as JSON). */
    getConfigSchema(): SerializedConfigSchema;
    /** Get all emitted messages. */
    readonly messages: IncomingMessage[];
    /** Get all emitted events. */
    readonly events: Array<{
        type: string;
        payload: Record<string, unknown>;
    }>;
    /** Get all emitted notifications. */
    readonly notifications: NotificationPayload[];
    /** Get all status updates. */
    readonly statuses: PluginStatus[];
    /** Get the in-memory secret store. */
    readonly secretStore: Record<string, string>;
    /** Get the in-memory KV store. */
    readonly kvStore: Record<string, unknown>;
    /** Get log entries. */
    readonly logs: Array<{
        level: string;
        msg: string;
        data?: Record<string, unknown>;
    }>;
    /** Reset all captured data (messages, events, logs, etc.). */
    reset(): void;
}
export declare function createTestHarness<TShape extends ZodRawShape>(definition: PluginDefinition<TShape>, options?: TestHarnessOptions): TestHarness;
//# sourceMappingURL=testing.d.ts.map