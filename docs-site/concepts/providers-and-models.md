# Providers & Models

A provider is where your AI models live — cloud APIs, local servers, or both.

HiveMind OS doesn't lock you into a single vendor. You can wire up as many providers as you like — mixing cloud heavyweights with local models running on your own hardware — and the system will route requests to the right one automatically.

## What Is a Provider?

A **provider** is an API endpoint that serves language models. It could be a cloud service like OpenAI, a self-hosted Ollama instance on your LAN, or even a tiny model running directly inside the HiveMind OS daemon. Each provider has:

- A **connection** (URL + credentials)
- A list of **available models**
- A **data classification level** that controls what information can be sent to it

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

## Channel Classification

Every provider gets a **data classification level** that controls what data can be sent to it.

::: info How Classification Works
Every piece of data in HiveMind OS carries a label — `PUBLIC`, `INTERNAL`, `CONFIDENTIAL`, or `RESTRICTED`. Every provider has a channel class that determines the maximum data level it can receive. If data is too sensitive for a provider's clearance, the request is blocked or rerouted automatically.

| Channel Class | Accepts Data Up To |
|---------------|--------------------|
| `public` | `PUBLIC` only |
| `internal` | `INTERNAL` |
| `private` | `CONFIDENTIAL` |
| `local-only` | All levels (data never leaves your machine) |
:::

This means you can safely mix cloud and local providers — sensitive data automatically stays on-device.

## Putting It All Together

Here's a real-world configuration using two providers with different trust levels:

```yaml
providers:
  - kind: anthropic
    name: Claude (Primary)
    channel_class: private
    models:
      primary: claude-sonnet-4-20250514
      coding: claude-sonnet-4-20250514

  - kind: ollama-local
    name: Local Llama
    channel_class: local-only
    models:
      scanner: llama3.2
      admin: llama3.2
```

Cloud Claude handles conversations and coding with `private` channel class (accepts up to CONFIDENTIAL data), while local Llama runs prompt scanning and housekeeping tasks with `local-only` channel class — meaning even your most sensitive data never leaves your machine for those tasks.

## Learn More

- [Configure Providers](/guides/configure-providers) — Step-by-step setup for each provider
- [Privacy & Security](./privacy-and-security) — Deep dive into the classification system
- [How It Works](./how-it-works) — The big picture of HiveMind OS architecture
