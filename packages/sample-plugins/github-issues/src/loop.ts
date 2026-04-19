/**
 * Background polling loop for GitHub Issues.
 *
 * Polls for new/updated issues and emits them as incoming messages
 * into the Hivemind connector pipeline.
 */

import type { PluginContext } from "@hivemind/plugin-sdk";
import { githubFetch, formatIssue } from "./api.js";

export async function pollForUpdates(ctx: PluginContext): Promise<void> {
  const { owner, repo, token, pollInterval, watchLabels } = ctx.config as any;
  const pollIntervalMs = ((pollInterval as number) ?? 120) * 1000;

  // Restore last sync timestamp from persistent store
  let lastChecked =
    (await ctx.store.get<string>("lastChecked")) ?? new Date().toISOString();

  await ctx.updateStatus({ state: "syncing", message: "Starting sync..." });

  while (!ctx.signal.aborted) {
    try {
      const issues = await githubFetch(`/repos/${owner}/${repo}/issues`, {
        token: token as string,
        params: {
          since: lastChecked,
          state: "all",
          sort: "updated",
          per_page: "50",
        },
      });

      const now = new Date().toISOString();

      // Filter out PRs
      const realIssues = issues.filter((i: any) => !i.pull_request);

      for (const issue of realIssues) {
        // Label filtering
        if (
          watchLabels &&
          Array.isArray(watchLabels) &&
          watchLabels.length > 0
        ) {
          const issueLabels = (issue.labels ?? []).map((l: any) => l.name);
          if (
            !watchLabels.some((wl: string) => issueLabels.includes(wl))
          ) {
            continue;
          }
        }

        const formatted = formatIssue(issue);

        await ctx.emitMessage({
          source: `github:issue:${owner}/${repo}#${issue.number}:${issue.updated_at}`,
          channel: `${owner}/${repo}`,
          content: [
            `[#${issue.number}] ${issue.title} (${issue.state})`,
            `Author: ${issue.user?.login} | Labels: ${(issue.labels ?? []).map((l: any) => l.name).join(", ") || "none"}`,
            "",
            issue.body?.substring(0, 500) ?? "(no description)",
          ].join("\n"),
          sender: {
            id: issue.user?.login ?? "unknown",
            name: issue.user?.login ?? "Unknown",
            avatarUrl: issue.user?.avatar_url,
          },
          metadata: {
            type: "github_issue",
            ...formatted,
          },
        });
      }

      // Persist cursor
      lastChecked = now;
      await ctx.store.set("lastChecked", lastChecked);

      await ctx.updateStatus({
        state: "connected",
        message: `Synced ${realIssues.length} issues at ${new Date().toLocaleTimeString()}`,
      });
    } catch (err) {
      ctx.logger.error("GitHub poll failed", { error: String(err) });
      await ctx.updateStatus({
        state: "error",
        message: `Sync failed: ${err instanceof Error ? err.message : String(err)}`,
      });
    }

    await ctx.sleep(pollIntervalMs);
  }

  await ctx.updateStatus({ state: "disconnected", message: "Loop stopped" });
}
