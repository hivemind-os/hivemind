/**
 * Unit tests for the GitHub Issues connector plugin.
 *
 * These tests use the SDK test harness to verify tool behavior
 * without making real GitHub API calls.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { createTestHarness } from "@hivemind/plugin-sdk/testing";

process.env.HIVEMIND_PLUGIN_TEST_MODE = "1";
import pluginDef from "../src/index.js";

describe("GitHub Issues Plugin", () => {
  let harness: ReturnType<typeof createTestHarness>;

  beforeEach(() => {
    harness = createTestHarness(pluginDef, {
      config: {
        token: "ghp_test_token_123",
        owner: "hivemind-os",
        repo: "hivemind",
        pollInterval: 120,
        watchLabels: [],
      },
    });
  });

  // ── Config Schema ─────────────────────────────────────────

  describe("Config Schema", () => {
    it("should have all required fields", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties).toHaveProperty("token");
      expect(schema.properties).toHaveProperty("owner");
      expect(schema.properties).toHaveProperty("repo");
      expect(schema.properties).toHaveProperty("pollInterval");
      expect(schema.properties).toHaveProperty("watchLabels");
    });

    it("should mark token as secret", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties.token.hivemind?.secret).toBe(true);
    });

    it("should have correct sections", () => {
      const schema = harness.getConfigSchema();
      expect(schema.properties.token.hivemind?.section).toBe("Authentication");
      expect(schema.properties.owner.hivemind?.section).toBe("Repository");
      expect(schema.properties.repo.hivemind?.section).toBe("Repository");
      expect(schema.properties.pollInterval.hivemind?.section).toBe(
        "Background Sync",
      );
    });

    it("should validate valid config", () => {
      const result = harness.validateConfig({
        token: "ghp_abc",
        owner: "user",
        repo: "myrepo",
      });
      expect(result.valid).toBe(true);
    });

    it("should reject missing required fields", () => {
      const result = harness.validateConfig({});
      expect(result.valid).toBe(false);
      expect(result.errors!.length).toBeGreaterThanOrEqual(3); // token, owner, repo
    });

    it("should reject invalid poll interval", () => {
      const result = harness.validateConfig({
        token: "key",
        owner: "user",
        repo: "repo",
        pollInterval: 5, // min is 30
      });
      expect(result.valid).toBe(false);
    });
  });

  // ── Tool Discovery ────────────────────────────────────────

  describe("Tool Discovery", () => {
    it("should have all four tools defined", () => {
      const tools = pluginDef.tools;
      expect(tools).toHaveLength(4);

      const names = tools.map((t) => t.name);
      expect(names).toContain("list_issues");
      expect(names).toContain("create_issue");
      expect(names).toContain("add_comment");
      expect(names).toContain("search_issues");
    });

    it("create_issue should require approval", () => {
      const tool = pluginDef.tools.find((t) => t.name === "create_issue");
      expect(tool?.annotations?.sideEffects).toBe(true);
      expect(tool?.annotations?.approval).toBe("suggest");
    });

    it("add_comment should require approval", () => {
      const tool = pluginDef.tools.find((t) => t.name === "add_comment");
      expect(tool?.annotations?.sideEffects).toBe(true);
      expect(tool?.annotations?.approval).toBe("suggest");
    });

    it("list_issues should NOT require approval", () => {
      const tool = pluginDef.tools.find((t) => t.name === "list_issues");
      expect(tool?.annotations?.sideEffects).toBeUndefined();
    });
  });

  // ── Plugin Capabilities ───────────────────────────────────

  describe("Plugin Capabilities", () => {
    it("should have a background loop defined", () => {
      expect(pluginDef.loop).toBeDefined();
      expect(typeof pluginDef.loop).toBe("function");
    });

    it("should have lifecycle hooks", () => {
      expect(pluginDef.onActivate).toBeDefined();
      expect(pluginDef.onDeactivate).toBeDefined();
    });

    it("should have auth config", () => {
      expect(pluginDef.auth).toBeDefined();
      expect(pluginDef.auth?.type).toBe("token");
    });
  });
});
