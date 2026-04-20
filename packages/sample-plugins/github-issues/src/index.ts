/**
 * @hivemind-os/connector-github-issues — Reference connector plugin
 *
 * This is a complete, working example of a Hivemind connector plugin.
 * It connects to a GitHub repository and provides:
 *
 * Tools:
 *   - list_issues: List issues with filtering and sorting
 *   - create_issue: Create new issues (requires approval)
 *   - add_comment: Comment on existing issues (requires approval)
 *   - search_issues: Full-text search using GitHub's search API
 *
 * Background Loop:
 *   - Polls for new/updated issues at a configurable interval
 *   - Emits issues as incoming messages to the Hivemind connector pipeline
 *   - Supports label-based filtering
 *   - Persists sync cursor for restart resilience
 *
 * Config:
 *   - Personal Access Token (stored in OS keyring)
 *   - Repository owner/name
 *   - Poll interval and label filters
 */

import { definePlugin, z } from "@hivemind-os/plugin-sdk";
import { listIssues } from "./tools/list-issues.js";
import { createIssue } from "./tools/create-issue.js";
import { addComment } from "./tools/add-comment.js";
import { searchIssues } from "./tools/search-issues.js";
import { pollForUpdates } from "./loop.js";

export default definePlugin({
  configSchema: z.object({
    // Auth
    token: z
      .string()
      .secret()
      .label("Personal Access Token")
      .helpText(
        'Create at github.com/settings/tokens. Needs "repo" scope for private repos, or "public_repo" for public.',
      )
      .section("Authentication"),

    // Repository
    owner: z
      .string()
      .label("Repository Owner")
      .placeholder("hivemind-os")
      .describe("GitHub username or organization")
      .section("Repository"),
    repo: z
      .string()
      .label("Repository Name")
      .placeholder("hivemind")
      .section("Repository"),

    // Polling
    pollInterval: z
      .number()
      .min(30)
      .max(3600)
      .default(120)
      .label("Poll Interval (seconds)")
      .helpText("How often to check for new/updated issues")
      .section("Background Sync"),
    watchLabels: z
      .array(z.string())
      .default([])
      .label("Watch Labels")
      .helpText(
        "Only sync issues with these labels. Leave empty to sync all issues.",
      )
      .section("Background Sync"),
  }),

  auth: {
    type: "token",
    fields: [
      {
        key: "token",
        label: "Personal Access Token",
        helpUrl: "https://github.com/settings/tokens/new?scopes=repo",
        helpText: 'Click to create a token with "repo" scope',
      },
    ],
  },

  tools: [listIssues, createIssue, addComment, searchIssues],

  loop: pollForUpdates,

  onActivate: async (ctx) => {
    const token = ctx.config.token as string;
    const owner = ctx.config.owner as string;
    const repo = ctx.config.repo as string;

    // Validate the token by hitting /user
    const res = await fetch("https://api.github.com/user", {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!res.ok) {
      throw new Error(
        `Invalid GitHub token (HTTP ${res.status}). Create one at https://github.com/settings/tokens`,
      );
    }
    const user = await res.json();
    ctx.logger.info(`Authenticated as ${user.login}`);

    // Verify repo access
    const repoRes = await fetch(
      `https://api.github.com/repos/${owner}/${repo}`,
      { headers: { Authorization: `Bearer ${token}` } },
    );
    if (!repoRes.ok) {
      throw new Error(
        `Cannot access repo ${owner}/${repo} (HTTP ${repoRes.status}). Check the token has "repo" scope.`,
      );
    }

    await ctx.updateStatus({
      state: "connected",
      message: `Connected as ${user.login} to ${owner}/${repo}`,
    });
  },

  onDeactivate: async (ctx) => {
    ctx.logger.info("GitHub Issues plugin deactivated");
    await ctx.updateStatus({ state: "disconnected" });
  },
});
