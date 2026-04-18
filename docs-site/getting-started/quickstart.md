# Quickstart

You installed HiveMind OS — nice. Let's get you from zero to a working AI agent in about two minutes.

## 1. The Setup Wizard

Open HiveMind OS. On first launch you'll see a **Setup Wizard** that walks you through everything step by step:

::: info 📸 Screenshot needed
The Setup Wizard welcome screen on first launch.
:::

| Step | What you do |
|------|-------------|
| **Welcome** | Click **Get Started** |
| **Providers** | Add at least one AI model provider (see options below) |
| **Connectors** | Optionally connect email, calendar, or chat services — or skip for now |
| **Web Search** | Optionally configure web search — or skip |
| **Models** | Optionally download the **BGE Small** embedding model for local knowledge search |
| **Tour** | Quick overview of key features — then you're in! |

You only need to complete the **Providers** step to start chatting. Everything else can be configured later in **Settings**.

## 2. Connect a Model Provider

In the Providers step, pick a provider card and fill in the details:

::: info 📸 Screenshot needed
Provider selection screen showing the available provider cards.
:::

### Option A: OpenAI-Compatible API Key (simplest)

1. Select **OpenAI Compatible**
2. Paste your API key and base URL. Done.

### Option B: Anthropic

1. Select **Anthropic**
2. Paste your Anthropic API key. Done.

### Option C: GitHub Copilot

1. Select **GitHub Copilot**
2. Your browser opens the GitHub OAuth flow — approve access
3. Token is stored securely in your OS keychain. Done.

### Option D: Ollama (Local)

1. Select **Ollama (Local)**
2. Make sure Ollama is running on your machine (default: `http://localhost:11434`)
3. Pick which models to use. Done — fully offline.

::: tip
You can add more providers later in **Settings → Providers**. HiveMind OS routes between them automatically based on task, cost, and data sensitivity.
:::

## 3. Your First Conversation

Once connected, you land in the **Chat** view with a greeting from your agent. Let's try something practical.

::: info 📸 Screenshot needed
The Chat view with the agent greeting after completing setup.
:::

**Step 1 — Ask a real business question.** Type (or copy-paste):

```
I need to write a professional email to a client explaining that their
project will be delayed by two weeks. Keep it empathetic and solution-focused.
```

**Step 2 — Watch the agent work.** You'll see HiveMind OS thinking in real time:

```
🧠 Planning: I'll draft a professional, empathetic email explaining the delay...

✍️ Drafting email with a clear explanation, apology, and revised timeline...

✅ Done — here's your email draft.
```

**Step 3 — Get a polished email draft.** The agent delivers a ready-to-send email — professional tone, empathetic language, and a clear next step for the client. Copy it, tweak it if you like, and send.

**Now try something else:**

```
Summarize the key points from this article: https://example.com/your-article
```

The agent fetches the page, reads the content, and gives you a clean summary with the key takeaways — no need to read the whole thing yourself.

## 4. What Just Happened?

Behind the scenes, HiveMind OS:

- **Understood your request** — figured out what you needed and the best way to help
- **Chose the right approach** — drafting, researching, analyzing, or a combination
- **Used tools when needed** — web search, file reading, email, and more
- **Kept your data private** — nothing left your computer unless you allowed it

That's the core of HiveMind OS: an AI assistant that thinks, acts, and respects your privacy — automatically.

## 5. What's Next?

You're up and running! Here's where to go from here:

### For Business Users

- **[Use Cases](/use-cases/)** — Ready-made automations for customer support, meeting prep, daily briefings, and more
- **[No-Code Guide](/guides/no-code-guide)** — Do everything from the app — no terminal, no config files
- **[First Five Minutes](./first-five-minutes.md)** — Quick tips to get the most out of HiveMind OS
- **[Privacy & Security](../concepts/privacy-and-security.md)** — Learn how your private data stays private

### For Developers

- **[Concepts → Agentic Loops](../concepts/agentic-loops.md)** — Understand how HiveMind OS reasons and plans
- **[Concepts → Knowledge Graph](../concepts/knowledge-graph.md)** — See how memory works across sessions
- **[Concepts → Tools & MCP](../concepts/tools-and-mcp.md)** — Connect external tools like GitHub, Slack, or databases
