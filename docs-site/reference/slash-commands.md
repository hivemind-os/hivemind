# Slash Commands

Slash commands are shortcuts you can type directly in the HiveMind OS chat input. Press `/` to see available commands.

## Command Reference

| Command | Alias | Description | Example |
|---|---|---|---|
| `/prompt` | `/p` | Invoke a persona's prompt template | `/prompt audit-directory` |

## Details

### /prompt

Invoke a reusable prompt template defined on the active persona (or a specific persona). Prompt templates are Handlebars templates with optional parameters — they let you create repeatable, parameterized interactions.

```
/prompt audit-directory
/p audit-directory
```

When invoked, HiveMind OS looks up the template by ID on the current persona, renders any parameters (prompting you to fill them in if needed), and sends the result as your message.

::: tip
Define prompt templates in your persona YAML under the `prompts` field. Each template has an `id`, `name`, `template` (Handlebars string), and optional `input_schema` for parameters. See the [Prompt Templates Reference](/reference/prompt-templates) for full syntax details.
:::

## Other Capabilities

Most interactions with HiveMind OS happen through **natural language conversation** rather than slash commands. The agent has access to built-in tools for:

- **Knowledge management** — ask the agent to remember facts or query stored knowledge (tool: `knowledge.query`)
- **File operations** — read, write, search, and list files (tools: `filesystem.read`, `filesystem.write`, `filesystem.search`, `filesystem.glob`, etc.)
- **Shell commands** — execute commands in the terminal (tool: `shell.execute`)
- **Web requests** — fetch web pages and APIs (tool: `http.request`)
- **Agent coordination** — spawn, signal, and manage sub-agents (tools: `core.spawn_agent`, `core.signal_agent`, etc.)
- **Workflows** — launch and manage workflows (tools: `workflow.launch`, `workflow.status`)
- **Scheduling** — create scheduled tasks through conversation
- **Communication** — send messages through connected channels (tool: `comm.send_external_message`)

Simply describe what you need in plain language, and the agent will use the appropriate tools.

## See Also

- [Keyboard Shortcuts](./keyboard-shortcuts.md)
- [Personas Guide](/guides/personas) — defining prompt templates on personas
