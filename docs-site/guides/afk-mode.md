# AFK Mode

AFK (Away From Keyboard) mode lets HiveMind OS keep working while you're away. When agents need your approval or have questions, those requests are forwarded to a channel you choose — Slack, Discord, or email — so you can respond from your phone or another device.

## How It Works

HiveMind OS tracks your presence through four status levels:

| Status | Meaning |
|--------|---------|
| **Active** | You're at your desk. Approvals and questions appear in the desktop app. |
| **Idle** | No mouse or keyboard activity for a while. Behaviour is the same as Active by default. |
| **Away** | Extended inactivity or manually set. Requests are forwarded to your configured channel. |
| **Do Not Disturb** | Manually set. Requests are forwarded, same as Away. |

### Automatic Transitions

The desktop app sends a heartbeat whenever it detects mouse movement, keyboard input, or clicks. Based on inactivity:

1. **Active → Idle** after the configured idle threshold (optional).
2. **Idle → Away** after the configured away threshold (optional).
3. Activity resumes → back to **Active** automatically.

If no desktop client connects at all (e.g. the daemon is running headless), the system transitions to **Away** after a grace period (default: 5 minutes).

You can always override the status manually using the status indicator in the top-right corner of the desktop app.

## Setting Up Forwarding

1. Open **Settings** and find the **AFK** section.
2. Choose a **forwarding channel** — this is a connector channel (e.g. a Slack DM, Discord channel, or email address) where requests will be sent.
3. Select what to forward:
   - **Approvals** — tool-use permission requests from agents.
   - **Questions** — agents asking for clarification or input.
4. Choose which statuses trigger forwarding. By default, forwarding activates when you're **Away** or **Do Not Disturb**.

::: tip
You need at least one connector configured (Slack, Discord, or email) before you can set up AFK forwarding. See [Connectors](/guides/messaging-bridges) for setup instructions.
:::

## Responding Remotely

When a request is forwarded, you can respond directly from the channel:

- **Slack** — use the interactive buttons on the forwarded message to approve, deny, or type a reply.
- **Discord** — use the interaction buttons or reply to the message.
- **Email** — reply to the forwarded email with your answer.

Your response is routed back to the agent that asked, and it continues working as if you'd answered in the desktop app.

## Auto-Approve Timeout

For approvals that shouldn't block agents indefinitely, you can set an **auto-approve timeout**. If you don't respond within the configured time, the request is automatically approved.

::: warning
Use auto-approve with care. It grants agents permission to run tools without your explicit review. Consider combining it with [Security Policies](/guides/security-policies) to limit what can be auto-approved.
:::

## Configuration Reference

All AFK settings live under the `afk` key in your configuration:

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `forward_on` | list | `[Away, DoNotDisturb]` | Which statuses trigger forwarding |
| `forward_channel_id` | string | — | Connector channel ID to forward requests to |
| `forward_to_address` | string | — | Email address to forward requests to (alternative to channel) |
| `forward_approvals` | bool | `true` | Forward tool-approval requests |
| `forward_questions` | bool | `true` | Forward agent questions |
| `auto_idle_after_secs` | number | — | Seconds of inactivity before transitioning to Idle |
| `auto_away_after_secs` | number | — | Seconds of inactivity before transitioning to Away |
| `auto_approve_on_timeout_secs` | number | — | Auto-approve approvals after this many seconds |
| `no_client_grace_period_secs` | number | `300` | Seconds to wait for a desktop client before transitioning to Away |

Settings with no default (—) are disabled until configured.

## API

You can also manage AFK status programmatically:

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/status` | Get current user status |
| `PUT /api/v1/status` | Set user status manually |
| `POST /api/v1/status/heartbeat` | Record activity heartbeat |
| `GET /api/v1/status/events` | Subscribe to status change events |

## Learn More

- [Connectors](/guides/messaging-bridges) — set up Slack, Discord, or email connectors for forwarding
- [Security Policies](/guides/security-policies) — control what agents can do without approval
- [Workflows](/guides/workflows) — agent approvals and questions often come from workflow steps
