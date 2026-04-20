/**
 * Helper for making authenticated GitHub API requests.
 */

export interface GitHubApiOptions {
  token: string;
  baseUrl?: string;
}

export async function githubFetch(
  path: string,
  opts: GitHubApiOptions & {
    method?: string;
    params?: Record<string, string>;
    body?: unknown;
  },
): Promise<any> {
  const base = opts.baseUrl ?? "https://api.github.com";
  const url = new URL(`${base}${path}`);

  if (opts.params) {
    for (const [key, value] of Object.entries(opts.params)) {
      if (value !== undefined && value !== "") {
        url.searchParams.set(key, value);
      }
    }
  }

  const res = await fetch(url.toString(), {
    method: opts.method ?? "GET",
    headers: {
      Authorization: `Bearer ${opts.token}`,
      Accept: "application/vnd.github.v3+json",
      "Content-Type": "application/json",
    },
    body: opts.body ? JSON.stringify(opts.body) : undefined,
  });

  if (!res.ok) {
    const errorBody = await res.text().catch(() => "");
    throw new Error(
      `GitHub API error: ${res.status} ${res.statusText}${errorBody ? ` — ${errorBody.substring(0, 200)}` : ""}`,
    );
  }

  return res.json();
}

export function formatIssue(issue: any): Record<string, unknown> {
  return {
    number: issue.number,
    title: issue.title,
    state: issue.state,
    author: issue.user?.login,
    labels: issue.labels?.map((l: any) => l.name) ?? [],
    assignees: issue.assignees?.map((a: any) => a.login) ?? [],
    created: issue.created_at,
    updated: issue.updated_at,
    comments: issue.comments,
    url: issue.html_url,
    body_preview: issue.body?.substring(0, 200) ?? "",
  };
}
