# Cookbook: REST API CRUD Tools

A pattern for wrapping a standard REST API as plugin tools.

## Pattern

```typescript
import { definePlugin, z, type ToolDefinition } from '@hivemind/plugin-sdk';

// Reusable API helper
async function api(ctx: any, path: string, opts?: { method?: string; body?: unknown }) {
  const res = await fetch(`${ctx.config.baseUrl}${path}`, {
    method: opts?.method ?? 'GET',
    headers: {
      Authorization: `Bearer ${ctx.config.apiKey}`,
      'Content-Type': 'application/json',
    },
    body: opts?.body ? JSON.stringify(opts.body) : undefined,
  });
  if (!res.ok) throw new Error(`API error: ${res.status}`);
  return res.json();
}

// List tool (read-only)
const listItems: ToolDefinition = {
  name: 'list_items',
  description: 'List items with optional filters',
  parameters: z.object({
    status: z.enum(['active', 'archived', 'all']).default('active'),
    limit: z.number().min(1).max(100).default(20),
  }),
  execute: async (params, ctx) => {
    const items = await api(ctx, `/items?status=${params.status}&limit=${params.limit}`);
    return { content: items };
  },
};

// Get tool (read-only)
const getItem: ToolDefinition = {
  name: 'get_item',
  description: 'Get a single item by ID',
  parameters: z.object({
    id: z.string().describe('Item ID'),
  }),
  execute: async (params, ctx) => {
    return { content: await api(ctx, `/items/${params.id}`) };
  },
};

// Create tool (side-effect)
const createItem: ToolDefinition = {
  name: 'create_item',
  description: 'Create a new item',
  parameters: z.object({
    title: z.string(),
    description: z.string().optional(),
  }),
  annotations: { sideEffects: true, approval: 'suggest' },
  execute: async (params, ctx) => {
    const item = await api(ctx, '/items', { method: 'POST', body: params });
    await ctx.emitEvent('item.created', { id: item.id });
    return { content: item };
  },
};

// Update tool (side-effect)
const updateItem: ToolDefinition = {
  name: 'update_item',
  description: 'Update an existing item',
  parameters: z.object({
    id: z.string(),
    title: z.string().optional(),
    status: z.enum(['active', 'archived']).optional(),
  }),
  annotations: { sideEffects: true, approval: 'suggest' },
  execute: async (params, ctx) => {
    const { id, ...body } = params;
    return { content: await api(ctx, `/items/${id}`, { method: 'PATCH', body }) };
  },
};

// Delete tool (side-effect, always require approval)
const deleteItem: ToolDefinition = {
  name: 'delete_item',
  description: 'Delete an item permanently',
  parameters: z.object({
    id: z.string().describe('Item ID to delete'),
  }),
  annotations: { sideEffects: true, approval: 'always' },
  execute: async (params, ctx) => {
    await api(ctx, `/items/${params.id}`, { method: 'DELETE' });
    return { content: `Item ${params.id} deleted` };
  },
};

export default definePlugin({
  configSchema: z.object({
    apiKey: z.string().secret().label('API Key').section('Auth'),
    baseUrl: z.string().default('https://api.myservice.com').label('Base URL'),
  }),
  tools: [listItems, getItem, createItem, updateItem, deleteItem],
});
```

## Key Points

- **Read tools** have no annotations (safe to auto-execute)
- **Write tools** use `sideEffects: true` + `approval: 'suggest'`
- **Destructive tools** use `approval: 'always'`
- **Emit events** after mutations for workflow triggers
- **Reusable API helper** reduces boilerplate
