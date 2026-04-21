# Connectors

Connectors let HiveMind OS integrate with external communication platforms, calendars, cloud drives, and contacts — turning your agent into a true productivity hub. Each connector runs inside the daemon and provides services that agents, workflows, and bots can use.

## Supported Connectors

| Provider | Services | Auth Type |
|---|---|---|
| **Microsoft 365** | Email, Calendar, Drive, Contacts | OAuth2 |
| **Gmail / Google** | Email, Calendar, Drive, Contacts | OAuth2 |
| **IMAP / SMTP** | Email | Password |
| **Slack** | Messaging | BotToken |
| **Discord** | Messaging | BotToken |
| **Apple** | Calendar, Contacts | Local |
| **Coinbase** | Trading (Advanced Trade API) | CdpApiKey |

## Auth Configuration

Each provider uses a specific `AuthConfig` variant:

### OAuth2 (Microsoft, Gmail)

```yaml
auth:
  type: oauth2
  client_id: env:CLIENT_ID
  client_secret: env:CLIENT_SECRET
  refresh_token: env:REFRESH_TOKEN
  access_token: env:ACCESS_TOKEN       # optional, auto-refreshed
  token_url: https://oauth2.example.com/token  # optional
```

### Password (IMAP/SMTP)

```yaml
auth:
  type: password
  username: env:IMAP_USER
  password: env:IMAP_PASS
  imap_host: imap.example.com
  imap_port: 993
  smtp_host: smtp.example.com
  smtp_port: 587
  smtp_encryption: starttls
```

### BotToken (Slack, Discord)

```yaml
auth:
  type: bot_token
  bot_token: env:BOT_TOKEN
  app_token: env:APP_TOKEN   # optional, used for Slack Socket Mode
```

### CdpApiKey (Coinbase)

```yaml
auth:
  type: cdp_api_key
  key_name: env:CDP_KEY_NAME
  private_key: env:CDP_PRIVATE_KEY
```

### Local (Apple)

```yaml
auth:
  type: local
```

## Setting Up a Connector

1. Open **Settings → Connectors → Add Connector**
2. Pick a provider (e.g. Gmail, Microsoft, Slack)
3. Authenticate — OAuth2 providers open a browser flow, IMAP asks for credentials
4. Enable the services you want (communication, calendar, drive, contacts)
5. Optionally restrict which **personas** can access this connector

### Example: Gmail Connector

```yaml
connectors:
  - id: my-gmail
    name: Personal Gmail
    provider: gmail
    auth:
      type: oauth2
      client_id: env:GMAIL_CLIENT_ID
      client_secret: env:GMAIL_CLIENT_SECRET
      refresh_token: env:GMAIL_REFRESH_TOKEN
    services:
      communication: true
      calendar: true
      drive: true
      contacts: true
    allowed_personas:
      - user/support-agent
      - user/scheduler
```

### Example: Slack Connector

```yaml
connectors:
  - id: work-slack
    name: Work Slack
    provider: slack
    auth:
      type: bot_token
      bot_token: env:SLACK_BOT_TOKEN
      app_token: env:SLACK_APP_TOKEN
    services:
      communication: true
```

### Example: IMAP/SMTP (Generic Email)

```yaml
connectors:
  - id: work-email
    name: Work Email (IMAP)
    provider: imap
    auth:
      type: password
      username: env:IMAP_USER
      password: env:IMAP_PASS
      imap_host: imap.example.com
      imap_port: 993
      smtp_host: smtp.example.com
      smtp_port: 587
      smtp_encryption: starttls
    services:
      communication: true
```

## Persona Scoping

Connectors can be restricted to specific personas using `allowed_personas`. This means:
- A `user/support-agent` persona can read and send emails
- A `user/researcher` persona can't — it never sees the connector's tools

If `allowed_personas` is empty, no persona has access by default — you must explicitly grant it.

::: warning
Be intentional about which personas can access which connectors. A persona with email access can read and send messages on your behalf.
:::

## Classification on Connectors

Connectors are subject to the same [data classification](/concepts/privacy-and-security) system as everything else. Outbound messages pass through the classification gate — if an agent tries to send `CONFIDENTIAL` data through a connector classified as `PUBLIC`, the override policy kicks in (block, prompt, or redact).

## What Connectors Enable

With connectors configured, your agents can:
- **Read and reply to emails** — trigger workflows on incoming messages
- **Schedule and manage calendar events** — create meetings, check availability
- **Browse and manage files** — read from Google Drive or OneDrive
- **Send Slack/Discord messages** — post updates, respond to team members
- **Look up contacts** — find email addresses, phone numbers

These capabilities are exposed as tools that any persona (with access) can use, and as workflow triggers (e.g. `incoming_message` trigger type).

## Building Custom Connectors

The built-in connectors above cover common services, but you can build your own using the **Hivemind Plugin SDK**. Connector plugins are TypeScript packages that provide:

- **Custom tools** — expose any API as agent-callable functions
- **Configuration UI** — Zod-based schemas render settings forms automatically
- **Background loops** — poll external services and push messages into Hivemind
- **Lifecycle hooks** — validate credentials on activation, clean up on deactivation

Plugins run as isolated Node.js processes and communicate with the host over JSON-RPC. The protocol is a superset of MCP, so every connector plugin is also a valid MCP tool server.

::: tip Get started
See the [Plugin Development Guide](/plugin-development/) to build your first connector plugin in 5 minutes, or jump straight to the [Quick Start](/plugin-development/quick-start).
:::

## Learn More

- [Workflows Guide](/guides/workflows) — Use `incoming_message` triggers with connectors
- [Personas Guide](/guides/personas) — Scoping connector access per persona
- [Security Policies](/guides/security-policies) — Classification and override policies
