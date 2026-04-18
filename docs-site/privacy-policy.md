# Privacy Policy

**Effective Date:** April 17, 2026
**Last Updated:** April 17, 2026

HiveMind OS is an open-source, privacy-first AI agent platform that runs entirely on your machine. This Privacy Policy explains how HiveMind OS ("the Software"), developed by HiveMind OS Contributors ("we", "us", "our"), handles your data — including data accessed through third-party services like Google.

## Core Privacy Principle

HiveMind OS is a **local-first application**. Your data is stored on your device, processed on your device, and stays under your control. We do not operate servers that collect, store, or process your personal data.

## What Data HiveMind OS Accesses

When you connect third-party services through HiveMind OS, the application accesses data from those services **on your behalf and at your direction**. This may include:

### Google Services

When you connect your Google account, HiveMind OS may request access to the following scopes depending on which features you enable:

| Feature | Scopes | What It Accesses |
|---------|--------|------------------|
| **Authentication** | `openid`, `email`, `profile` | Your basic profile information (name, email address) to identify your account |
| **Gmail** | `gmail.modify`, `gmail.send` | Read, draft, send, and organize your email messages |
| **Google Calendar** | `calendar` | Read and create calendar events |
| **Google Drive** | `drive` | Read and manage files in your Google Drive |
| **Google Contacts** | `contacts.readonly` | Read your contacts (read-only — no modifications) |

You choose which features to enable when configuring the Google connector. Only the scopes required for your selected features are requested.

### Other Connected Services

HiveMind OS supports connectors for Microsoft 365, IMAP email, Slack, Discord, Apple, and other services. Each connector accesses only the data required for the features you enable.

### AI Provider Services

When you configure an AI provider (Anthropic, OpenAI-compatible, GitHub Copilot, Azure AI Foundry, or Ollama), your prompts and conversations are sent to that provider's API for processing. Each provider has its own privacy policy governing how it handles that data. When using Ollama, all AI processing happens locally on your machine.

## How Your Data Is Used

HiveMind OS uses the data it accesses **solely to perform the tasks you direct it to perform**. For example:

- Reading your email to summarize or draft replies
- Checking your calendar to prepare meeting briefs
- Searching your contacts to find recipients
- Reading Drive files you reference in conversations

**We do not:**

- Collect, transmit, or store your data on any external server we operate
- Use your data for advertising, analytics, or profiling
- Sell, share, or license your data to third parties
- Train AI models on your data

## Where Your Data Is Stored

All data accessed by HiveMind OS is stored **locally on your machine**:

- Conversations, workflow state, and configuration are stored in local SQLite databases under `~/.hivemind/`
- OAuth tokens for connected services are stored locally in your operating system's secure credential storage
- No data is replicated to cloud services we operate

The only external data transfers occur when:

1. **You send a prompt to an AI provider** — the prompt and context are sent to the provider's API
2. **You use a connector to send a message** — the message is sent through the connected service (e.g., Gmail sends an email)
3. **You use web search** — search queries are sent to the search provider

These transfers happen only at your direction, as part of tasks you initiate.

## Data Classification and Protection

HiveMind OS includes a built-in [data classification system](/concepts/privacy-and-security) that labels content by sensitivity level (Public, Internal, Confidential, Restricted) and enforces policies about which data can be sent to which destinations. This gives you fine-grained control over what information leaves your machine.

## Data Retention and Deletion

Since all data is stored locally on your machine:

- **You control retention** — data exists as long as you keep it
- **Deleting the app or its data directory removes all stored data**
- **Revoking a connector's OAuth access** removes HiveMind OS's ability to access that service — you can also revoke access directly from your Google Account at [myaccount.google.com/permissions](https://myaccount.google.com/permissions)
- **No server-side data to delete** — we don't store your data, so there's nothing for us to purge

## Third-Party Services

HiveMind OS integrates with third-party services that have their own privacy policies. When you connect a service, that service's privacy policy governs how it handles your data on its side. Key third-party policies:

- [Google Privacy Policy](https://policies.google.com/privacy)
- [Microsoft Privacy Statement](https://privacy.microsoft.com/privacystatement)
- [Anthropic Privacy Policy](https://www.anthropic.com/privacy)
- [OpenAI Privacy Policy](https://openai.com/policies/privacy-policy)
- [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Slack Privacy Policy](https://slack.com/trust/privacy/privacy-policy)
- [Discord Privacy Policy](https://discord.com/privacy)

## Google API Services User Data Policy

HiveMind OS's use and transfer to any other app of information received from Google APIs will adhere to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

## Children's Privacy

HiveMind OS is not directed at children under the age of 13. We do not knowingly collect personal information from children.

## Changes to This Policy

We may update this Privacy Policy from time to time. Changes will be posted to this page with an updated "Last Updated" date. Since HiveMind OS is open source, all changes are tracked in the project's [public repository](https://github.com/hivemind-os/hivemind).

## Contact Us

If you have questions about this Privacy Policy or HiveMind OS's data practices, contact us at:

**Email:** [danielgerlag@gmail.com](mailto:danielgerlag@gmail.com)
**GitHub:** [github.com/hivemind-os/hivemind](https://github.com/hivemind-os/hivemind)
