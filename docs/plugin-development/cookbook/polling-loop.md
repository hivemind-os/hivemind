# Cookbook: Efficient Polling Loop

A pattern for reliable background polling with deduplication, cursor persistence, error resilience, and status reporting.

## Pattern

```typescript
import type { PluginContext } from '@hivemind/plugin-sdk';

export async function pollLoop(ctx: PluginContext): Promise<void> {
  const pollIntervalMs = (ctx.config.pollInterval as number) * 1000;

  // Restore cursor from persistent store (survives restarts)
  let cursor = await ctx.store.get<string>('pollCursor')
    ?? new Date().toISOString();

  let consecutiveErrors = 0;

  await ctx.updateStatus({ state: 'syncing', message: 'Starting sync...' });

  while (!ctx.signal.aborted) {
    try {
      // ── Fetch updates since cursor ──────────────────────
      const response = await fetchUpdates(ctx.config, cursor);

      // ── Emit each update as a message ───────────────────
      for (const item of response.items) {
        await ctx.emitMessage({
          // Dedup key: includes updated_at so re-edits are delivered
          source: `myservice:${item.type}:${item.id}:${item.updatedAt}`,
          channel: `myservice/${ctx.config.project}`,
          content: formatItem(item),
          sender: {
            id: item.author.id,
            name: item.author.name,
          },
          metadata: {
            type: item.type,
            id: item.id,
            url: item.url,
            priority: item.priority,
          },
        });

        // Also emit as workflow event
        await ctx.emitEvent(`myservice.${item.type}_updated`, {
          id: item.id,
          url: item.url,
        });
      }

      // ── Update cursor ───────────────────────────────────
      if (response.nextCursor) {
        cursor = response.nextCursor;
        await ctx.store.set('pollCursor', cursor);
      }

      // ── Report success ──────────────────────────────────
      consecutiveErrors = 0;
      await ctx.updateStatus({
        state: 'connected',
        message: `Synced ${response.items.length} items at ${new Date().toLocaleTimeString()}`,
      });

    } catch (err) {
      // ── Handle errors with backoff ──────────────────────
      consecutiveErrors++;
      ctx.logger.error('Poll failed', {
        error: String(err),
        consecutiveErrors,
      });

      await ctx.updateStatus({
        state: 'error',
        message: `Sync failed (attempt ${consecutiveErrors}): ${err.message}`,
      });

      // Notify user after 3 consecutive failures
      if (consecutiveErrors === 3) {
        await ctx.notify({
          title: 'Sync Error',
          body: `Failed to sync after ${consecutiveErrors} attempts: ${err.message}`,
          action: { type: 'open_settings', target: 'plugins' },
        });
      }

      // Exponential backoff: 10s, 20s, 40s, 80s, max 5min
      const backoffMs = Math.min(300_000, 10_000 * Math.pow(2, consecutiveErrors - 1));
      await ctx.sleep(backoffMs);
      continue; // skip the normal sleep
    }

    // ── Normal poll interval ──────────────────────────────
    await ctx.sleep(pollIntervalMs);
  }

  await ctx.updateStatus({ state: 'disconnected', message: 'Loop stopped' });
}
```

## Key Points

- **Cursor persistence** — `ctx.store.set('pollCursor', cursor)` survives restarts
- **Dedup-friendly source** — include `updatedAt` so re-edits are delivered
- **Exponential backoff** — don't hammer the API on errors
- **Status updates** — user sees real-time sync status in Settings
- **User notification** — alert after repeated failures
- **Event emission** — enables workflow automations
