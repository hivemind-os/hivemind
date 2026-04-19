#!/usr/bin/env node
/**
 * extract-schema — Extracts the config schema from a built plugin and writes
 * it to dist/config-schema.json. Run this after `tsc` during the build step.
 *
 * Usage:
 *   hivemind-extract-schema [--entry dist/index.js] [--out dist/config-schema.json]
 *
 * This script imports the plugin module with HIVEMIND_PLUGIN_TEST_MODE=1
 * (preventing the runtime from starting), reads the configSchema property,
 * serializes it, and writes the JSON file.
 */

import { pathToFileURL } from "node:url";
import { resolve, dirname, join } from "node:path";
import { writeFileSync, mkdirSync } from "node:fs";

// Prevent plugin runtime from starting
process.env.HIVEMIND_PLUGIN_TEST_MODE = "1";

async function main() {
  const args = process.argv.slice(2);
  let entry = "dist/index.js";
  let out = "dist/config-schema.json";

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--entry" && args[i + 1]) entry = args[++i];
    if (args[i] === "--out" && args[i + 1]) out = args[++i];
  }

  const entryPath = resolve(process.cwd(), entry);
  const outPath = resolve(process.cwd(), out);

  try {
    const mod = await import(pathToFileURL(entryPath).href);
    const definition = mod.default?.default ?? mod.default;

    if (!definition?.configSchema) {
      console.log("No configSchema found — writing empty schema.");
      const empty = { type: "object", properties: {}, required: [] };
      mkdirSync(dirname(outPath), { recursive: true });
      writeFileSync(outPath, JSON.stringify(empty, null, 2) + "\n");
      return;
    }

    // Import the SDK's serializeConfigSchema
    const { serializeConfigSchema } = await import(
      "@hivemind/plugin-sdk"
    );
    const schema = serializeConfigSchema(definition.configSchema);
    mkdirSync(dirname(outPath), { recursive: true });
    writeFileSync(outPath, JSON.stringify(schema, null, 2) + "\n");
    console.log(`Config schema written to ${out}`);
  } catch (err) {
    console.error("Failed to extract config schema:", err);
    process.exit(1);
  }
}

main();
