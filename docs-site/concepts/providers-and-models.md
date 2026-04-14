# Providers & Models

A provider is where your AI models live — cloud APIs, local servers, or both.

HiveMind OS doesn't lock you into a single vendor. You can wire up as many providers as you like — mixing cloud heavyweights with local models running on your own hardware — and the system will route requests to the right one automatically.

## What Is a Provider?

A **provider** is an API endpoint that serves language models. It could be a cloud service like OpenAI, a self-hosted Ollama instance on your LAN, or even a tiny model running directly inside the HiveMind OS daemon. Each provider has:

- A **connection** (URL + credentials)
- A list of **available models**

## Supported Providers

| Provider | Kind (config value) | Notes |
|----------|------|-------|
| **OpenAI-compatible** | `open-ai-compatible` | Any API that speaks the OpenAI protocol — including OpenAI itself |
| **Anthropic** | `anthropic` | Claude Sonnet, Claude Opus |
| **GitHub Copilot** | `github-copilot` | Free for GitHub users — see tip below |
| **Microsoft Foundry** | `microsoft-foundry` | Azure-hosted models with auto-discovery |
| **Ollama** | `ollama-local` | Run open models on your own machine |
| **Local Models** | `local-models` | Models running directly on your hardware |

::: tip GitHub Copilot — Free AI Models
If you have a GitHub account, you already have access to AI models through GitHub Copilot. HiveMind OS can authenticate via GitHub OAuth — no API keys to manage, no credit card required. It's the fastest way to get started.
:::

## Model Roles

Not every task needs the most powerful (and expensive) model. HiveMind OS assigns models to **roles** so the right model handles the right job:

| Role | Purpose | Example Model |
|------|---------|---------------|
| **Primary** | Main conversation & reasoning | Claude Sonnet, GPT-4o |
| **Secondary** | Fallback when primary is unavailable | GPT-4o-mini, Gemini Flash |
| **Admin** | High-frequency housekeeping (routing, summarisation, triage) | Llama 3, GPT-4o-mini |
| **Coding** | Optimised for code generation | Claude Sonnet via Copilot |
| **Scanner** | Prompt injection detection | Llama 3.2 (can be a cheap/fast model) |
| **Vision** | Image understanding | GPT-4o |

When the system needs a model, it resolves in order: **explicit role → admin → primary**. You can override any role per conversation, per bot, or per workflow step.

## Multi-Provider Setup

The real power is mixing providers. Use the best model for each job:

- **Claude** for complex reasoning (primary)
- **Local Llama** for classification and scanning (admin, scanner)
- **GPT-4o** for image understanding (vision)
- **GitHub Copilot** for code generation (coding)

This keeps costs down, data local where it matters, and gives you frontier-quality results where it counts.

## Data Classification

Model providers are **not** part of the data classification system — classification gates apply to outbound **channels** like MCP servers, messaging connectors, and peer connections. Data sent to your configured model providers is governed by your choice of provider (cloud vs. local) rather than the channel classification rules.

If keeping sensitive data off cloud APIs matters to you, run a local model (Ollama, Local Models) for tasks that handle private information, and use cloud providers for less sensitive work.

::: tip
For full details on how data classification works, see [Privacy & Security](./privacy-and-security).
:::

## Putting It All Together

Here's a real-world configuration using two providers — a cloud provider for powerful reasoning and a local one for sensitive tasks:

```yaml
providers:
  - kind: anthropic
    name: Claude (Primary)
    models:
      primary: claude-sonnet-4-20250514
      coding: claude-sonnet-4-20250514

  - kind: ollama-local
    name: Local Llama
    models:
      scanner: llama3.2
      admin: llama3.2
```

Cloud Claude handles conversations and coding, while local Llama runs prompt scanning and housekeeping tasks — keeping those operations entirely on your machine.

## Learn More

- [Configure Providers](/guides/configure-providers) — Step-by-step setup for each provider
- [Privacy & Security](./privacy-and-security) — Deep dive into the classification system
- [How It Works](./how-it-works) — The big picture of HiveMind OS architecture
