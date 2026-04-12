# Local Models

Running models locally keeps your data on-device, costs nothing per token, and works offline. HiveMind OS supports local model providers as first-class citizens — they follow the same routing, classification, and role-assignment rules as cloud providers.

## Why Local Models?

- **Privacy** — Data classified as `RESTRICTED` or `CONFIDENTIAL` never leaves your machine. Local providers have a `private` channel class, so the classification gate allows sensitive data to flow freely.
- **Zero cost** — No API keys, no per-token billing. Run as many requests as your hardware allows.
- **Offline** — Works without an internet connection. Useful for travel, air-gapped environments, or unreliable networks.
- **Low latency** — No network round-trip. Local models are ideal for high-frequency internal tasks like classification, summarization, and intent parsing.

## Setting Up Ollama

[Ollama](https://ollama.com) is the recommended way to run local models with HiveMind OS.

**1. Install Ollama:**

```bash
# macOS / Linux
curl -fsSL https://ollama.com/install.sh | sh

# Windows — download from https://ollama.com/download
```

**2. Pull a model:**

```bash
ollama pull llama3
ollama pull deepseek-coder   # great for coding tasks
```

**3. Configure as a provider** in your HiveMind OS `config.yaml`:

```yaml
models:
  providers:
    - id: ollama-local
      kind: ollama-local
      base_url: http://localhost:11434
      auth: none
      models: [llama3, deepseek-coder]
      channel_class: local-only      # Data stays on-device
```

**4. Use a local scanner model** to route prompt injection scanning to your local model:

```yaml
security:
  prompt_injection:
    enabled: true
    model_scanning_enabled: true
    scanner_models:
      - provider: ollama-local
        model: llama3
```

::: tip Use local models for scanning
The scanner model reviews inbound data for prompt injection. Running it locally ensures you never send potentially sensitive payloads to a cloud provider for scanning.
:::

## Setting Up LM Studio

[LM Studio](https://lmstudio.ai) is an alternative that provides a GUI for downloading and running models. It exposes an OpenAI-compatible API.

1. Download and install LM Studio.
2. Browse the model catalog and download a model.
3. Start the local server (default: `http://localhost:1234/v1`).
4. Configure in HiveMind OS:

```yaml
models:
  providers:
    - id: lm-studio
      kind: open-ai-compatible
      base_url: http://localhost:1234/v1
      auth: none
      models: [your-model-name]
      channel_class: local-only
```

## Hardware Detection & GPU Offloading

HiveMind OS detects available hardware on startup and selects the best inference backend automatically — no configuration required.

| GPU | Backend | Notes |
|---|---|---|
| Apple Silicon | Metal | Excellent performance, unified memory |
| NVIDIA | CUDA | Best throughput; needs CUDA toolkit |
| AMD / Intel | Vulkan | Broad support, slightly lower performance |
| CPU only | — | Works but slower; stick to small models |

You can tune local model behavior through the `local_models` section of your config:

```yaml
local_models:
  enabled: true
  max_loaded_models: 2          # How many models to keep in memory
  max_download_concurrent: 2    # Parallel downloads from HuggingFace
  auto_evict: true              # Unload least-used model when limit is hit
  isolate_runtimes: true        # Run inference in child processes (crash safety)
```

## Recommended Models

| Size | Model | RAM Required | Best For |
|---|---|---|---|
| **Small** (2 GB) | `phi3:mini`, `gemma:2b` | 4 GB | Admin tasks, classification, intent parsing |
| **Medium** (4 GB) | `llama3:8b`, `mistral` | 8 GB | General chat, summarization, coding assist |
| **Large** (8 GB) | `llama3:70b-q4`, `deepseek-coder:33b` | 16 GB+ | Complex reasoning, detailed code generation |

::: tip Embedding model
During the Setup Wizard, HiveMind OS offers to download the **BGE Small** embedding model for local knowledge search. You can also download it later in **Settings → Models**.
:::

## Using Local Models for Privacy

Pair local models with classification policies for defense in depth:

```yaml
security:
  override_policy:
    restricted: block          # Never send RESTRICTED data to cloud
    confidential: prompt       # Ask before sending to internal channels
    internal: allow
```

With this setup, `RESTRICTED` data never leaves the device, `CONFIDENTIAL` data requires your explicit approval, and the classification gate enforces this regardless of which model or persona is invoked.
