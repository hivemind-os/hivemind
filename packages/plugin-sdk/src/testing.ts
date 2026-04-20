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

import type {
  PluginDefinition,
  PluginContext,
  ToolResult,
  IncomingMessage,
  PluginStatus,
  NotificationPayload,
} from "./types.js";
import type { ZodRawShape } from "zod";
import { serializeConfigSchema, type SerializedConfigSchema } from "./schema.js";

// ─── Test Harness ───────────────────────────────────────────────────────────

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
    errors?: Array<{ path: string; message: string }>;
  };

  /** Get the serialized config schema (as JSON). */
  getConfigSchema(): SerializedConfigSchema;

  /** Get all emitted messages. */
  readonly messages: IncomingMessage[];

  /** Get all emitted events. */
  readonly events: Array<{ type: string; payload: Record<string, unknown> }>;

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

export function createTestHarness<TShape extends ZodRawShape>(
  definition: PluginDefinition<TShape>,
  options: TestHarnessOptions = {},
): TestHarness {
  // Internal state
  const messages: IncomingMessage[] = [];
  const events: Array<{
    type: string;
    payload: Record<string, unknown>;
  }> = [];
  const notifications: NotificationPayload[] = [];
  const statuses: PluginStatus[] = [];
  const logs: Array<{
    level: string;
    msg: string;
    data?: Record<string, unknown>;
  }> = [];
  const secretStore: Record<string, string> = {
    ...(options.secrets ?? {}),
  };
  const kvStore: Record<string, unknown> = {};
  const dataFiles: Record<string, string> = {};

  // Create a mock context
  function createMockContext(
    signal?: AbortSignal,
  ): PluginContext {
    const abortController = new AbortController();

    return {
      pluginId: "test-plugin",
      config: (options.config ?? {}) as any,
      signal: signal ?? abortController.signal,

      async emitMessage(msg) {
        messages.push(msg);
      },
      async emitMessages(msgs) {
        messages.push(...msgs);
      },

      secrets: {
        async get(key) {
          return secretStore[key] ?? null;
        },
        async set(key, value) {
          secretStore[key] = value;
        },
        async delete(key) {
          delete secretStore[key];
        },
        async has(key) {
          return key in secretStore;
        },
      },

      store: {
        async get<T = unknown>(key: string): Promise<T | null> {
          return (kvStore[key] as T) ?? null;
        },
        async set<T = unknown>(key: string, value: T): Promise<void> {
          kvStore[key] = value;
        },
        async delete(key: string): Promise<void> {
          delete kvStore[key];
        },
        async keys(): Promise<string[]> {
          return Object.keys(kvStore);
        },
      },

      logger: {
        debug(msg, data) {
          logs.push({ level: "debug", msg, data });
        },
        info(msg, data) {
          logs.push({ level: "info", msg, data });
        },
        warn(msg, data) {
          logs.push({ level: "warn", msg, data });
        },
        error(msg, data) {
          logs.push({ level: "error", msg, data });
        },
      },

      async notify(notification) {
        notifications.push(notification);
      },

      async emitEvent(eventType, payload) {
        events.push({ type: eventType, payload });
      },

      async updateStatus(status) {
        statuses.push(status);
      },

      sleep(ms) {
        return new Promise((resolve, reject) => {
          const sig = signal ?? abortController.signal;
          if (sig.aborted) {
            reject(new DOMException("Aborted", "AbortError"));
            return;
          }
          const timer = setTimeout(resolve, ms);
          const onAbort = () => {
            clearTimeout(timer);
            reject(new DOMException("Aborted", "AbortError"));
          };
          sig.addEventListener("abort", onAbort, { once: true });
        });
      },

      async schedule() {
        // No-op in test
      },
      async unschedule() {
        // No-op in test
      },

      http: {
        fetch: globalThis.fetch,
      },

      dataDir: {
        async resolve(path) {
          return `/mock-data-dir/${path}`;
        },
        async readFile(path) {
          if (!(path in dataFiles))
            throw new Error(`File not found: ${path}`);
          return dataFiles[path];
        },
        async writeFile(path, content) {
          dataFiles[path] =
            typeof content === "string"
              ? content
              : Buffer.from(content).toString("utf8");
        },
        async readDir(_path) {
          return Object.keys(dataFiles);
        },
        async exists(path) {
          return path in dataFiles;
        },
        async mkdir() {
          // No-op
        },
        async remove(path) {
          delete dataFiles[path];
        },
      },

      host: {
        version: options.hostInfo?.version ?? "0.0.0-test",
        platform: options.hostInfo?.platform ?? "linux",
        capabilities: options.hostInfo?.capabilities ?? ["test"],
      },

      connectors: {
        async list() {
          return [];
        },
      },
      personas: {
        async list() {
          return [];
        },
      },
    };
  }

  return {
    async callTool(name, args = {}) {
      const tool = definition.tools.find((t) => t.name === name);
      if (!tool) throw new Error(`Unknown tool: ${name}`);

      const parsed = tool.parameters.parse(args);
      const ctx = createMockContext();
      return tool.execute(parsed, ctx);
    },

    async runLoopUntil(opts = {}) {
      if (!definition.loop) throw new Error("Plugin has no loop defined");

      const { messageCount = 1, timeoutMs = 5000 } = opts;
      const abortController = new AbortController();
      const ctx = createMockContext(abortController.signal);

      const loopPromise = definition.loop(ctx).catch((err) => {
        if (err?.name !== "AbortError") throw err;
      });

      // Wait for messages or timeout
      const startCount = messages.length;
      const deadline = Date.now() + timeoutMs;

      while (messages.length - startCount < messageCount) {
        if (Date.now() > deadline) {
          abortController.abort();
          await loopPromise;
          throw new Error(
            `Timeout waiting for ${messageCount} messages (got ${messages.length - startCount})`,
          );
        }
        await new Promise((r) => setTimeout(r, 50));
      }

      abortController.abort();
      await loopPromise;
    },

    async activate() {
      if (definition.onActivate) {
        const ctx = createMockContext();
        await definition.onActivate(ctx);
      }
    },

    async deactivate() {
      if (definition.onDeactivate) {
        const ctx = createMockContext();
        await definition.onDeactivate(ctx);
      }
    },

    validateConfig(config) {
      const result = definition.configSchema.safeParse(config);
      if (result.success) {
        return { valid: true };
      }
      return {
        valid: false,
        errors: result.error.issues.map((issue) => ({
          path: issue.path.join("."),
          message: issue.message,
        })),
      };
    },

    getConfigSchema() {
      return serializeConfigSchema(definition.configSchema);
    },

    get messages() {
      return messages;
    },
    get events() {
      return events;
    },
    get notifications() {
      return notifications;
    },
    get statuses() {
      return statuses;
    },
    get secretStore() {
      return secretStore;
    },
    get kvStore() {
      return kvStore;
    },
    get logs() {
      return logs;
    },

    reset() {
      messages.length = 0;
      events.length = 0;
      notifications.length = 0;
      statuses.length = 0;
      logs.length = 0;
      for (const key of Object.keys(kvStore)) delete kvStore[key];
      for (const key of Object.keys(dataFiles)) delete dataFiles[key];
    },
  };
}
