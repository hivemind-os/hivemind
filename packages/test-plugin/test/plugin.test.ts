/**
 * E2E-style unit tests for the test plugin using the SDK test harness.
 *
 * These tests verify that the test plugin correctly exercises every host API.
 * They run in-process (no JSON-RPC, no real host) using the mock context
 * from @hivemind-os/plugin-sdk/testing.
 *
 * The Rust integration tests (crates/hive-plugins/tests/) will test the
 * same plugin over the real JSON-RPC transport.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { createTestHarness } from "@hivemind-os/plugin-sdk/testing";
// Import the raw definition (HIVEMIND_PLUGIN_TEST_MODE prevents auto-start)
process.env.HIVEMIND_PLUGIN_TEST_MODE = "1";
import pluginDef from "../src/index.js";

describe("Test Plugin", () => {
  let harness: ReturnType<typeof createTestHarness>;

  beforeEach(() => {
    harness = createTestHarness(pluginDef, {
      config: {
        apiKey: "test-api-key-123",
        endpoint: "https://httpbin.org",
        pollInterval: 1,
        failOnActivate: false,
      },
      secrets: {
        existing_secret: "pre-populated-value",
      },
      hostInfo: {
        version: "0.1.26",
        platform: "windows",
        capabilities: ["tools", "loop", "lifecycle"],
      },
    });
  });

  // ── Config Schema ─────────────────────────────────────────

  describe("Config Schema", () => {
    it("should return a valid config schema", () => {
      const schema = harness.getConfigSchema();
      expect(schema.type).toBe("object");
      expect(schema.properties).toHaveProperty("apiKey");
      expect(schema.properties).toHaveProperty("endpoint");
      expect(schema.properties).toHaveProperty("pollInterval");
      expect(schema.properties).toHaveProperty("failOnActivate");
    });

    it("should mark apiKey as secret", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties.apiKey.hivemind?.secret).toBe(true);
    });

    it("should have correct defaults", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties.endpoint.default).toBe("https://httpbin.org");
      expect(schema.properties.pollInterval.default).toBe(30);
      expect(schema.properties.failOnActivate.default).toBe(false);
    });

    it("should have sections", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties.apiKey.hivemind?.section).toBe("Authentication");
      expect(schema.properties.endpoint.hivemind?.section).toBe("Connection");
      expect(schema.properties.pollInterval.hivemind?.section).toBe("Sync");
    });

    it("should validate valid config", () => {
      const result = harness.validateConfig({
        apiKey: "key",
        endpoint: "https://example.com",
        pollInterval: 60,
        failOnActivate: false,
      });
      expect(result.valid).toBe(true);
    });

    it("should reject config with missing required fields", () => {
      const result = harness.validateConfig({});
      expect(result.valid).toBe(false);
      expect(result.errors).toBeDefined();
      expect(result.errors!.length).toBeGreaterThan(0);
    });

    it("should reject config with invalid pollInterval", () => {
      const result = harness.validateConfig({
        apiKey: "key",
        pollInterval: -1,
      });
      expect(result.valid).toBe(false);
    });
  });

  // ── Lifecycle ─────────────────────────────────────────────

  describe("Lifecycle", () => {
    it("should activate successfully", async () => {
      await harness.activate();
      expect(harness.statuses.some((s) => s.state === "connected")).toBe(true);
      expect(harness.logs.some((l) => l.msg.includes("activated"))).toBe(true);
    });

    it("should fail activation when failOnActivate is true", async () => {
      const failHarness = createTestHarness(pluginDef, {
        config: {
          apiKey: "key",
          endpoint: "https://httpbin.org",
          pollInterval: 5,
          failOnActivate: true,
        },
      });
      await expect(failHarness.activate()).rejects.toThrow(
        "Simulated activation failure",
      );
    });

    it("should deactivate successfully", async () => {
      await harness.deactivate();
      expect(harness.statuses.some((s) => s.state === "disconnected")).toBe(
        true,
      );
      expect(harness.logs.some((l) => l.msg.includes("deactivated"))).toBe(
        true,
      );
    });
  });

  // ── Tool: echo ────────────────────────────────────────────

  describe("Tool: echo", () => {
    it("should echo back the message", async () => {
      const result = await harness.callTool("echo", {
        message: "Hello, World!",
      });
      expect(result.content).toBe("Echo: Hello, World!");
    });

    it("should log the call", async () => {
      await harness.callTool("echo", { message: "test" });
      expect(
        harness.logs.some(
          (l) => l.level === "info" && l.msg === "Echo tool called",
        ),
      ).toBe(true);
    });
  });

  // ── Tool: test_secrets ────────────────────────────────────

  describe("Tool: test_secrets", () => {
    it("should set and get a secret", async () => {
      await harness.callTool("test_secrets", {
        action: "set",
        key: "mykey",
        value: "myvalue",
      });
      const result = await harness.callTool("test_secrets", {
        action: "get",
        key: "mykey",
      });
      expect(result.content).toBe("myvalue");
    });

    it("should read pre-populated secrets", async () => {
      const result = await harness.callTool("test_secrets", {
        action: "get",
        key: "existing_secret",
      });
      expect(result.content).toBe("pre-populated-value");
    });

    it("should check if secret exists", async () => {
      const result = await harness.callTool("test_secrets", {
        action: "has",
        key: "existing_secret",
      });
      expect(result.content).toBe("true");
    });

    it("should return (null) for missing secrets", async () => {
      const result = await harness.callTool("test_secrets", {
        action: "get",
        key: "nonexistent",
      });
      expect(result.content).toBe("(null)");
    });

    it("should delete a secret", async () => {
      await harness.callTool("test_secrets", {
        action: "set",
        key: "temp",
        value: "val",
      });
      await harness.callTool("test_secrets", {
        action: "delete",
        key: "temp",
      });
      const result = await harness.callTool("test_secrets", {
        action: "has",
        key: "temp",
      });
      expect(result.content).toBe("false");
    });
  });

  // ── Tool: test_store ──────────────────────────────────────

  describe("Tool: test_store", () => {
    it("should set and get a value", async () => {
      await harness.callTool("test_store", {
        action: "set",
        key: "cursor",
        value: "abc123",
      });
      const result = await harness.callTool("test_store", {
        action: "get",
        key: "cursor",
      });
      expect(result.content).toBe('"abc123"');
    });

    it("should list keys", async () => {
      await harness.callTool("test_store", {
        action: "set",
        key: "key1",
        value: "v1",
      });
      await harness.callTool("test_store", {
        action: "set",
        key: "key2",
        value: "v2",
      });
      const result = await harness.callTool("test_store", {
        action: "keys",
      });
      const keys = JSON.parse(result.content as string);
      expect(keys).toContain("key1");
      expect(keys).toContain("key2");
    });

    it("should delete a value", async () => {
      await harness.callTool("test_store", {
        action: "set",
        key: "temp",
        value: "val",
      });
      await harness.callTool("test_store", {
        action: "delete",
        key: "temp",
      });
      const result = await harness.callTool("test_store", {
        action: "get",
        key: "temp",
      });
      expect(result.content).toBe("null");
    });
  });

  // ── Tool: emit_test_message ───────────────────────────────

  describe("Tool: emit_test_message", () => {
    it("should emit a message", async () => {
      await harness.callTool("emit_test_message", {
        channel: "test-channel",
        content: "Hello from test",
      });
      expect(harness.messages).toHaveLength(1);
      expect(harness.messages[0].channel).toBe("test-channel");
      expect(harness.messages[0].content).toBe("Hello from test");
      expect(harness.messages[0].sender?.name).toBe("Test Bot");
      expect(harness.messages[0].metadata?.isTest).toBe(true);
    });

    it("should include threadId when provided", async () => {
      await harness.callTool("emit_test_message", {
        channel: "ch",
        content: "threaded",
        threadId: "thread-42",
      });
      expect(harness.messages[0].threadId).toBe("thread-42");
    });

    it("should use custom sourceId for dedup", async () => {
      await harness.callTool("emit_test_message", {
        channel: "ch",
        content: "msg1",
        sourceId: "dedup-key-1",
      });
      expect(harness.messages[0].source).toBe("dedup-key-1");
    });
  });

  // ── Tool: emit_test_event ─────────────────────────────────

  describe("Tool: emit_test_event", () => {
    it("should emit an event", async () => {
      await harness.callTool("emit_test_event", {
        eventType: "test.thing_happened",
        payload: { id: "123", action: "created" },
      });
      expect(harness.events).toHaveLength(1);
      expect(harness.events[0].type).toBe("test.thing_happened");
      expect(harness.events[0].payload).toEqual({
        id: "123",
        action: "created",
      });
    });

    it("should emit event with empty payload", async () => {
      await harness.callTool("emit_test_event", {
        eventType: "test.simple",
      });
      expect(harness.events[0].payload).toEqual({});
    });
  });

  // ── Tool: update_status ───────────────────────────────────

  describe("Tool: update_status", () => {
    it("should update status with state and message", async () => {
      await harness.callTool("update_status", {
        state: "syncing",
        message: "Working...",
        progress: 50,
      });
      expect(harness.statuses).toHaveLength(1);
      expect(harness.statuses[0]).toEqual({
        state: "syncing",
        message: "Working...",
        progress: 50,
      });
    });

    it("should update status with state only", async () => {
      await harness.callTool("update_status", { state: "error" });
      expect(harness.statuses[0].state).toBe("error");
    });
  });

  // ── Tool: send_notification ───────────────────────────────

  describe("Tool: send_notification", () => {
    it("should send a notification", async () => {
      await harness.callTool("send_notification", {
        title: "Alert",
        body: "Something happened",
      });
      expect(harness.notifications).toHaveLength(1);
      expect(harness.notifications[0]).toEqual({
        title: "Alert",
        body: "Something happened",
      });
    });
  });

  // ── Tool: test_filesystem ─────────────────────────────────

  describe("Tool: test_filesystem", () => {
    it("should write and read a file", async () => {
      await harness.callTool("test_filesystem", {
        action: "write",
        path: "data.json",
        content: '{"key":"value"}',
      });
      const result = await harness.callTool("test_filesystem", {
        action: "read",
        path: "data.json",
      });
      expect(result.content).toBe('{"key":"value"}');
    });

    it("should check file existence", async () => {
      await harness.callTool("test_filesystem", {
        action: "write",
        path: "test.txt",
        content: "hello",
      });
      const exists = await harness.callTool("test_filesystem", {
        action: "exists",
        path: "test.txt",
      });
      expect(exists.content).toBe("true");

      const notExists = await harness.callTool("test_filesystem", {
        action: "exists",
        path: "nope.txt",
      });
      expect(notExists.content).toBe("false");
    });

    it("should resolve paths", async () => {
      const result = await harness.callTool("test_filesystem", {
        action: "resolve",
        path: "subdir/file.txt",
      });
      expect(result.content).toContain("subdir/file.txt");
    });

    it("should remove a file", async () => {
      await harness.callTool("test_filesystem", {
        action: "write",
        path: "temp.txt",
        content: "temp",
      });
      await harness.callTool("test_filesystem", {
        action: "remove",
        path: "temp.txt",
      });
      const exists = await harness.callTool("test_filesystem", {
        action: "exists",
        path: "temp.txt",
      });
      expect(exists.content).toBe("false");
    });
  });

  // ── Tool: get_host_info ───────────────────────────────────

  describe("Tool: get_host_info", () => {
    it("should return host information", async () => {
      const result = await harness.callTool("get_host_info", {});
      const info = result.content as Record<string, unknown>;
      expect(info.version).toBe("0.1.26");
      expect(info.platform).toBe("windows");
      expect(info.capabilities).toEqual(["tools", "loop", "lifecycle"]);
    });
  });

  // ── Tool: discover ────────────────────────────────────────

  describe("Tool: discover", () => {
    it("should list connectors (empty in test)", async () => {
      const result = await harness.callTool("discover", {
        what: "connectors",
      });
      expect(Array.isArray(result.content)).toBe(true);
    });

    it("should list personas (empty in test)", async () => {
      const result = await harness.callTool("discover", {
        what: "personas",
      });
      expect(Array.isArray(result.content)).toBe(true);
    });
  });

  // ── Tool: test_http ───────────────────────────────────────

  describe("Tool: test_http", () => {
    // Note: In the test harness, ctx.http.fetch uses global fetch.
    // These tests verify the tool's parameter handling; the Rust E2E
    // tests will verify the host proxy.
    it("should make an HTTP request", async () => {
      const result = await harness.callTool("test_http", {
        url: "https://httpbin.org/get",
        method: "GET",
      });
      const content = result.content as Record<string, unknown>;
      expect(content.status).toBe(200);
      expect(typeof content.bodyLength).toBe("number");
    });
  });

  // ── Background Loop ───────────────────────────────────────

  describe("Background Loop", () => {
    it("should emit messages on each tick", async () => {
      await harness.runLoopUntil({ messageCount: 3, timeoutMs: 10000 });

      expect(harness.messages.length).toBeGreaterThanOrEqual(3);
      expect(harness.messages[0].channel).toBe("test-loop");
      expect(harness.messages[0].content).toBe("Loop tick 1");
      expect(harness.messages[1].content).toBe("Loop tick 2");
      expect(harness.messages[2].content).toBe("Loop tick 3");
    });

    it("should emit events on each tick", async () => {
      await harness.runLoopUntil({ messageCount: 2, timeoutMs: 10000 });

      expect(harness.events.length).toBeGreaterThanOrEqual(2);
      expect(harness.events[0].type).toBe("test.loop_tick");
      expect(harness.events[0].payload.tick).toBe(1);
    });

    it("should persist tick counter in store", async () => {
      await harness.runLoopUntil({ messageCount: 2, timeoutMs: 10000 });

      const tick = harness.kvStore.loopTick;
      expect(tick).toBeGreaterThanOrEqual(2);
    });

    it("should update status on each tick", async () => {
      await harness.runLoopUntil({ messageCount: 1, timeoutMs: 10000 });

      // Should have "syncing" (loop start) and "connected" (tick update)
      expect(harness.statuses.some((s) => s.state === "syncing")).toBe(true);
      expect(harness.statuses.some((s) => s.state === "connected")).toBe(true);
    });
  });

  // ── Error Handling ────────────────────────────────────────

  describe("Error Handling", () => {
    it("should throw on unknown tool", async () => {
      await expect(
        harness.callTool("nonexistent_tool", {}),
      ).rejects.toThrow("Unknown tool: nonexistent_tool");
    });

    it("should throw on invalid tool params", async () => {
      await expect(harness.callTool("echo", {})).rejects.toThrow();
    });
  });

  // ── Reset ─────────────────────────────────────────────────

  describe("Harness Reset", () => {
    it("should clear all captured data", async () => {
      await harness.callTool("echo", { message: "test" });
      await harness.callTool("emit_test_message", {
        channel: "ch",
        content: "msg",
      });
      await harness.callTool("emit_test_event", {
        eventType: "test",
      });

      expect(harness.messages.length).toBeGreaterThan(0);
      expect(harness.events.length).toBeGreaterThan(0);
      expect(harness.logs.length).toBeGreaterThan(0);

      harness.reset();

      expect(harness.messages).toHaveLength(0);
      expect(harness.events).toHaveLength(0);
      expect(harness.logs).toHaveLength(0);
    });
  });
});
