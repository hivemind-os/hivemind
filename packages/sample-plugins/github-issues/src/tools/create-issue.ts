/**
 * Tool: create_issue — Create a new issue in the configured GitHub repository.
 */

import { z, type ToolDefinition } from "@hivemind/plugin-sdk";
import { githubFetch, formatIssue } from "../api.js";

export const createIssue: ToolDefinition = {
  name: "create_issue",
  description:
    "Create a new issue in the configured GitHub repository",
  parameters: z.object({
    title: z.string().describe("Issue title"),
    body: z.string().optional().describe("Issue body (Markdown supported)"),
    labels: z
      .array(z.string())
      .optional()
      .describe("Labels to assign to the issue"),
    assignees: z
      .array(z.string())
      .optional()
      .describe("GitHub usernames to assign"),
  }),
  annotations: {
    sideEffects: true,
    approval: "suggest",
  },
  execute: async (params, ctx) => {
    const { owner, repo, token } = ctx.config as any;

    const issue = await githubFetch(`/repos/${owner}/${repo}/issues`, {
      token,
      method: "POST",
      body: {
        title: params.title,
        body: params.body,
        labels: params.labels,
        assignees: params.assignees,
      },
    });

    ctx.logger.info("Issue created", {
      number: issue.number,
      title: issue.title,
    });

    await ctx.emitEvent("github.issue_created", {
      number: issue.number,
      title: issue.title,
      url: issue.html_url,
    });

    return {
      content: formatIssue(issue),
    };
  },
};
