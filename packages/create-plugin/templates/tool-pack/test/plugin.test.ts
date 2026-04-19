import { describe, it, expect } from "vitest";
import { createTestHarness } from "@hivemind-os/plugin-sdk/testing";

process.env.HIVEMIND_PLUGIN_TEST_MODE = "1";
import myPlugin from "../src/index.js";

describe("{{name}}", () => {
  const harness = createTestHarness(myPlugin, {
    config: { apiKey: "test-key" },
  });

  it("should validate config", () => {
    expect(harness.validateConfig({ apiKey: "key" }).valid).toBe(true);
    expect(harness.validateConfig({}).valid).toBe(false);
  });

  it("hello should greet", async () => {
    const result = await harness.callTool("hello", { name: "World" });
    expect(result.content).toBe("Hello, World!");
  });
});
