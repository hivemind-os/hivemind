import { definePlugin, z } from "@hivemind-os/plugin-sdk";
import { listItems } from "./tools/list-items.js";
import { getItem } from "./tools/get-item.js";

export default definePlugin({
  configSchema: z.object({
    apiKey: z
      .string()
      .secret()
      .label("API Key")
      .helpText("Your API key")
      .section("Authentication"),
    baseUrl: z
      .string()
      .default("https://api.example.com")
      .label("API Base URL")
      .section("Connection"),
    pollInterval: z
      .number()
      .min(30)
      .max(3600)
      .default(120)
      .label("Poll Interval (seconds)")
      .section("Sync"),
  }),

  auth: {
    type: "token",
    fields: [
      {
        key: "apiKey",
        label: "API Key",
        helpText: "Find your API key in your account settings",
      },
    ],
  },

  tools: [listItems, getItem],

  loop: async (ctx) => {
    const pollIntervalMs = (ctx.config.pollInterval as number) * 1000;
    let cursor = await ctx.store.get<string>("lastSync");

    await ctx.updateStatus({ state: "syncing", message: "Starting..." });

    while (!ctx.signal.aborted) {
      try {
        // TODO: Replace with your actual API call
        ctx.logger.info("Polling for updates", { cursor });

        // Example: fetch updates and emit messages
        // const updates = await fetchUpdates(ctx.config, cursor);
        // for (const item of updates) {
        //   await ctx.emitMessage({
        //     source: `myservice:${item.id}:${item.updatedAt}`,
        //     channel: 'my-feed',
        //     content: item.title,
        //   });
        // }

        cursor = new Date().toISOString();
        await ctx.store.set("lastSync", cursor);
        await ctx.updateStatus({
          state: "connected",
          message: `Last sync: ${new Date().toLocaleTimeString()}`,
        });
      } catch (err) {
        ctx.logger.error("Poll failed", { error: String(err) });
        await ctx.updateStatus({ state: "error", message: String(err) });
      }

      await ctx.sleep(pollIntervalMs);
    }
  },

  onActivate: async (ctx) => {
    ctx.logger.info("Plugin activated");
    // TODO: Validate credentials here
    await ctx.updateStatus({ state: "connected", message: "Ready" });
  },

  onDeactivate: async (ctx) => {
    ctx.logger.info("Plugin deactivated");
    await ctx.updateStatus({ state: "disconnected" });
  },
});
