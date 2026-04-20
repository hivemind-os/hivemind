# @hivemind-os/plugin-sdk

TypeScript SDK for building Hivemind connector plugins. Create plugins that provide AI agent tools, configuration UIs, and background sync loops — all in TypeScript.

## Quick Start

```bash
# Create a new plugin project
npm create @hivemind-os/plugin my-connector

# Or start from scratch
mkdir my-connector && cd my-connector
npm init -y
npm install @hivemind-os/plugin-sdk
```

```typescript
// src/index.ts
import { definePlugin, z } from '@hivemind-os/plugin-sdk';

export default definePlugin({
  configSchema: z.object({
    apiKey: z.string().secret().label('API Key').section('Auth'),
    endpoint: z.string().default('https://api.example.com').label('API URL'),
  }),

  tools: [
    {
      name: 'get_items',
      description: 'Fetch items from the API',
      parameters: z.object({
        limit: z.number().default(10).describe('Max results'),
      }),
      execute: async (params, ctx) => {
        const res = await fetch(`${ctx.config.endpoint}/items?limit=${params.limit}`, {
          headers: { Authorization: `Bearer ${ctx.config.apiKey}` },
        });
        return { content: await res.json() };
      },
    },
  ],
});
```

## Building

```bash
tsc && hivemind-extract-schema
```

The SDK includes `hivemind-extract-schema`, a CLI that extracts your Zod config schema to `dist/config-schema.json` at build time. This static file is what Hivemind reads to render your config form — no running plugin process needed. Add both commands to your `"build"` script in `package.json`.

## Features

### Config Schemas (Zod-based)

Define your plugin's configuration using Zod schemas with UI extensions:

```typescript
configSchema: z.object({
  apiKey: z.string().secret().label('API Key'),      // password input, stored in keyring
  team: z.string().label('Team Name'),                // text input
  mode: z.enum(['fast', 'safe']).radio(),             // radio buttons
  limit: z.number().min(1).max(100).default(20),      // number input with validation
  tags: z.array(z.string()).default([]),               // array input
  notify: z.boolean().default(true).label('Notify'),   // checkbox
})
```

**UI extensions:**
- `.label(text)` — display label
- `.helpText(text)` — tooltip
- `.section(name)` — group fields into sections
- `.secret()` — password field, stored in OS keyring
- `.radio()` — render enum as radio buttons
- `.placeholder(text)` — input placeholder

### Tools

Tools are functions the AI agent can call:

```typescript
tools: [
  {
    name: 'search',
    description: 'Search for items',
    parameters: z.object({
      query: z.string().describe('Search query'),
    }),
    annotations: { sideEffects: false },
    execute: async (params, ctx) => {
      const results = await myApi.search(params.query);
      return { content: results };
    },
  },
]
```

### Background Loop

Optionally poll for updates and push messages into Hivemind:

```typescript
loop: async (ctx) => {
  let cursor = await ctx.store.get<string>('lastSync');

  while (!ctx.signal.aborted) {
    const updates = await myApi.getUpdates(cursor);

    for (const update of updates) {
      await ctx.emitMessage({
        source: `myapp:${update.id}`,       // dedup key
        channel: 'my-feed',
        content: update.text,
        sender: { id: update.userId, name: update.userName },
      });
    }

    cursor = updates.cursor;
    await ctx.store.set('lastSync', cursor);
    await ctx.sleep(60_000);
  }
},
```

### Plugin Context (`ctx`)

Every tool and lifecycle hook receives a `PluginContext` with host APIs:

| API | Description |
|-----|-------------|
| `ctx.config` | Resolved config values |
| `ctx.emitMessage(msg)` | Push message into connector pipeline |
| `ctx.emitMessages(msgs)` | Batch message emission |
| `ctx.secrets.get/set/delete/has` | OS keyring (plugin-scoped) |
| `ctx.store.get/set/delete/keys` | Persistent KV storage |
| `ctx.logger.debug/info/warn/error` | Structured logging |
| `ctx.notify({ title, body })` | Desktop notification |
| `ctx.emitEvent(type, payload)` | Emit workflow trigger event |
| `ctx.updateStatus({ state, message })` | Update UI status |
| `ctx.sleep(ms)` | Cancellation-aware sleep |
| `ctx.http.fetch(url, init)` | Proxied HTTP |
| `ctx.dataDir.*` | Plugin data directory |
| `ctx.host.version/platform` | Host environment info |
| `ctx.connectors.list()` | List other connectors |
| `ctx.personas.list()` | List personas |

### Testing

Use the built-in test harness:

```typescript
import { createTestHarness } from '@hivemind-os/plugin-sdk/testing';
import myPlugin from '../src/index';

const harness = createTestHarness(myPlugin, {
  config: { apiKey: 'test-key' },
  secrets: { extra: 'value' },
});

// Call tools
const result = await harness.callTool('search', { query: 'hello' });
expect(result.content).toHaveLength(5);

// Run the loop until N messages are emitted
await harness.runLoopUntil({ messageCount: 3, timeoutMs: 5000 });
expect(harness.messages).toHaveLength(3);

// Check captured side effects
expect(harness.events).toHaveLength(1);
expect(harness.notifications).toHaveLength(0);
expect(harness.statuses[0].state).toBe('connected');
```

### Lifecycle Hooks

```typescript
onActivate: async (ctx) => {
  // Validate config, warm caches
  const res = await fetch(ctx.config.endpoint + '/me', { ... });
  if (!res.ok) throw new Error('Invalid credentials');
  await ctx.updateStatus({ state: 'connected' });
},

onDeactivate: async (ctx) => {
  await ctx.updateStatus({ state: 'disconnected' });
},
```

## Package Structure

```
my-plugin/
├── package.json              # includes "hivemind" manifest field
├── tsconfig.json
├── src/
│   ├── index.ts              # definePlugin({ ... })
│   ├── tools/                # one file per tool (recommended)
│   │   ├── search.ts
│   │   └── create.ts
│   └── loop.ts               # background loop (optional)
├── test/
│   └── plugin.test.ts
└── dist/                     # compiled JS
```

### package.json `hivemind` field

```json
{
  "hivemind": {
    "type": "connector",
    "displayName": "My Service",
    "description": "Connect to My Service",
    "categories": ["productivity"],
    "permissions": ["network:api.example.com", "secrets:read", "loop:background"]
  }
}
```

## API Reference

See the full TypeScript types in `src/types.ts` for detailed documentation of every interface.

## License

MIT
