# Agent Kits

Agent Kits let you bundle personas, workflows, skills, and attachments into a portable `.agentkit` file. Use them to share complete agent configurations with your team, back up your setup, or move agents between environments.

::: tip
An `.agentkit` file is a self-contained ZIP archive. Share it via Git, email, or a shared drive — no server connection required.
:::

## What's Inside a Kit

| Included | Not Included |
|----------|--------------|
| Personas (YAML + settings) | Knowledge graph data |
| Workflows (definitions + attachments) | Conversation history |
| Skills (Markdown skill files) | Connector credentials |
| File attachments (PDFs, docs, etc.) | Running workflow instances |

The archive contains a `manifest.json` describing every item, plus the raw files for each persona and workflow.

## Exporting a Kit

1. Open **Agent Kits** from the sidebar.
2. Switch to the **Export** tab.
3. Enter a **kit name** and optional **description**.
4. Select the **personas** and **workflows** you want to include. Only user-created items appear — `system/*` items are excluded.
5. Click **Export**. A save dialog opens for the `.agentkit` file.

::: tip
Skills are exported automatically with their parent persona — you don't need to select them separately.
:::

## Importing a Kit

Importing is a two-step process: **preview** first, then **apply**.

### Step 1 — Preview

1. Open **Agent Kits** from the sidebar.
2. Switch to the **Import** tab.
3. Select an `.agentkit` file from your file system.
4. Enter a **target namespace** (e.g. `myteam`). This determines where the imported items will live.
5. Click **Preview Import**.

The preview shows you:

- **Items to import** — each persona and workflow with its new namespaced ID.
- **Overwrites** — items that already exist under the target namespace and will be replaced.
- **Warnings** — cross-references to personas or workflows that are not included in the kit.
- **Errors** — issues that prevent import (e.g. trying to import into the `system/` namespace).

### Step 2 — Apply

1. Review the preview and use the checkboxes to select which items to import. All items are selected by default.
2. Click **Import** to apply.
3. A summary shows what was imported, what was skipped, and any per-item errors.

## Namespace Remapping

When you import a kit, every item's root namespace is replaced with your chosen target namespace. The rest of the path stays the same.

| Original ID | Target Namespace | New ID |
|---|---|---|
| `acme/sales-bot` | `myteam` | `myteam/sales-bot` |
| `acme/sub/helper` | `myteam` | `myteam/sub/helper` |
| `vendor/lead-flow` | `myteam` | `myteam/lead-flow` |

### Cross-Reference Rewriting

Workflows often reference personas or other workflows. During import, these references are automatically updated to match the new namespace:

- **`persona_id`** fields (e.g. in `invoke_agent` steps)
- **`workflow_name`** fields (e.g. in `launch_workflow` steps)
- **`definition`** fields (e.g. in `schedule_task` steps)

References to items **outside** the kit are left unchanged and flagged as warnings in the preview.

## Overwriting Existing Items

If an item with the same namespaced ID already exists:

- The preview marks it as an **overwrite**.
- You can deselect it to skip the import for that item.
- If you proceed, the existing item is replaced with the imported version.

## API Reference

Agent Kits can also be managed programmatically:

| Endpoint | Description |
|---|---|
| `POST /api/v1/agent-kits/export` | Create an `.agentkit` archive from selected personas and workflows |
| `POST /api/v1/agent-kits/preview` | Preview what an import would do (no side effects) |
| `POST /api/v1/agent-kits/import` | Apply an import after preview |

All endpoints accept and return JSON. The archive content is transferred as a base64-encoded string.

## Learn More

- [Personas Guide](/guides/personas) — creating and configuring personas
- [Workflows Guide](/guides/workflows) — building workflow automations
- [Skills Guide](/guides/skills) — adding skills to personas
- [Knowledge Management](/guides/knowledge-management) — managing knowledge (not included in kits)
