# Configure Providers

HiveMind OS is provider-agnostic — connect one or many LLM backends, and the model router picks the right one for each request based on data classification, task type, and availability.

All provider configuration lives in your `config.yaml` under `models` → `providers`.

## OpenAI

```yaml
models:
  providers:
    - id: openai
      kind: open-ai-compatible
      name: OpenAI
      auth: env:OPENAI_API_KEY
      base_url: https://api.openai.com/v1
      channel_class: internal
      models:
        - gpt-4o
```

## Anthropic

```yaml
models:
  providers:
    - id: anthropic
      kind: anthropic
      name: Anthropic
      auth: env:ANTHROPIC_API_KEY
      base_url: https://api.anthropic.com
      channel_class: private
      models:
        - claude-sonnet-4-20250514
```

Set `channel_class: private` if your Anthropic agreement covers sensitive data — the model router will respect this when routing prompts.

## GitHub Copilot

```yaml
models:
  providers:
    - id: copilot
      kind: github-copilot
      name: GitHub Copilot
      auth: github-oauth
      channel_class: internal
      models: []
```

No API key needed. HiveMind OS launches the GitHub OAuth device flow — sign in with your GitHub account and you're done. The token is stored securely in your OS keychain.

::: tip Free to use
If you have a GitHub account with Copilot access (free, Pro, or Enterprise), this provider costs you nothing extra. Great way to get started.
:::

## Ollama (Local Models)

Run models entirely on your machine. No data leaves your device.

```yaml
models:
  providers:
    - id: ollama
      kind: ollama-local
      name: Local Models
      auth: none
      channel_class: local-only
      models:
        - llama3.2
```

Make sure Ollama is running before starting HiveMind OS. The default base URL is `http://localhost:11434/v1` — you can omit `base_url` if using the default.

## OpenAI-Compatible

For any API that implements the OpenAI chat completions spec — self-hosted models, corporate proxies, or third-party services.

```yaml
models:
  providers:
    - id: my-custom-api
      kind: open-ai-compatible
      name: My Custom API
      base_url: https://my-api.example.com/v1
      auth: env:MY_API_KEY
      channel_class: internal
      models:
        - my-model
```

This also works with Azure OpenAI (use the alias `azure-open-ai` for `kind` if you prefer).

## Microsoft Foundry

```yaml
models:
  providers:
    - id: azure-foundry
      kind: microsoft-foundry
      name: Azure Foundry
      base_url: https://my-foundry.azure.com/v1
      auth: env:AZURE_API_KEY
      channel_class: private
      models:
        - gpt-4o
```

## Multiple Providers & Fallback Chains

You can configure multiple providers. The model router selects among them based on data classification, task type, and priority. If the primary provider fails (rate limit, timeout, auth error), HiveMind OS automatically cascades to the next eligible provider.

```yaml
models:
  providers:
    - id: anthropic
      kind: anthropic
      name: Claude (Primary)
      auth: env:ANTHROPIC_API_KEY
      base_url: https://api.anthropic.com
      channel_class: private
      models:
        - claude-sonnet-4-20250514

    - id: openai
      kind: open-ai-compatible
      name: OpenAI (Fallback)
      auth: env:OPENAI_API_KEY
      base_url: https://api.openai.com/v1
      channel_class: internal
      models:
        - gpt-4o

    - id: ollama
      kind: ollama-local
      name: Local (Offline Safety Net)
      auth: none
      channel_class: local-only
      models:
        - llama3.2
```

With this setup:
- **Claude** handles primary reasoning tasks and can receive CONFIDENTIAL data (via `private` channel class)
- **OpenAI** kicks in as a fallback for INTERNAL-classified prompts
- **Ollama** runs locally for scanning tasks — data never leaves your machine

## Environment Variables for Secrets

Use the `env:VAR_NAME` syntax in the `auth` field to reference environment variables. HiveMind OS resolves these at startup.

```yaml
auth: env:OPENAI_API_KEY         # ✅ reads from environment
auth: none                       # ✅ no auth needed (e.g. local models)
auth: github-oauth               # ✅ GitHub device flow
auth: api-key                    # ✅ API key from OS keychain
```

::: warning Never hardcode API keys
Always use `env:VAR` references for secrets. Hardcoded keys in config files risk leaking credentials through backups, version control, or file sharing. Set your keys in your shell profile, `.env` file, or OS secret manager.
:::

## Channel Class Reference

Each provider needs a `channel_class` that tells the model router what data is safe to send:

| Channel Class | Meaning |
|---|---|
| `local-only` | Most sensitive — only local providers should use this |
| `private` | Sensitive data — provider must have a data-handling agreement |
| `internal` | General-purpose — safe for org-internal data |
| `public` | Open data only — nothing private or proprietary |
