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
export {};
//# sourceMappingURL=extract-schema.d.ts.map