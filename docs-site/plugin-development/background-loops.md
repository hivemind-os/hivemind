# Background Loops

Plugins can run a long-lived background loop that polls external services and pushes incoming messages into the Hivemind connector pipeline.

## Basic Loop

```typescript
loop: async (ctx) => {
  while (!ctx.signal.aborted) {
    const updates = await fetchUpdates();

    for (const update of updates) {
      await ctx.emitMessage({
        source: `myapp:${update.id}`,
        channel: 'updates',
        content: update.text,
      });
    }

    await ctx.sleep(60_000); // 60 seconds
  }
},
```

## Key Concepts

### Cancellation

The loop receives `ctx.signal` (an `AbortSignal`). Always:
- Check `ctx.signal.aborted` in your while condition
- Use `ctx.sleep()` instead of `setTimeout` — it's cancellation-aware
- Catch `AbortError` if you need cleanup

```typescript
loop: async (ctx) => {
  try {
    while (!ctx.signal.aborted) {
      await doWork(ctx);
      await ctx.sleep(30_000);
    }
  } catch (err) {
    if (err.name === 'AbortError') return; // normal shutdown
    throw err; // unexpected error
  }
},
```

### Message Deduplication

The `source` field on messages is the dedup key. Messages with the same source won't be delivered twice:

```typescript
await ctx.emitMessage({
  source: `github:issue:123:2024-01-15T10:00:00Z`,  // unique per update
  channel: 'github/myrepo',
  content: 'Issue updated: Fix login bug',
});
```

Good source patterns:
- `{service}:{type}:{id}:{updated_at}` — delivers when updated
- `{service}:{type}:{id}` — delivers once per unique item
- `{service}:{type}:{id}:v{version}` — delivers on version change

### Sync Cursors

Use `ctx.store` to persist your sync position across restarts:

```typescript
loop: async (ctx) => {
  let cursor = await ctx.store.get<string>('syncCursor');

  while (!ctx.signal.aborted) {
    const { items, nextCursor } = await api.getUpdates(cursor);

    for (const item of items) {
      await ctx.emitMessage({ ... });
    }

    if (nextCursor) {
      cursor = nextCursor;
      await ctx.store.set('syncCursor', cursor);
    }

    await ctx.sleep(ctx.config.pollInterval * 1000);
  }
},
```

### Status Updates

Keep the user informed about sync progress:

```typescript
loop: async (ctx) => {
  await ctx.updateStatus({ state: 'syncing', message: 'Starting...' });

  while (!ctx.signal.aborted) {
    try {
      const items = await sync(ctx);
      await ctx.updateStatus({
        state: 'connected',
        message: `Synced ${items.length} items at ${new Date().toLocaleTimeString()}`,
      });
    } catch (err) {
      await ctx.updateStatus({
        state: 'error',
        message: `Sync failed: ${err.message}`,
      });
    }

    await ctx.sleep(60_000);
  }

  await ctx.updateStatus({ state: 'disconnected' });
},
```

### Error Resilience

Don't let transient errors kill the loop:

```typescript
loop: async (ctx) => {
  let consecutiveErrors = 0;

  while (!ctx.signal.aborted) {
    try {
      await syncOnce(ctx);
      consecutiveErrors = 0;
    } catch (err) {
      consecutiveErrors++;
      ctx.logger.error('Sync failed', { error: String(err), consecutiveErrors });

      // Exponential backoff on repeated failures
      const backoff = Math.min(300_000, 10_000 * Math.pow(2, consecutiveErrors));
      await ctx.sleep(backoff);
      continue;
    }

    await ctx.sleep(ctx.config.pollInterval * 1000);
  }
},
```

## Incoming Message Format

```typescript
await ctx.emitMessage({
  source: 'unique-dedup-key',              // required — deduplication
  channel: 'feed-name',                     // required — routing
  content: 'Human-readable message text',   // required

  sender: {                                 // optional
    id: 'user-123',
    name: 'Jane Smith',
    avatarUrl: 'https://...',
  },

  metadata: {                               // optional — for workflow triggers
    type: 'issue',
    priority: 'high',
    url: 'https://...',
  },

  threadId: 'thread-abc',                   // optional — conversation threading
  classification: 'work',                   // optional — personal/work/automated/spam
  timestamp: '2024-01-15T10:00:00Z',       // optional — defaults to now

  attachments: [{                           // optional
    name: 'screenshot.png',
    mimeType: 'image/png',
    content: 'base64...',
  }],
});
```
