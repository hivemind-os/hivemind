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
import { z } from "@hivemind-os/plugin-sdk";
declare const _default: import("@hivemind-os/plugin-sdk").PluginDefinition<{
    apiKey: z.ZodString;
    endpoint: z.ZodDefault<z.ZodString>;
    pollInterval: z.ZodDefault<z.ZodNumber>;
    failOnActivate: z.ZodDefault<z.ZodBoolean>;
}>;
export default _default;
