/**
 * Tool: add_comment — Add a comment to an existing GitHub issue.
 */

import { z, type ToolDefinition } from "@hivemind/plugin-sdk";
import { githubFetch } from "../api.js";

export const addComment: ToolDefinition = {
  name: "add_comment",
  description: "Add a comment to an existing GitHub issue by number",
  parameters: z.object({
    issue_number: z.number().describe("The issue number to comment on"),
    body: z.string().describe("Comment body (Markdown supported)"),
  }),
  annotations: {
    sideEffects: true,
    approval: "suggest",
  },
  execute: async (params, ctx) => {
    const { owner, repo, token } = ctx.config as any;

    const comment = await githubFetch(
      `/repos/${owner}/${repo}/issues/${params.issue_number}/comments`,
      {
        token,
        method: "POST",
        body: { body: params.body },
      },
    );

    ctx.logger.info("Comment added", {
      issueNumber: params.issue_number,
      commentId: comment.id,
    });

    return {
      content: {
        id: comment.id,
        issueNumber: params.issue_number,
        author: comment.user?.login,
        created: comment.created_at,
        url: comment.html_url,
        bodyPreview: params.body.substring(0, 100),
      },
    };
  },
};
