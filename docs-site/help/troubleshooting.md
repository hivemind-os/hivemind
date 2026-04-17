# Troubleshooting

Solutions to common issues in HiveMind OS. If your problem isn't listed here, check the [FAQ](./faq.md) or [file a bug report](https://github.com/hivemind-os/hivemind/issues).

## 1. Daemon Not Starting

**Symptoms:** CLI commands fail with "connection refused" or "daemon not running."

**Fixes:**
- Check the system tray — the HiveMind OS icon should be visible. Click it and select **Start Daemon**.
- Restart manually:
  ```bash
  hive daemon stop
  hive daemon start
  ```
- Check logs for errors:
  ```bash
  cat ~/.hivemind/logs/daemon.log | tail -50
  ```
- Ensure port `9180` is not in use by another process.

::: warning
On Linux, if the daemon crashes at startup, verify that your `DISPLAY` or `WAYLAND_DISPLAY` environment variable is set correctly.
:::

## 2. "No Secret Found" After Setting Up a Provider

**Symptoms:** You added a provider API key in Settings → Providers, but using it gives:
```
no secret found in OS keyring for key `provider:...:api-key`
```

**Fixes:**
- **Save your config after adding the key.** The daemon reloads secrets when the configuration is saved. Click **Save** in Settings after entering your API key.
- If you just added the key, try switching to a chat session and sending a message — the config save triggers a reload automatically.
- As a last resort, restart the daemon from the system tray or via `hive daemon stop && hive daemon start`.

## 3. Connector "No OAuth Tokens" After Authorization

**Symptoms:** You authorized a connector (e.g., Microsoft 365) successfully and the test passed, but the agent reports:
```
no OAuth tokens for connector 'microsoft'. Please re-authorize via Settings → Connectors.
```

**Fixes:**
- **Re-authorize** from Settings → Connectors — click the Authorize button again.
- Ensure you're running the latest version. Earlier releases had a bug where secrets could be lost when saving other settings.
- Restart the daemon: `hive daemon stop && hive daemon start`, then re-authorize.

## 4. Provider Connection Failed

**Symptoms:** "Provider unreachable" or "401 Unauthorized" errors.

**Fixes:**
- Verify your API key is set correctly in **Settings → Providers**.
- Check that environment variables are loaded (if using `env:VAR_NAME` in provider auth config).
- For Ollama, ensure the server is running: `ollama serve`.

## 5. MCP Server Not Connecting

**Symptoms:** Tools from an MCP server are unavailable or show "disconnected."

**Fixes:**
- Verify the command path exists and is executable:
  ```bash
  which npx  # or the command in your config
  ```
- Check the MCP server config in your persona's `mcp_servers` section (visible in **Settings → Personas**).
- Restart the connection from the persona's MCP Servers section in **Settings → Personas**.
- Check MCP logs:
  ```bash
  cat ~/.hivemind/logs/mcp-*.log | tail -30
  ```

## 6. Agent Seems Stuck

**Symptoms:** The agent shows a spinner but produces no output for a long time.

**Fixes:**
- **Check the tool approval queue** — the agent may be waiting for you to approve a tool call. Look for a pending approval badge in the UI.
- Press `Ctrl+C` to stop the current agent and retry.
- If the agent loops, try simplifying your request or switching to a different model.

::: tip
Configure `security.default_permissions` in your config to pre-approve trusted tools and avoid blocking on approvals. Permission rules specify tool patterns with `auto`, `ask`, or `deny` actions.
:::

## 7. Knowledge Not Being Recalled

**Symptoms:** Asking the agent about previously saved information returns no results.

**Fixes:**
- Try broader or different search terms when asking the agent about stored knowledge.
- Use the **Knowledge Explorer** in the UI to browse and search saved knowledge.
- Check that the knowledge entry was saved — open the Knowledge panel and search for it.

## 8. High Memory Usage

**Symptoms:** HiveMind OS uses excessive RAM, causing system slowdowns.

**Fixes:**
- Use smaller local models (e.g., `llama3:8b` instead of `llama3:70b`).
- Close unused chat sessions — each session holds context in memory.
- Restart the daemon to clear accumulated state:
  ```bash
  hive daemon stop && hive daemon start
  ```

## 9. Bot Not Responding

**Symptoms:** A launched bot doesn't reply to messages or seems idle.

**Fixes:**
- Check bot status in the **Bots** page in the UI.
- Verify the persona configuration is valid — a missing or broken system prompt can cause silent failures.
- Check that the persona's assigned provider is connected in **Settings → Providers**.
- Stop and relaunch the bot from the Bots page.

## 10. Workflow Not Triggering

**Symptoms:** A workflow that should run automatically never starts.

**Fixes:**
- Check the workflow configuration in the **Workflows** page in the UI (workflows are managed through the UI/API, not config files).
- For event-based triggers, verify the event source is connected and emitting events.
- Try running the workflow manually from the **Workflows** page in the UI.
- Check workflow run history on the Workflows page for errors.

::: tip
Run `hive daemon status` to confirm the daemon is running and healthy.
:::

## See Also

- [FAQ](./faq.md) — common questions about HiveMind OS
- [CLI Commands](../cli/commands.md) — full command reference
- [Configuration Reference](../reference/configuration.md) — config schema and examples
