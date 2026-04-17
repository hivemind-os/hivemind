# No-Code Guide

Everything in HiveMind OS can be done from the desktop app — no terminal, no config files, no coding. This guide walks you through every feature using just the UI.

Whether you're a small business owner, a team lead, or someone who just wants AI to handle the boring stuff, this page is for you. We'll cover setup, providers, personas, connectors, workflows, bots, and chat — all from the app window.


## Getting Set Up

When you first open HiveMind OS, a **Setup Wizard** walks you through everything you need. It takes about five minutes. You'll connect an AI provider, optionally add email or chat connectors, enable web search, pick your default model, and get a quick tour of the app. You can skip any step and come back to it later in **Settings**. For a detailed walkthrough of first launch, see the [Quickstart](/getting-started/quickstart).


## Adding AI Providers

Providers are the AI services that power your assistants. Go to **Settings → Providers** to add, edit, or remove them.

Each provider appears as a card. Click a card to configure it, or click **Add Provider** to set up a new one.

| Provider | What You Need | Best For |
|---|---|---|
| **Anthropic** | API key from [console.anthropic.com](https://console.anthropic.com) | High-quality reasoning, writing, and analysis |
| **OpenAI Compatible** | API key and endpoint URL | Works with OpenAI, Groq, Together, and other compatible services |
| **GitHub Copilot** | Sign in with your GitHub account (OAuth) | If you already pay for GitHub Copilot |
| **Azure AI Foundry** | Azure subscription and endpoint | Enterprise deployments with Azure compliance |
| **Ollama** | Install Ollama on your machine (no key needed) | Free, private, runs entirely on your computer |

To add a provider, click its card, enter the required credentials, and click **Save**. HiveMind OS will test the connection automatically.

::: tip Which provider should I choose?
- **Just want it to work?** → Anthropic or OpenAI Compatible (needs an API key)
- **Already use GitHub?** → GitHub Copilot (sign in with your GitHub account)
- **Want everything local?** → Ollama (free, runs on your machine, no API key needed)
:::


## Creating Personas

Personas are the AI assistants you talk to. Each one has its own personality, instructions, and capabilities. Go to **Settings → Personas** and click **New Persona**.

Here's what each field means:

- **Name** — What you call this assistant. Pick something descriptive, like "Customer Support" or "Content Writer."

- **Description** — A short note for yourself about what this persona does. Only you see this.

- **Avatar** — Pick an emoji to represent this persona in the chat list.

- **Color** — Choose a color for the chat bubble so you can tell personas apart at a glance.

- **System Prompt** — This is the most important field. It tells the AI *how to behave*. Write it like you're briefing a new employee. For example:

  > You are a friendly customer support agent for Riverstone Coffee Co. Always be empathetic and patient. Check the product manual before answering any question about our products. If you don't know the answer, say so honestly and offer to escalate to a human.

  Be specific. The more detail you put here, the better your assistant will perform.

- **Preferred Models** — Which AI model this persona should use. Leave it blank to use your default model. If you have multiple providers, you can pick a specific model here.

- **Allowed Tools** — Which tools (email, calendar, web search, etc.) this persona can access. Leave the defaults for most use cases. Restrict tools if you want a persona that only answers questions without taking actions.

### Quick Persona Recipes

Here are three personas you can create in a few minutes:

**Customer Support** — Handles emails with empathy and product knowledge. Set the system prompt to describe your company, your tone of voice, and your most common questions. Add your product manual as a knowledge attachment. Allow email tools so it can draft replies.

**Content Writer** — Drafts blog posts and social media in your brand voice. In the system prompt, describe your brand personality, target audience, and any style rules (word count, tone, topics to avoid). Allow web search so it can research topics.

**Research Assistant** — Finds information and summarizes it clearly. Keep the system prompt simple: tell it to be thorough, cite sources, and present findings in plain language. Allow web search and file tools.


## Connecting Email, Calendar, and Chat

Connectors let your AI read and send messages, check your calendar, and interact with your team's chat tools. Go to **Settings → Connectors**.

1. Click **Add Connector**
2. Choose your provider:
   - **Gmail** — Sign in with Google (OAuth). Gives access to email, calendar, and contacts.
   - **Microsoft 365** — Sign in with your Microsoft account (OAuth). Covers Outlook email, calendar, and OneDrive.
   - **IMAP** — Connect any email provider with server address, username, and password. Use this for custom or corporate email that isn't Gmail or Outlook.
   - **Slack** — Authorize with your Slack workspace. Your AI can read and post messages in channels.
   - **Discord** — Connect with a bot token. Useful for community management.
   - **Apple** — Connect your Apple account for iCloud mail and calendar.
   - **Coinbase** — Link your Coinbase account for crypto portfolio queries.
3. Follow the authorization flow. For Gmail and Microsoft 365, this opens a browser window where you sign in and grant permission. For IMAP, you enter your credentials directly.
4. Once connected, the connector appears in your list with a green status indicator. Your AI personas can now use it.

::: tip Start with email
Email is the most versatile connector. Once connected, you can auto-reply to customers, summarize your inbox, and trigger workflows from incoming messages.
:::


## Building Workflows (Visual Designer)

Workflows let you automate multi-step tasks. You build them visually — no code, no YAML. Go to **Workflows** in the sidebar.

### Create a New Workflow

1. Click **New Workflow**
2. Give it a name and description
3. Choose a **mode**:
   - **Background** — Runs automatically without your input. Great for automations like "summarize every new email."
   - **Chat** — Interactive. The workflow can ask you questions and wait for your input along the way.

### Pick a Trigger

The trigger decides *what starts* your workflow:

| Trigger | What It Does |
|---|---|
| **Manual** | You start it yourself by clicking a button |
| **Schedule** | Runs on a timer — daily, weekly, or a custom schedule |
| **Incoming Message** | Fires when a new email, Slack message, or other message arrives |
| **Event Pattern** | Fires when a specific pattern of events occurs |
| **MCP Notification** | Fires when an external tool sends a notification |

### Add Steps

Steps are the actions your workflow performs, in order:

- **Invoke Agent** — Have one of your personas do something. For example: "Read the incoming email and draft a polite reply."
- **Call Tool** — Perform a specific action like sending an email, creating a calendar event, or searching the web.
- **Feedback Gate** — Pause the workflow and ask *you* a question before continuing. Great for approvals ("Does this reply look good?").
- **Delay** — Wait a specific amount of time before moving on.

### Attachments

You can upload reference documents to your workflow — product manuals, style guides, FAQ sheets. The AI will consult these when performing its steps.

### Save and Activate

When you're happy with your workflow, save it and toggle it to **active**. It will start running based on its trigger.

### Launching a Workflow

To run a workflow you've already created:

1. Open **Workflows** in the sidebar
2. Find your workflow and click **Launch**
3. If the workflow asks for inputs (e.g., a project name or description), fill in the form that appears
4. Click **Run**

**Background workflows** start working on their own — you'll see a progress tracker on the Workflows page. **Chat workflows** are launched from the **Chat view** and run inside your conversation so you can interact with them.

Workflows with automatic triggers (schedule, incoming message) run on their own once saved — no need to launch them manually.

### Try the Bundled Workflows

HiveMind OS comes with several pre-built workflows ready to use — no setup required. Open **Workflows** and look for the ones with a `system/` prefix:

- **Approval Workflow** — submit a request, get an AI analysis, then approve or reject it. A great way to see how interactive workflows work.
- **Email Responder** — automatically replies to incoming customer emails using your uploaded product docs.
- **Email Triage** — classifies incoming emails by type (bug report, billing, feature request) and routes them.
- **Software Feature** — guides you through planning, implementing, and documenting a software feature with AI agents at each stage.

To customize a bundled workflow, use **New Workflow → Copy from existing** — this creates your own copy that you can edit freely.

::: tip Start simple
Your first workflow should have just three parts: a trigger, one agent step, and one action. You can always add more steps later. Or try launching one of the **bundled workflows** to see a complete example in action.
:::


## Launching Bots

Bots are persistent AI agents that work on tasks in the background. They're like employees you assign work to. Go to **Bots** in the sidebar.

### Create a Bot

1. Click **New Bot**
2. Pick a **persona** — this determines the bot's personality and capabilities
3. Write a **launch prompt** — describe the task in plain language. For example:

   > Research the top five competitors in the specialty coffee market. For each one, find their pricing, key products, and what customers say about them online. Summarize everything in a table.

4. Choose a **mode**:

   | Mode | What It Does |
   |---|---|
   | **One-shot** | Does the task once, then stops. Use for one-off jobs. |
   | **Idle after task** | Completes the task, then waits for you to give it more work. |
   | **Continuous** | Keeps running and working on its own. Use for ongoing monitoring. |

5. Click **Launch**

### Watching Your Bot Work

After launching, your bot appears in the **Agent Stage** — a live view where you can watch what it's doing, see its progress, and read its output. You can have multiple bots running at the same time.

Bots are great for both one-off tasks ("research competitors in my industry") and ongoing work ("monitor my inbox and flag urgent emails every morning").


## Using Chat

The **Chat** view is where you have direct conversations with your AI personas.

- **Pick a persona** from the persona picker at the top of the chat. Each persona has its own conversation history.
- **Type your message** and press Enter. The AI responds using the persona's instructions and preferred model.
- **Use `/prompt`** (or `/p`) to load a saved prompt template. This is handy for tasks you repeat often, like "summarize this article" or "draft a response to this email."
- **Tools happen automatically.** If your persona has access to tools like web search, email, or file reading, it will use them when needed. You don't have to do anything special.
- **Everything stays on your machine.** Your conversations are stored locally, not in the cloud.


## What's Next?

Now that you know your way around the app, explore these resources to go deeper:

- [Use Cases](/use-cases/) — Ready-made automations for common business tasks
- [Personas Guide](/guides/personas) — Deep dive into creating effective AI assistants
- [Workflows Guide](/guides/workflows) — Advanced workflow features and patterns
- [Privacy & Security](/concepts/privacy-and-security) — How your data stays safe
