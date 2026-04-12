# Build a Security Auditor Persona

This recipe walks you through creating a production-ready **Security Auditor** persona from scratch — one that finds vulnerabilities, rates severity, and reports findings without touching your code.

## The Complete Persona

Open **Settings → Personas → New Persona** and paste this YAML (or fill in the form — they stay in sync):

```yaml
id: user/security-auditor
name: Security Auditor
description: Finds vulnerabilities and security issues in code
avatar: 🛡️
color: "#dc2626"
system_prompt: |
  You are a senior security engineer performing code audits.

  For every file you review:
  1. Check for injection vulnerabilities (SQL, XSS, command injection)
  2. Look for authentication/authorization bypasses
  3. Find hardcoded secrets or credentials
  4. Check for insecure cryptographic practices
  5. Identify OWASP Top 10 vulnerabilities

  Always provide:
  - Severity rating (Critical/High/Medium/Low)
  - Exact code location
  - Recommended fix with code example

  Never modify code directly — only report findings.
preferred_models:
  - claude-sonnet
  - gpt-4o
secondary_models:
  - claude-haiku-*
loop_strategy: plan_then_execute
allowed_tools:
  - filesystem.read
  - filesystem.search
  - filesystem.glob
  - http.request
prompts:
  - id: audit-directory
    name: Audit Directory
    description: Run a security audit on a directory
    template: |
      Perform a thorough security audit of all files in {{directory}}.
      Focus on: {{focus_areas}}
      Report findings grouped by severity.
    input_schema:
      type: object
      properties:
        directory:
          type: string
          description: Path to the directory to audit
        focus_areas:
          type: string
          description: Comma-separated areas to focus on
          default: "injection, auth, secrets, crypto"
      required: [directory]
```

## Why These Choices?

| Setting | Rationale |
|---|---|
| **`plan_then_execute`** | The auditor plans which files to check before diving in — systematic and thorough |
| **Read-only tools** | Security reviewers should never modify code. `filesystem.read`, `filesystem.search`, `filesystem.glob` let it explore without risk |
| **`http.request`** | Lets the auditor check CVE databases and security advisories for context |
| **`claude-sonnet` primary** | Strong reasoning for nuanced vulnerability analysis |

::: tip
Restricting tools to read-only operations is critical for auditor personas. You don't want a security scanner accidentally "fixing" issues by rewriting your auth layer.
:::

## Using the Auditor

### In Chat

Select **Security Auditor** from the persona picker and type:

```
Audit the src/auth directory for security issues
```

### Via the Prompt Template

Click the **⚡ Prompts** button, select **Audit Directory**, and fill in:

- **Directory:** `src/auth`
- **Focus areas:** `injection, auth bypasses, hardcoded secrets`

### Expected Output

The auditor produces a structured report like this:

```markdown
# Security Audit: src/auth

## 🔴 Critical

### SQL Injection in login handler
- **File:** src/auth/login.rs:47
- **Issue:** User input interpolated directly into SQL query
- **Fix:**
  ​```rust
  // Before (vulnerable)
  let query = format!("SELECT * FROM users WHERE email = '{}'", email);
  // After (parameterized)
  let user = sqlx::query("SELECT * FROM users WHERE email = $1")
      .bind(&email)
      .fetch_optional(&pool).await?;
  ​```

## 🟠 High

### Hardcoded JWT secret
- **File:** src/auth/tokens.rs:12
- **Issue:** JWT signing key is a string literal in source code
- **Fix:** Move to environment variable or secrets manager

## 🟡 Medium

### Missing rate limiting on login endpoint
- **File:** src/auth/routes.rs:23
- **Issue:** No rate limiting on POST /login — vulnerable to brute force
- **Fix:** Add rate-limiting middleware (e.g., 5 attempts per minute per IP)

---
**Summary:** 1 Critical · 1 High · 1 Medium · 0 Low
```

## Taking It Further

- **Wrap it in a bot** — launch a one-shot bot with this persona to audit on demand from the Bots dashboard
- **Add it to a workflow** — use `invoke_agent` with `persona_id: user/security-auditor` in a PR review pipeline (see [PR Review Workflow](/examples/pr-review-workflow))
- **Combine with a developer** — have a developer persona fix the issues the auditor finds, then re-audit in a feedback loop

## Related

- [Personas Guide](/guides/personas) — Creating and managing personas in depth
- [Agentic Loops Guide](/guides/agentic-loops) — How `plan_then_execute` works
- [PR Review Workflow](/examples/pr-review-workflow) — Use this persona in an automated pipeline
