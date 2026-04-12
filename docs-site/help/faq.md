# FAQ

Frequently asked questions about HiveMind OS.

## 1. Is my data sent to the cloud?

**Only if you configure a cloud provider** (OpenAI, Anthropic, etc.). HiveMind OS gives you full control:

- Local models (via Ollama) keep everything on your machine.
- The [security classification system](../reference/configuration.md) lets you define rules so sensitive data is never sent to cloud providers.
- You choose which provider handles each conversation or persona.

## 2. Which AI model should I use?

It depends on the task:

| Task | Recommended Model |
|---|---|
| Complex reasoning & coding | Claude Sonnet, GPT-4 |
| Quick questions & chat | Claude Haiku, GPT-4o mini |
| Offline / private work | Llama 3 (via Ollama) |
| Embeddings & knowledge | nomic-embed-text (via Ollama) |

::: tip
You can assign different models to different personas — use a fast model for chat and a powerful model for code review.
:::

## 3. Can I use it offline?

**Yes.** Install [Ollama](https://ollama.com) and configure a local model. All features — chat, knowledge graph, bots, workflows — work fully offline with local models.

## 4. How much disk space does it need?

- **Base install:** ~200 MB
- **With local models:** varies by model (e.g., Llama 3 8B ≈ 4.7 GB)
- **Knowledge graph:** grows with usage, typically under 500 MB

## 5. Can I use multiple AI providers at once?

**Yes.** Configure as many providers as you like and assign them to different personas using `preferred_models`:

```yaml
# personas/coder.yaml
preferred_models:
  - claude-sonnet

# personas/researcher.yaml
preferred_models:
  - gpt-4o
```

The router automatically directs requests to the right provider based on the active persona's preferred models.

## 6. Is it free?

**HiveMind OS is open source and free to use.** You bring your own API keys for cloud providers (OpenAI, Anthropic, etc.) and pay them directly. Local models via Ollama are completely free.

## 7. How do I update?

- **Desktop app:** Auto-updates by default. Check for updates in **Settings → About**.
- **CLI standalone:** Run the update command:
  ```bash
  hive update
  ```

## 8. Can I use it with my team?

**Yes.** HiveMind OS supports collaboration through:

- **Shared configs:** Export and share config files for consistent setups.
- **Workflow sharing:** Distribute workflow definitions across your organization.

## 9. What operating systems are supported?

| OS | Status |
|---|---|
| macOS (Apple Silicon & Intel) | ✅ Fully supported |
| Windows 10/11 | ✅ Fully supported |
| Linux (Ubuntu, Fedora, Arch) | ✅ Fully supported |

## 10. Where do I report bugs?

File an issue on the [GitHub Issues](https://github.com/hivemind-os/hivemind/issues) page. Include:

- Your OS and HiveMind OS version (from **Settings → About**)
- Steps to reproduce
- Relevant logs (`~/.hivemind/logs/`)

::: tip
Run `hive daemon status` and include the output in your bug report — it captures useful diagnostic information.
:::

## See Also

- [Troubleshooting](./troubleshooting.md) — solutions to common issues
- [Configuration Reference](../reference/configuration.md) — full config guide
