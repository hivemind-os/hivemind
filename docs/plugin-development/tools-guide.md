# Writing Tools

Tools are the primary way your plugin provides capabilities to the AI agent. Each tool is a function with a name, description, typed parameters, and an execute handler.

## Basic Tool

```typescript
{
  name: 'get_weather',
  description: 'Get the current weather for a city',
  parameters: z.object({
    city: z.string().describe('City name'),
    units: z.enum(['celsius', 'fahrenheit']).default('celsius'),
  }),
  execute: async (params, ctx) => {
    const data = await fetchWeather(params.city, params.units);
    return { content: data };
  },
}
```

## Tool Naming

- Use `snake_case` for tool names
- Names must be unique within your plugin
- The host registers tools as `plugin.<pluginId>.<toolName>`
- Keep names concise but descriptive

## Parameters

Parameters use Zod schemas. The `.describe()` text is shown to the AI agent to help it understand what to provide.

```typescript
parameters: z.object({
  query: z.string().describe('Search query text'),
  limit: z.number().min(1).max(100).default(20).describe('Max results'),
  status: z.enum(['open', 'closed', 'all']).default('open'),
  tags: z.array(z.string()).optional().describe('Filter by tags'),
})
```

## Return Values

Tools return a `ToolResult`:

```typescript
// Simple string result
return { content: 'Task completed successfully' };

// Structured data (serialized to JSON for the agent)
return {
  content: {
    items: [...],
    total: 42,
    page: 1,
  },
};

// Error result
return { content: 'API rate limit exceeded', isError: true };

// Result with artifacts (files)
return {
  content: 'Report generated',
  artifacts: [{
    name: 'report.pdf',
    mimeType: 'application/pdf',
    content: base64EncodedPdf,
  }],
};
```

## Side Effects and Approval

Tools that modify external state should declare annotations:

```typescript
{
  name: 'create_item',
  description: 'Create a new item',
  parameters: z.object({ title: z.string() }),
  annotations: {
    sideEffects: true,    // this tool modifies external state
    approval: 'suggest',  // suggest user approval before execution
  },
  execute: async (params, ctx) => { ... },
}
```

Approval modes:
- `'never'` — execute immediately (read-only tools)
- `'suggest'` — suggest approval but allow auto-execution
- `'always'` — always require user approval before execution

## Using the Plugin Context

Tools receive the full `PluginContext`:

```typescript
execute: async (params, ctx) => {
  // Access config
  const apiKey = ctx.config.apiKey;

  // Log activity
  ctx.logger.info('Fetching data', { query: params.query });

  // Read/write secrets
  const token = await ctx.secrets.get('oauth_token');

  // Emit events for workflow triggers
  await ctx.emitEvent('item.created', { id: result.id });

  // Push messages into the connector pipeline
  await ctx.emitMessage({
    source: `myapp:${result.id}`,
    channel: 'notifications',
    content: `New item created: ${result.title}`,
  });

  return { content: result };
},
```

## Error Handling

Throw errors for unrecoverable failures — the host will capture them and report to the agent:

```typescript
execute: async (params, ctx) => {
  const res = await fetch(url, { headers: { Authorization: `Bearer ${ctx.config.token}` } });

  if (res.status === 401) {
    throw new Error('Authentication failed. Check your API key in plugin settings.');
  }
  if (res.status === 429) {
    return { content: 'Rate limited. Try again in a few minutes.', isError: true };
  }
  if (!res.ok) {
    throw new Error(`API error: ${res.status} ${res.statusText}`);
  }

  return { content: await res.json() };
},
```

Use `isError: true` for soft errors the agent can work around. Throw for hard errors.
