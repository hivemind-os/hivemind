import { definePlugin, z } from "@hivemind/plugin-sdk";

export default definePlugin({
  configSchema: z.object({
    apiKey: z
      .string()
      .secret()
      .label("API Key")
      .section("Authentication"),
  }),

  tools: [
    {
      name: "hello",
      description: "A simple hello world tool",
      parameters: z.object({
        name: z.string().describe("Name to greet"),
      }),
      execute: async (params, ctx) => {
        ctx.logger.info(`Hello ${params.name}`);
        return { content: `Hello, ${params.name}!` };
      },
    },
  ],
});
