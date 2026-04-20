/**
 * Tool: search_issues — Search issues across the configured repository using GitHub's search API.
 */

import { z, type ToolDefinition } from "@hivemind-os/plugin-sdk";
import { githubFetch, formatIssue } from "../api.js";

export const searchIssues: ToolDefinition = {
  name: "search_issues",
  description:
    "Search issues in the configured GitHub repository using a text query. " +
    "Supports GitHub search syntax (e.g., 'label:bug', 'is:open', 'author:user')",
  parameters: z.object({
    query: z.string().describe("Search query text"),
    sort: z
      .enum(["created", "updated", "comments"])
      .default("updated")
      .describe("Sort field"),
    order: z.enum(["asc", "desc"]).default("desc").describe("Sort order"),
    limit: z
      .number()
      .min(1)
      .max(100)
      .default(20)
      .describe("Maximum results"),
  }),
  execute: async (params, ctx) => {
    const { owner, repo, token } = ctx.config as any;

    // Scope search to the configured repo
    const scopedQuery = `${params.query} repo:${owner}/${repo} is:issue`;

    const result = await githubFetch("/search/issues", {
      token,
      params: {
        q: scopedQuery,
        sort: params.sort,
        order: params.order,
        per_page: String(params.limit),
      },
    });

    return {
      content: {
        totalCount: result.total_count,
        issues: result.items.map(formatIssue),
      },
    };
  },
};
