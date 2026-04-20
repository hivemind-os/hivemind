/**
 * @hivemind-os/test-plugin — E2E test plugin
 *
 * This plugin exercises every host API. It is used by the Rust integration
 * tests in crates/hive-plugins/tests/ to validate the full plugin protocol.
 *
 * Tools:
 *   echo              — basic tool execution
 *   test_secrets       — secret storage CRUD
 *   test_store         — persistent KV store CRUD
 *   emit_test_message  — push message into connector pipeline
 *   emit_test_event    — emit workflow-triggerable event
 *   update_status      — update plugin status in UI
 *   send_notification  — desktop notification
 *   test_filesystem    — plugin data directory operations
 *   get_host_info      — host environment info
 *   discover           — list connectors and personas
 *   test_http          — HTTP proxy round-trip
 *
 * Loop:
 *   Emits a message every `pollInterval` seconds with an incrementing tick
 *   counter. Persists the tick in the KV store for restart resilience testing.
 *
 * Lifecycle:
 *   onActivate  — validates config (can simulate failure via failOnActivate)
 *   onDeactivate — logs and updates status
 */

import { definePlugin, z } from "@hivemind-os/plugin-sdk";

export default definePlugin({
  configSchema: z.object({
    apiKey: z
      .string()
      .secret()
      .label("API Key")
      .helpText("Test API key (any value accepted)")
      .section("Authentication"),
    endpoint: z
      .string()
      .default("https://httpbin.org")
      .label("API Endpoint")
      .section("Connection"),
    pollInterval: z
      .number()
      .min(1)
      .max(3600)
      .default(30)
      .label("Poll Interval (seconds)")
      .section("Sync"),
    failOnActivate: z
      .boolean()
      .default(false)
      .label("Simulate activation failure")
      .helpText("When true, onActivate will throw an error")
      .section("Testing"),
  }),

  tools: [
    // ── 1. Echo — basic tool execution ──────────────────────
    {
      name: "echo",
      description: "Echoes back the input (for testing basic tool execution)",
      parameters: z.object({
        message: z.string().describe("Message to echo back"),
      }),
      execute: async (params, ctx) => {
        ctx.logger.info("Echo tool called", { message: params.message });
        return { content: `Echo: ${params.message}` };
      },
    },

    // ── 2. Secret Storage CRUD ──────────────────────────────
    {
      name: "test_secrets",
      description: "Tests secret storage round-trip (get/set/delete/has)",
      parameters: z.object({
        action: z.enum(["get", "set", "delete", "has"]),
        key: z.string(),
        value: z.string().optional(),
      }),
      execute: async (params, ctx) => {
        switch (params.action) {
          case "set":
            await ctx.secrets.set(params.key, params.value!);
            return { content: `Secret '${params.key}' stored` };
          case "get": {
            const val = await ctx.secrets.get(params.key);
            return { content: val ?? "(null)" };
          }
          case "delete":
            await ctx.secrets.delete(params.key);
            return { content: `Secret '${params.key}' deleted` };
          case "has": {
            const exists = await ctx.secrets.has(params.key);
            return { content: String(exists) };
          }
          default:
            return { content: "unknown action", isError: true };
        }
      },
    },

    // ── 3. Persistent KV Store CRUD ─────────────────────────
    {
      name: "test_store",
      description: "Tests persistent KV storage round-trip (get/set/delete/keys)",
      parameters: z.object({
        action: z.enum(["get", "set", "delete", "keys"]),
        key: z.string().default(""),
        value: z.string().optional(),
      }),
      execute: async (params, ctx) => {
        switch (params.action) {
          case "set":
            await ctx.store.set(params.key, params.value);
            return { content: "stored" };
          case "get": {
            const v = await ctx.store.get(params.key);
            return { content: JSON.stringify(v) };
          }
          case "delete":
            await ctx.store.delete(params.key);
            return { content: "deleted" };
          case "keys": {
            const keys = await ctx.store.keys();
            return { content: JSON.stringify(keys) };
          }
          default:
            return { content: "unknown action", isError: true };
        }
      },
    },

    // ── 4. Message Emission ─────────────────────────────────
    {
      name: "emit_test_message",
      description: "Emits a test message into the host connector pipeline",
      parameters: z.object({
        channel: z.string().describe("Channel name for routing"),
        content: z.string().describe("Message content"),
        threadId: z.string().optional().describe("Optional thread ID"),
        sourceId: z.string().optional().describe("Custom dedup source ID"),
      }),
      execute: async (params, ctx) => {
        await ctx.emitMessage({
          source: params.sourceId ?? `test:${Date.now()}:${Math.random()}`,
          channel: params.channel,
          content: params.content,
          threadId: params.threadId,
          sender: { id: "test-bot", name: "Test Bot" },
          metadata: { isTest: true, timestamp: new Date().toISOString() },
        });
        return { content: "Message emitted" };
      },
    },

    // ── 5. Event Emission ───────────────────────────────────
    {
      name: "emit_test_event",
      description: "Emits a test event for workflow triggers",
      parameters: z.object({
        eventType: z.string().describe("Event type (e.g. 'test.thing_happened')"),
        payload: z.record(z.unknown()).optional().describe("Event payload"),
      }),
      execute: async (params, ctx) => {
        await ctx.emitEvent(params.eventType, params.payload ?? {});
        return { content: `Event '${params.eventType}' emitted` };
      },
    },

    // ── 6. Status Updates ───────────────────────────────────
    {
      name: "update_status",
      description: "Updates the plugin status displayed in the Settings UI",
      parameters: z.object({
        state: z.enum([
          "connected",
          "connecting",
          "disconnected",
          "error",
          "syncing",
        ]),
        message: z.string().optional(),
        progress: z.number().min(0).max(100).optional(),
      }),
      execute: async (params, ctx) => {
        await ctx.updateStatus(params);
        return { content: `Status updated to ${params.state}` };
      },
    },

    // ── 7. Desktop Notifications ────────────────────────────
    {
      name: "send_notification",
      description: "Sends a test desktop notification",
      parameters: z.object({
        title: z.string(),
        body: z.string(),
      }),
      execute: async (params, ctx) => {
        await ctx.notify({ title: params.title, body: params.body });
        return { content: "Notification sent" };
      },
    },

    // ── 8. Data Directory Operations ────────────────────────
    {
      name: "test_filesystem",
      description: "Tests the plugin's private data directory operations",
      parameters: z.object({
        action: z.enum(["write", "read", "exists", "list", "resolve", "remove"]),
        path: z.string().describe("Path relative to plugin data dir"),
        content: z.string().optional().describe("Content for write operations"),
      }),
      execute: async (params, ctx) => {
        switch (params.action) {
          case "write":
            await ctx.dataDir.writeFile(params.path, params.content!);
            return { content: "written" };
          case "read": {
            const data = await ctx.dataDir.readFile(params.path);
            return { content: data };
          }
          case "exists": {
            const exists = await ctx.dataDir.exists(params.path);
            return { content: String(exists) };
          }
          case "list": {
            const files = await ctx.dataDir.readDir(params.path);
            return { content: JSON.stringify(files) };
          }
          case "resolve": {
            const abs = await ctx.dataDir.resolve(params.path);
            return { content: abs };
          }
          case "remove":
            await ctx.dataDir.remove(params.path);
            return { content: "removed" };
          default:
            return { content: "unknown action", isError: true };
        }
      },
    },

    // ── 9. Host Info ────────────────────────────────────────
    {
      name: "get_host_info",
      description: "Returns host environment information (version, platform, capabilities)",
      parameters: z.object({}),
      execute: async (_params, ctx) => {
        return {
          content: {
            version: ctx.host.version,
            platform: ctx.host.platform,
            capabilities: ctx.host.capabilities,
          },
        };
      },
    },

    // ── 10. Discovery ───────────────────────────────────────
    {
      name: "discover",
      description: "Lists connectors or personas from the host",
      parameters: z.object({
        what: z.enum(["connectors", "personas"]),
      }),
      execute: async (params, ctx) => {
        if (params.what === "connectors") {
          return { content: await ctx.connectors.list() };
        } else {
          return { content: await ctx.personas.list() };
        }
      },
    },

    // ── 11. HTTP Proxy ──────────────────────────────────────
    {
      name: "test_http",
      description: "Makes an HTTP request through the host proxy",
      parameters: z.object({
        url: z.string().describe("URL to request"),
        method: z.enum(["GET", "POST"]).default("GET"),
        body: z.string().optional(),
      }),
      execute: async (params, ctx) => {
        const res = await ctx.http.fetch(params.url, {
          method: params.method,
          body: params.body,
        });
        const body = await res.text();
        return {
          content: {
            status: res.status,
            bodyLength: body.length,
            bodyPreview: body.substring(0, 200),
          },
        };
      },
    },
  ],

  // ── Background Loop ─────────────────────────────────────────
  loop: async (ctx) => {
    let tick = 0;
    const pollInterval = ctx.config.pollInterval as number;
    await ctx.updateStatus({ state: "syncing", message: "Loop starting" });

    while (!ctx.signal.aborted) {
      tick++;
      await ctx.store.set("loopTick", tick);

      await ctx.emitMessage({
        source: `test:loop:${tick}`,
        channel: "test-loop",
        content: `Loop tick ${tick}`,
        metadata: { tick, timestamp: new Date().toISOString() },
      });

      await ctx.emitEvent("test.loop_tick", { tick });

      ctx.logger.info(`Loop tick ${tick}`);
      await ctx.updateStatus({
        state: "connected",
        message: `Tick ${tick}`,
      });

      await ctx.sleep(pollInterval * 1000);
    }

    await ctx.updateStatus({
      state: "disconnected",
      message: "Loop stopped",
    });
  },

  // ── Lifecycle Hooks ───────────────────────────────────────
  onActivate: async (ctx) => {
    if (ctx.config.failOnActivate as boolean) {
      throw new Error("Simulated activation failure (failOnActivate=true)");
    }
    ctx.logger.info("Test plugin activated", {
      endpoint: ctx.config.endpoint as string,
    });
    await ctx.updateStatus({ state: "connected", message: "Ready" });
  },

  onDeactivate: async (ctx) => {
    ctx.logger.info("Test plugin deactivated");
    await ctx.updateStatus({ state: "disconnected", message: "Stopped" });
  },
});
