<p align="center">
  <img src="docs-site/public/hivemind-logo.jpg" alt="HiveMind OS" width="280" />
</p>

<h1 align="center">HiveMind OS</h1>

<p align="center"><strong>Your AI agent. Your machine. Your rules.</strong></p>

<p align="center">
  <a href="https://hivemind-os.io">Documentation</a> |
  <a href="https://hivemind-os.io/getting-started/quickstart">Quickstart</a> |
  <a href="https://hivemind-os.io/getting-started/installation">Installation</a> |
  <a href="https://hivemind-os.io/plugin-development/">Plugin Development</a>
</p>

---

HiveMind OS is a privacy-first desktop AI agent and automation platform. It runs on your machine, connects to the models and tools you choose, and helps you build reusable personas, workflows, and background bots without handing over all of your data to a hosted SaaS.

If you want to **use** HiveMind OS, start with the hosted docs at **[hivemind-os.io](https://hivemind-os.io)**. If you want to **build or contribute**, this repository contains the Rust workspace, desktop app, CLI, SDK packages, and documentation source.

## Why HiveMind OS

- **Local-first** - keep conversations, automations, and knowledge close to your machine
- **Privacy-aware** - data classification and outbound channel rules help prevent accidental leaks
- **Model-flexible** - use OpenAI, Anthropic, GitHub Copilot, Ollama, Azure, OpenRouter, and local runtimes
- **Automation-ready** - create personas, workflows, bots, scheduled jobs, and MCP-powered toolchains
- **Extensible** - add custom tools and plugins without rewriting the core app

## Documentation

The repo contains the docs source in `docs-site/`, but the best entry point is the hosted site:

| Need | Link |
|---|---|
| Docs home | [hivemind-os.io](https://hivemind-os.io) |
| Installation | [Installation Guide](https://hivemind-os.io/getting-started/installation) |
| First setup | [Quickstart](https://hivemind-os.io/getting-started/quickstart) |
| Core concepts | [How It Works](https://hivemind-os.io/concepts/how-it-works) |
| Personas and workflows | [Guides](https://hivemind-os.io/guides/personas) |
| Plugin authors | [Plugin Development Docs](https://hivemind-os.io/plugin-development/) |

## What's in this repo

| Path | Purpose |
|---|---|
| `apps/hivemind-desktop/` | Tauri + SolidJS desktop application |
| `crates/` | Rust workspace for the daemon, API, CLI, models, tools, memory, and agent runtime |
| `packages/plugin-sdk/` | TypeScript SDK for HiveMind plugins |
| `packages/plugin-registry/` | Published plugin catalog metadata |
| `docs-site/` | VitePress source for the hosted documentation site |
| `tests/` | Workspace-level integration and support tests |

## Developer quick start

### Prerequisites

- [Rust](https://rustup.rs) 1.85+
- [Node.js](https://nodejs.org) 18+
- npm or pnpm
- [Tauri CLI](https://v2.tauri.app) v2 for desktop development

### Build and run

```bash
# Build the Rust workspace
cargo build --workspace

# Run the workspace tests
cargo test --workspace

# Build the desktop frontend
cd apps/hivemind-desktop
npm install
npm run build
```

### Common dev commands

```bash
# Run the desktop app in development mode
cargo tauri dev

# Run the daemon directly
cargo run -p hive-daemon

# Explore the CLI
cargo run -p hive-cli -- --help
```

## Architecture at a glance

HiveMind OS is organized around a local daemon with multiple clients and extension points:

- **Desktop app** for interactive chat, setup, workflows, and model management
- **Rust services** for orchestration, security policies, model routing, MCP, scheduling, and knowledge storage
- **CLI and API** for automation and operational control
- **Plugin and MCP layers** for connecting outside tools and services

## License

MIT - see [LICENSE](./LICENSE).
