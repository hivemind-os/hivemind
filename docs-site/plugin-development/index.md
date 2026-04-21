# Hivemind Plugin Development

Build connector plugins for Hivemind in TypeScript. Plugins can provide AI agent tools, configuration UIs, and background sync loops.

## Guides

| Guide | Description |
|-------|-------------|
| [Quick Start](/plugin-development/quick-start) | Get a working plugin in 5 minutes |
| [Concepts](/plugin-development/concepts) | Architecture, lifecycle, and how plugins work |
| [Config Schemas](/plugin-development/config-schemas) | Zod-based config schemas and UI rendering |
| [Tools Guide](/plugin-development/tools-guide) | Writing tools — parameters, results, side-effects |
| [Background Loops](/plugin-development/background-loops) | Polling, message emission, sync cursors |
| [Auth Guide](/plugin-development/auth-guide) | OAuth2, API keys, token-based authentication |
| [Testing Guide](/plugin-development/testing-guide) | Using the test harness, mocking, CI setup |
| [Publishing Guide](/plugin-development/publishing-guide) | How to publish to npm and submit to the registry |

## Cookbook

| Recipe | Description |
|--------|-------------|
| [REST API CRUD Tools](/plugin-development/cookbook/crud-tools) | Pattern for wrapping REST APIs |
| [Polling Loop](/plugin-development/cookbook/polling-loop) | Efficient polling with dedup and cursors |

## Reference

- **[@hivemind-os/plugin-sdk README](https://github.com/hivemind-os/hivemind/tree/main/packages/plugin-sdk)** — SDK API overview
- **[Type definitions](https://github.com/hivemind-os/hivemind/blob/main/packages/plugin-sdk/src/types.ts)** — Full TypeScript types
- **[Sample Plugin](https://github.com/hivemind-os/hivemind/tree/main/packages/sample-plugins/github-issues)** — Complete GitHub Issues connector
- **[Test Plugin](https://github.com/hivemind-os/hivemind/tree/main/packages/test-plugin)** — Plugin exercising every host API

