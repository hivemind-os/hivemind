# Quick Start: Your First Hivemind Plugin

Build a working Hivemind connector plugin in 5 minutes.

## Prerequisites

- Node.js 18+
- npm

## 1. Create a new project

```bash
mkdir my-first-plugin
cd my-first-plugin
npm init -y
npm install @hivemind-os/plugin-sdk
npm install -D typescript @types/node
```

Create `tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "declaration": true,
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*"]
}
```

## 2. Write your plugin

Create `src/index.ts`:

```typescript
import { definePlugin, z } from '@hivemind-os/plugin-sdk';

export default definePlugin({
  // Config — rendered as a form in the Hivemind Settings UI
  configSchema: z.object({
    apiKey: z.string().secret().label('API Key').section('Auth'),
    baseUrl: z.string().default('https://api.example.com').label('API URL'),
  }),

  // Tools — callable by the AI agent
  tools: [
    {
      name: 'hello',
      description: 'Say hello to someone',
      parameters: z.object({
        name: z.string().describe('Name of the person'),
      }),
      execute: async (params, ctx) => {
        ctx.logger.info(`Saying hello to ${params.name}`);
        return { content: `Hello, ${params.name}! (from ${ctx.config.baseUrl})` };
      },
    },
  ],
});
```

## 3. Add the hivemind manifest

Update your `package.json`:

```json
{
  "type": "module",
  "main": "dist/index.js",
  "hivemind": {
    "type": "connector",
    "displayName": "My First Plugin",
    "description": "A hello world connector plugin",
    "categories": ["example"]
  }
}
```

## 4. Build

```bash
npx tsc && hivemind-extract-schema
```

The `hivemind-extract-schema` command (included with `@hivemind-os/plugin-sdk`) reads your
compiled plugin and writes `dist/config-schema.json`. This static schema file is what
Hivemind reads to render your config form in the UI — no running plugin process needed.

Add it to your `package.json` build script:

```json
{
  "scripts": {
    "build": "tsc && hivemind-extract-schema"
  }
}
```

## 5. Test

Install vitest and create a test:

```bash
npm install -D vitest
```

Create `test/plugin.test.ts`:

```typescript
import { describe, it, expect } from 'vitest';
import { createTestHarness } from '@hivemind-os/plugin-sdk/testing';

process.env.HIVEMIND_PLUGIN_TEST_MODE = '1';
import myPlugin from '../src/index';

describe('My Plugin', () => {
  const harness = createTestHarness(myPlugin, {
    config: { apiKey: 'test-key', baseUrl: 'https://test.example.com' },
  });

  it('should say hello', async () => {
    const result = await harness.callTool('hello', { name: 'World' });
    expect(result.content).toBe('Hello, World! (from https://test.example.com)');
  });

  it('should validate config', () => {
    expect(harness.validateConfig({ apiKey: 'key' }).valid).toBe(true);
    expect(harness.validateConfig({}).valid).toBe(false);
  });
});
```

Run tests:

```bash
npx vitest run
```

## 6. Link to Hivemind (local development)

```bash
hivemind plugin link .
```

Your plugin now appears in Hivemind's Settings → Plugins.

## Next Steps

- **[Tools Guide](./tools-guide.md)** — Add more tools with parameters, side-effects, and approval
- **[Background Loops](./background-loops.md)** — Add a polling loop for real-time updates
- **[Config Schemas](./config-schemas.md)** — Rich configuration forms
- **[Sample Plugin](../../packages/sample-plugins/github-issues/)** — Study a complete real-world example

