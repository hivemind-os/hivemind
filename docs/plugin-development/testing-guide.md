# Testing Guide

The SDK includes a test harness that simulates the Hivemind host, letting you test plugins in isolation without JSON-RPC or a running host.

## Setup

```bash
npm install -D vitest
```

## Using the Test Harness

```typescript
import { describe, it, expect, beforeEach } from 'vitest';
import { createTestHarness } from '@hivemind-os/plugin-sdk/testing';

// Prevent auto-start of the JSON-RPC runtime
process.env.HIVEMIND_PLUGIN_TEST_MODE = '1';
import myPlugin from '../src/index';

describe('My Plugin', () => {
  let harness;

  beforeEach(() => {
    harness = createTestHarness(myPlugin, {
      config: {
        apiKey: 'test-key',
        baseUrl: 'https://test.example.com',
      },
      secrets: {
        oauth_token: 'mock-token',
      },
      hostInfo: {
        version: '0.1.0',
        platform: 'linux',
        capabilities: ['tools', 'loop'],
      },
    });
  });

  // ... tests
});
```

## Testing Tools

```typescript
it('should list items', async () => {
  const result = await harness.callTool('list_items', {
    status: 'active',
    limit: 5,
  });
  expect(result.content).toBeDefined();
  expect(result.isError).toBeUndefined();
});

it('should reject invalid params', async () => {
  await expect(
    harness.callTool('list_items', { limit: -1 })
  ).rejects.toThrow();
});
```

## Testing Config Validation

```typescript
it('should validate good config', () => {
  const result = harness.validateConfig({
    apiKey: 'sk-123',
    baseUrl: 'https://api.example.com',
  });
  expect(result.valid).toBe(true);
});

it('should reject bad config', () => {
  const result = harness.validateConfig({});
  expect(result.valid).toBe(false);
  expect(result.errors.length).toBeGreaterThan(0);
});
```

## Testing the Background Loop

```typescript
it('should emit messages from the loop', async () => {
  await harness.runLoopUntil({
    messageCount: 3,     // wait for 3 messages
    timeoutMs: 10000,    // timeout after 10s
  });

  expect(harness.messages).toHaveLength(3);
  expect(harness.messages[0].channel).toBe('my-feed');
});
```

## Testing Lifecycle Hooks

```typescript
it('should activate successfully', async () => {
  await harness.activate();
  expect(harness.statuses.some(s => s.state === 'connected')).toBe(true);
});

it('should handle activation failure', async () => {
  const badHarness = createTestHarness(myPlugin, {
    config: { apiKey: 'invalid' },
  });
  await expect(badHarness.activate()).rejects.toThrow();
});
```

## Inspecting Side Effects

The harness captures all host API calls:

```typescript
// Messages emitted to the connector pipeline
harness.messages      // IncomingMessage[]

// Events emitted for workflow triggers
harness.events        // { type: string, payload: object }[]

// Desktop notifications sent
harness.notifications // { title: string, body: string }[]

// Status updates
harness.statuses      // PluginStatus[]

// Log entries
harness.logs          // { level: string, msg: string, data?: object }[]

// Secret store contents
harness.secretStore   // Record<string, string>

// KV store contents
harness.kvStore       // Record<string, unknown>
```

## Resetting Between Tests

```typescript
afterEach(() => {
  harness.reset(); // clears all captured data
});
```

## CI Configuration

Add to your `package.json`:

```json
{
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest"
  }
}
```

GitHub Actions example:

```yaml
- name: Test plugin
  run: npm test
  working-directory: packages/my-plugin
```

