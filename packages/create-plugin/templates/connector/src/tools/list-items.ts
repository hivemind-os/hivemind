import { z, type ToolDefinition } from "@hivemind-os/plugin-sdk";

export const listItems: ToolDefinition = {
  name: "list_items",
  description: "List items from the API",
  parameters: z.object({
    limit: z.number().min(1).max(100).default(20).describe("Max results"),
    status: z.enum(["active", "archived", "all"]).default("active"),
  }),
  execute: async (params, ctx) => {
    // TODO: Replace with your actual API call
    ctx.logger.info("Listing items", { limit: params.limit, status: params.status });
    return {
      content: [
        { id: "1", title: "Example item", status: params.status },
      ],
    };
  },
};
