/**
 * Tool: list_issues — List issues from the configured GitHub repository.
 */

import { z, type ToolDefinition } from "@hivemind-os/plugin-sdk";
import { githubFetch, formatIssue } from "../api.js";

export const listIssues: ToolDefinition = {
  name: "list_issues",
  description:
    "List issues from the configured GitHub repository, with optional filters for state, labels, and assignee",
  parameters: z.object({
    state: z
      .enum(["open", "closed", "all"])
      .default("open")
      .describe("Issue state filter"),
    labels: z
      .string()
      .optional()
      .describe("Comma-separated label names to filter by"),
    assignee: z.string().optional().describe("Filter by assignee username"),
    sort: z
      .enum(["created", "updated", "comments"])
      .default("updated")
      .describe("Sort field"),
    direction: z.enum(["asc", "desc"]).default("desc").describe("Sort order"),
    limit: z
      .number()
      .min(1)
      .max(100)
      .default(20)
      .describe("Maximum number of issues to return"),
  }),
  execute: async (params, ctx) => {
    const { owner, repo, token } = ctx.config as any;

    const queryParams: Record<string, string> = {
      state: params.state,
      sort: params.sort,
      direction: params.direction,
      per_page: String(params.limit),
    };
    if (params.labels) queryParams.labels = params.labels;
    if (params.assignee) queryParams.assignee = params.assignee;

    const issues = await githubFetch(`/repos/${owner}/${repo}/issues`, {
      token,
      params: queryParams,
    });

    // Filter out pull requests (GitHub API includes them in /issues)
    const realIssues = issues.filter((i: any) => !i.pull_request);

    return {
      content: realIssues.map(formatIssue),
    };
  },
};
