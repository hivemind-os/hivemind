# Configuration Reference

HiveMind OS is configured through a YAML file that controls daemon settings, AI providers, security, personas, and more.

## Config File Location

The default config file is located at:

```
~/.hivemind/config.yaml
```

Override with the `HIVEMIND_CONFIG_PATH` environment variable:

```bash
HIVEMIND_CONFIG_PATH=/path/to/custom-config.yaml hive daemon start
```

## Config Precedence

Settings are resolved in the following order (highest priority first):

1. **Environment variables** — `HIVEMIND_CONFIG_PATH`, `HIVEMIND_HOME`, and other `HIVEMIND_*` prefixed vars
2. **Config file** — `~/.hivemind/config.yaml`
3. **Defaults** — built-in sensible defaults

## Environment Variable Substitution

API keys and secrets can be injected from environment variables using the `env:` prefix in auth fields:

```yaml
models:
  providers:
    - id: openai
      auth: "env:OPENAI_API_KEY"
```

## Schema Overview

The config file has the following top-level sections:

| Section | Description |
|---|---|
| `daemon` | Logging and event bus settings |
| `api` | HTTP API bind address and toggle |
| `security` | Override policies, prompt injection scanning, command policies, sandboxing |
| `models` | AI provider connections (`models.providers[]`) and timeout settings |
| `local_models` | On-device model inference configuration |
| `embedding` | Embedding model definitions and file-pattern rules |
| `compaction` | Context compaction settings |
| `personas` | Agent persona configurations (usually managed via persona YAML files) |
| `skills` | Skill configurations |
| `afk` | [AFK (idle) mode settings](/guides/afk-mode) |
| `python` | [Managed Python runtime](/concepts/managed-runtimes) configuration |
| `node` | [Managed Node.js runtime](/concepts/managed-runtimes) configuration |
| `tool_limits` | Per-tool rate and size limits |
| `web_search` | Web search configuration |

## Complete Example

```yaml
# ~/.hivemind/config.yaml

# --- Daemon ---
daemon:
  log_level: info
  event_bus_capacity: 512

# --- API ---
api:
  bind: "127.0.0.1:9180"
  http_enabled: true

# --- AI Providers ---
models:
  request_timeout_secs: 60
  stream_timeout_secs: 120
  providers:
    - id: openai
      kind: open-ai-compatible
      name: OpenAI
      base_url: "https://api.openai.com/v1"
      auth: "env:OPENAI_API_KEY"
      models:
        - gpt-4o
        - gpt-4o-mini
      channel_class: public
      priority: 100
      enabled: true

    - id: anthropic
      kind: anthropic
      name: Anthropic
      base_url: "https://api.anthropic.com"
      auth: "env:ANTHROPIC_API_KEY"
      models:
        - claude-sonnet-4-20250514
      channel_class: public
      priority: 90

    - id: local-ollama
      kind: ollama-local
      name: Local Ollama
      models:
        - llama3.1:70b
      channel_class: local-only
      priority: 50

# --- Local Models ---
local_models:
  enabled: false
  storage_path: ~/.hivemind/models
  max_loaded_models: 2
  max_download_concurrent: 1
  auto_evict: true
  isolate_runtimes: false

# --- Security ---
security:
  override_policy:
    internal: prompt
    confidential: prompt
    restricted: block
  prompt_injection:
    enabled: true
    action_on_detection: prompt
    confidence_threshold: 0.7
  command_policy:
    enabled: true
  sandbox:
    enabled: true
    allow_network: true

# --- Embedding ---
embedding:
  default_model: bge-small-en-v1.5
```

::: tip
Run `hive config show` to view the current configuration, or `hive config validate` to check your config for errors. Changes to the config file are picked up automatically — no restart needed.
:::

::: warning
Never commit API keys directly in the config file. Always use environment variable substitution (`env:VAR_NAME`) or a secrets manager.
:::

## See Also

- [CLI Overview](../cli/overview.md) — global flags that override config
