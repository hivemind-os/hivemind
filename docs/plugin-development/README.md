# Hivemind Plugin Development

Build connector plugins for Hivemind in TypeScript. Plugins can provide AI agent tools, configuration UIs, and background sync loops.

## Guides

| Guide | Description |
|-------|-------------|
| [Quick Start](./quick-start.md) | Get a working plugin in 5 minutes |
| [Concepts](./concepts.md) | Architecture, lifecycle, and how plugins work |
| [Config Schemas](./config-schemas.md) | Zod-based config schemas and UI rendering |
| [Tools Guide](./tools-guide.md) | Writing tools — parameters, results, side-effects |
| [Background Loops](./background-loops.md) | Polling, message emission, sync cursors |
| [Auth Guide](./auth-guide.md) | OAuth2, API keys, token-based authentication |
| [Testing Guide](./testing-guide.md) | Using the test harness, mocking, CI setup |
| [Publishing Guide](./publishing-guide.md) | How to publish to npm and submit to the registry |

## Cookbook

| Recipe | Description |
|--------|-------------|
| [REST API CRUD Tools](./cookbook/crud-tools.md) | Pattern for wrapping REST APIs |
| [Polling Loop](./cookbook/polling-loop.md) | Efficient polling with dedup and cursors |

## Reference

- **[@hivemind/plugin-sdk README](../../packages/plugin-sdk/README.md)** — SDK API overview
- **[Type definitions](../../packages/plugin-sdk/src/types.ts)** — Full TypeScript types
- **[Sample Plugin](../../packages/sample-plugins/github-issues/)** — Complete GitHub Issues connector
- **[Test Plugin](../../packages/test-plugin/)** — Plugin exercising every host API
