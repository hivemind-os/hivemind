import { z, type ToolDefinition } from "@hivemind-os/plugin-sdk";

export const getItem: ToolDefinition = {
  name: "get_item",
  description: "Get a single item by ID",
  parameters: z.object({
    id: z.string().describe("Item ID"),
  }),
  execute: async (params, ctx) => {
    // TODO: Replace with your actual API call
    ctx.logger.info("Getting item", { id: params.id });
    return {
      content: { id: params.id, title: "Example item", status: "active" },
    };
  },
};
