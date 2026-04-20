import { describe, it, expect } from "vitest";
import { createTestHarness } from "@hivemind-os/plugin-sdk/testing";

process.env.HIVEMIND_PLUGIN_TEST_MODE = "1";
import myPlugin from "../src/index.js";

describe("{{name}}", () => {
  const harness = createTestHarness(myPlugin, {
    config: {
      apiKey: "test-key",
      baseUrl: "https://test.example.com",
      pollInterval: 60,
    },
  });

  it("should have valid config schema", () => {
    const schema = harness.getConfigSchema();
    expect(schema.properties).toHaveProperty("apiKey");
    expect(schema.properties.apiKey.hivemind?.secret).toBe(true);
  });

  it("should validate config", () => {
    expect(harness.validateConfig({ apiKey: "key" }).valid).toBe(true);
    expect(harness.validateConfig({}).valid).toBe(false);
  });

  it("list_items should return results", async () => {
    const result = await harness.callTool("list_items", { limit: 10 });
    expect(result.content).toBeDefined();
  });

  it("get_item should return an item", async () => {
    const result = await harness.callTool("get_item", { id: "123" });
    const content = result.content as Record<string, unknown>;
    expect(content.id).toBe("123");
  });
});
