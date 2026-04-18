# Glossary

Quick reference for terms used throughout the HiveMind OS documentation. If you're new, start with the [No-Code Guide](/guides/no-code-guide) for a hands-on walkthrough.

---

## Agent Stage

A live dashboard in HiveMind OS where you can watch your bots working. Each bot appears as a card showing what it's doing, and you can respond to questions or approve actions directly.

**See also:** [Bots Guide](/guides/bots)

## Bot

A persistent AI agent that works on tasks in the background — like assigning a job to a virtual employee. You give it instructions, and it works autonomously until the task is done.

**See also:** [Bots Guide](/guides/bots)

## Connector

A link between HiveMind OS and an external service like Gmail, Google Calendar, Slack, or Discord. Once connected, your AI assistants can read and send messages, check your calendar, and more.

**See also:** [No-Code Guide → Connectors](/guides/no-code-guide#connecting-email-calendar-and-chat)

## Cron Expression

A scheduling format used to describe when something should run. For example, `0 9 * * 1-5` means "weekdays at 9:00 AM." You don't need to memorize the syntax — the workflow designer helps you set one up.

**See also:** [Scheduling Guide](/guides/scheduling)

## Daemon

The background service that powers HiveMind OS. It runs quietly on your computer so the app works. You don't need to manage it — the desktop app starts it automatically.

## Feedback Gate

A pause point in a workflow where the AI stops and asks you a question before continuing. Great for approvals, reviews, or any step where you want to stay in the loop.

**See also:** [Workflows Guide](/guides/workflows)

## Knowledge Graph

HiveMind OS's memory system. It stores facts, preferences, and context that the AI remembers across conversations. When you tell the AI "I prefer formal tone," it saves that and applies it later.

**See also:** [Knowledge Management Guide](/guides/knowledge-management)

## MCP Server

An external tool plugin that gives your AI new capabilities — like access to a database, a GitHub repo, or a specialised API. MCP stands for Model Context Protocol, an open standard.

**See also:** [MCP Servers Guide](/guides/mcp-servers)

## Model

The specific AI brain a provider offers. Different models have different strengths: some are faster, some are smarter, some are cheaper. For example, "Claude Sonnet" and "GPT-4o" are models.

**See also:** [Providers & Models](/concepts/providers-and-models)

## OAuth

A secure sign-in method where you grant HiveMind OS access to a service (like Gmail or GitHub) through that service's own login page. You never share your password with HiveMind OS — the service gives it a secure token instead.

## Persona

An AI assistant you create with a specific personality, instructions, and tool access. Think of it like hiring a specialist: you write a job description (the system prompt), choose what tools they can use, and give them a name.

**See also:** [Personas Guide](/guides/personas)

## Provider

An AI service that supplies the language models your personas use to think and respond. Examples include Anthropic (Claude), OpenAI (GPT), GitHub Copilot, and Ollama (local, free).

**See also:** [Configure Providers Guide](/guides/configure-providers)

## System Prompt

Instructions you write to tell an AI persona how to behave — like a detailed job description. The more specific you are, the better the persona performs.

**See also:** [Personas Guide → Writing Effective System Prompts](/guides/personas#writing-effective-system-prompts)

## Trigger

The event that starts a workflow. It could be a timer (schedule), an incoming email or message, a manual button click, or a specific event.

**See also:** [Workflows Guide](/guides/workflows)

## Workflow

A multi-step automation that chains AI tasks, tool actions, and decision points together. Think of it like an assembly line: each step does one job and passes the result to the next.

**See also:** [Workflows Guide](/guides/workflows)

## YAML

A simple text format used for configuration and data. It uses indentation and colons instead of code. You'll see YAML in some docs examples, but you can build most things in HiveMind OS without ever touching it — the visual designer handles it for you.
