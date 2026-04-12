# Fully Local, Zero-Cloud Setup

Run HiveMind OS with **zero cloud dependency** — all models local, all data on your machine, no telemetry. Ideal for air-gapped environments, sensitive codebases, or simply keeping everything private.

## Step 1: Install a Local Model via Ollama

Install [Ollama](https://ollama.ai) and pull a capable model:

::: code-group
```sh [macOS / Linux]
curl -fsSL https://ollama.ai/install.sh | sh
ollama pull llama3.1:70b
ollama pull llama3.1:8b   # smaller model for scanning
```
```sh [Windows]
# Download installer from https://ollama.ai/download
ollama pull llama3.1:70b
ollama pull llama3.1:8b
```
:::

::: tip Use a Smaller Model as Scanner
The `scanner` role handles quick classification tasks (data labeling, tool-call validation). A smaller model like `llama3.1:8b` or `mistral:7b` responds much faster and uses far less memory, freeing your GPU for the primary model.
:::

## Step 2: Configure the Provider

Set up Ollama as your only provider with `local-only` channel class — the strictest level, ensuring no data leaves your machine:

```yaml
# config.yaml
models:
  providers:
    - id: local-ollama
      kind: ollama-local
      name: Local Ollama
      channel_class: local-only
      models:
        - llama3.1:70b
        - llama3.1:8b
      enabled: true
      priority: 10
```

The `local-only` channel class means HiveMind OS treats all data handled by this provider as fully local — it will never attempt to send it to a less-restricted channel.

## Step 3: Block All Cloud Overrides

Even if a future configuration change adds a cloud provider, these override policies ensure nothing leaks:

```yaml
# config.yaml (continued)
security:
  override_policy:
    internal: block
    confidential: block
    restricted: block
```

Setting every override policy to `block` means HiveMind OS will hard-stop any attempt to send classified data to a less-restricted channel. No prompts, no exceptions.

## Step 4: Use Local MCP Servers Only

Configure MCP servers that run entirely on your machine. MCP servers are configured per-persona, so add them to your persona YAML. Avoid any SSE or HTTP transports pointing to external URLs:

```yaml
# In your persona YAML (e.g. personas/local-dev.yaml)
id: user/local-dev
name: Local Developer
system_prompt: "You are a helpful coding assistant."
preferred_models:
  - llama3.1:70b
loop_strategy: react
allowed_tools:
  - "*"
mcp_servers:
  - id: filesystem
    transport: stdio
    command: npx
    args: ["-y", "@anthropic/mcp-filesystem"]
    channel_class: local-only

  - id: git-tools
    transport: stdio
    command: npx
    args: ["-y", "@anthropic/mcp-git"]
    channel_class: local-only
```

The `local-only` channel class is the most restrictive — these tools can only exchange data with local providers.

## Full Configuration

Here is the complete `config.yaml` for a zero-cloud setup:

```yaml
# config.yaml
models:
  providers:
    - id: local-ollama
      kind: ollama-local
      name: Local Ollama
      channel_class: local-only
      models:
        - llama3.1:70b
        - llama3.1:8b
      enabled: true
      priority: 10

security:
  override_policy:
    internal: block
    confidential: block
    restricted: block
```

MCP servers are configured per-persona rather than in `config.yaml`. See **Step 4** above for the persona YAML with local-only MCP servers.

## Performance Tips for Local-Only Operation

- **GPU memory matters** — the 70B model needs ~40 GB VRAM for full speed. If you have less, use `llama3.1:8b` as primary or enable quantized variants (`llama3.1:70b-q4_K_M`)
- **Keep Ollama running** — start it as a system service so the model stays loaded in memory between requests
- **Use the scanner role** — offload quick classification tasks to the 8B model so the 70B isn't interrupted for trivial checks
- **SSD storage** — knowledge graph and context data benefit from fast local disk I/O

::: warning Air-Gapped Environments
If your machine has no internet at all, pre-download the Ollama models and MCP server npm packages on a connected machine, then transfer them. Ollama models live in `~/.ollama/models/` and can be copied directly.
:::
