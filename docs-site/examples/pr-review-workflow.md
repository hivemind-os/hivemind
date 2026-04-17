# Automated Email Support Agent

This recipe builds a **background workflow** that automatically responds to incoming customer emails — using a support agent persona with access to your product manual via workflow attachments.

## The Pipeline at a Glance

```mermaid
flowchart LR
    A["📨 Email arrives"] --> B["🏷️ Classify intent"]
    B --> C["✍️ Draft response\n(with product manual)"]
    C --> D["📤 Send reply"]

    style A fill:#0891b2,color:#fff
    style D fill:#16a34a,color:#fff
```

Four steps, one smart persona, instant responses. Every customer email gets a knowledgeable, consistent reply — day or night.

## Prerequisites

1. **An email connector** configured in HiveMind OS (e.g. IMAP/SMTP, Gmail, or Microsoft 365)
2. **A product manual** PDF or Markdown file uploaded as a workflow attachment
3. **A support persona** (we'll create one below)

### The Support Agent Persona

```yaml
id: user/support-agent
name: Support Agent
description: Friendly customer support specialist with deep product knowledge
avatar: 💬
color: "#0891b2"
system_prompt: |
  You are a friendly, knowledgeable customer support agent.

  Rules:
  1. Always consult the product manual before answering
  2. Be warm and empathetic — acknowledge the customer's issue first
  3. Give clear, step-by-step instructions when troubleshooting
  4. If you're unsure about something, say so honestly
  5. End every response with an offer to help further
  6. Never make up features or capabilities not in the manual

  Respond in the same language the customer used.
preferred_models:
  - claude-sonnet
loop_strategy: react
allowed_tools:
  - filesystem.read
  - http.request
  - knowledge.query
```

## The Full Workflow Definition

Open the workflow definitions view (⚙ gear icon next to **Workflows**), click **New Workflow**, switch to the YAML editor, and paste:

```yaml
name: user/email-support-responder
description: Auto-respond to customer emails using product knowledge
mode: background

# ── Workflow-level attachments ──────────────────────────────
# Upload your product manual in the Workflow Designer UI.
# These files are stored locally and made available to agent steps.
attachments:
  - id: product-manual
    filename: product-manual.pdf
    description: >
      The complete product manual. Use this as the primary reference
      when answering customer questions. Cite specific sections where
      relevant.
  - id: faq-doc
    filename: faq.md
    description: >
      Frequently asked questions and their approved answers.
      Prefer these answers when a question matches.

steps:
  # ── Trigger: fire on incoming email ───────────────────────
  - id: trigger
    type: trigger
    trigger:
      type: incoming_message
      channel_id: email-support
      ignore_replies: true
      subject_filter: null
      from_filter: null

  # ── Step 1: Classify the email intent ─────────────────────
  - id: classify
    type: task
    task:
      kind: invoke_agent
      persona_id: user/support-agent
      task: |
        Classify this customer email into one of these categories:
        - product_question (how to do something)
        - bug_report (something isn't working)
        - feature_request (they want something new)
        - billing (account/payment related)
        - other

        Subject: {{trigger.subject}}
        Body: {{trigger.body}}

        Respond with ONLY the category name.
    outputs:
      category: "{{result}}"

  # ── Step 2: Draft a response using the product manual ─────
  - id: draft_response
    type: task
    task:
      kind: invoke_agent
      persona_id: user/support-agent
      task: |
        A customer sent this email. Draft a helpful response.

        From: {{trigger.from}}
        Subject: {{trigger.subject}}
        Body:
        {{trigger.body}}

        Category: {{steps.classify.outputs.category}}

        Use the attached product manual and FAQ as your primary
        reference. Cite specific sections when relevant.
      timeout_secs: 120
      agent_name: "Support Agent"
      attachments:
        - product-manual
        - faq-doc
    outputs:
      reply: "{{result}}"

  # ── Step 3: Send the reply ────────────────────────────────
  - id: send_reply
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        channel_id: email-support
        to: "{{trigger.from}}"
        subject: "Re: {{trigger.subject}}"
        body: "{{steps.draft_response.outputs.reply}}"
    on_error:
      strategy: retry
      max_retries: 3
      delay_secs: 10

output:
  category: "{{steps.classify.outputs.category}}"
  response: "{{steps.draft_response.outputs.reply}}"
```

## How It Works

1. **Trigger** — the workflow fires on every new email arriving on the `email-support` connector channel. Reply threads are ignored (`ignore_replies: true`) to avoid infinite loops.
2. **Classify** — a quick `invoke_agent` step categorizes the email (product question, bug report, etc.) to help the agent tailor its response.
3. **Draft response** — spawns a `user/support-agent` with access to the **product manual** and **FAQ** attachments. The agent reads these files to ground its response in real product documentation — no hallucination.
4. **Send reply** — calls `connector.send_message` to send the response back to the customer on the same email thread.

::: tip Attachments are the secret sauce
Workflow attachments let you give an agent **specific knowledge** for a specific workflow — without cluttering the global knowledge graph. Upload your product manual, API docs, or any reference material and the agent can read them during execution.
:::

## Expected Email Response

```
Hi Sarah,

Thanks for reaching out! I understand you're having trouble
setting up SSO for your team.

According to the product manual (Section 4.2: Authentication),
here's how to enable SSO:

1. Go to Settings → Organization → Authentication
2. Click "Configure SSO Provider"
3. Enter your IdP metadata URL
4. Map the SAML attributes (see Section 4.2.3 for the full list)
5. Click "Test Connection" before saving

If you're seeing a specific error during step 3, could you share
the error message? That will help me pinpoint the issue.

Happy to help with anything else! 😊

— HiveMind OS Support
```

## Customization Ideas

- **Add a `feedback_gate`** — switch to `mode: chat` and have a human review responses before sending
- **Route by category** — use a `branch` step after classification to handle billing emails differently (e.g. forward to a human)
- **Escalation** — add a `branch` step: if the agent's confidence is low, escalate to a human inbox instead of auto-replying
- **Multi-language** — the support persona prompt already handles language matching, but you can add a `set_variable` step to detect language explicitly
- **Add more attachments** — upload API documentation, release notes, or troubleshooting guides for richer responses

## Related

- [Workflows Guide](/guides/workflows) — Step types, control flow, and error handling
- [Personas Guide](/guides/personas) — Building effective agent personas
- [MCP Servers Guide](/guides/mcp-servers) — Connecting email and messaging tools
- [Skills Guide](/guides/skills) — Adding domain knowledge via skills
