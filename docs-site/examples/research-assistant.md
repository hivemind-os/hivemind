# Deep Research Assistant

Set up a **continuous bot** that performs thorough, multi-source research and stores every finding in the knowledge graph for later recall.

## The Persona

First, create a dedicated research persona with access to web search, file reading, and knowledge graph tools:

```yaml
# personas/researcher.yaml
id: deep-researcher
name: Deep Researcher
description: Thorough research agent that cross-references multiple sources
system_prompt: |
  You are a meticulous research assistant. For every claim you make:
  1. Find at least 2 independent sources to verify it
  2. Note any disagreements between sources
  3. Store every finding as a knowledge graph observation with source URLs
  4. Organize findings into entities by subtopic
  5. Flag low-confidence findings clearly

  Always prefer primary sources (docs, papers, official announcements) over
  secondary summaries. When sources conflict, present both sides.
preferred_models:
  - claude-sonnet
  - gpt-4o
allowed_tools:
  - http.request
  - filesystem.read
  - filesystem.write
  - knowledge.query
loop_strategy: plan_then_execute
context_map_strategy: advanced
```

::: tip Why Plan-then-Execute?
The `plan_then_execute` loop strategy is ideal for research — the agent creates a research plan first, then systematically works through each area. This produces more thorough results than the default ReAct loop for open-ended tasks.
:::

## The Bot

Next, launch a continuous bot using this persona:

```yaml
# bots/research-bot.yaml
friendly_name: Research Bot
persona: deep-researcher
mode: continuous
data_class: INTERNAL
launch_prompt: |
  Research [topic] thoroughly. For every claim, find at least 2 independent
  sources. Store all findings in the knowledge graph with source URLs.
  Organize findings by subtopic. When finished with one area, move to
  the next until the topic is fully covered.
allowed_tools:
  - http.request: auto
  - filesystem.read: auto
  - filesystem.write: ask
  - knowledge.query: auto
timeout: 3600
```

The `continuous` mode keeps the bot running — it won't stop after completing a single task. It will keep researching until the timeout or you tell it to stop.

## Example: Researching a Technology Decision

Suppose your team needs to choose between SQLite and DuckDB for an embedded analytics engine:

```
You: Research SQLite vs DuckDB for embedded analytics in a Rust application.
     Cover performance, ecosystem maturity, Rust bindings, and licensing.
```

The bot will:

1. **Plan** — break the topic into subtopics (performance benchmarks, API comparison, Rust crate quality, license terms)
2. **Search** — query each subtopic, finding official docs, benchmark posts, and GitHub repos
3. **Verify** — cross-reference claims across at least 2 sources
4. **Store** — save each finding as a knowledge graph observation:

```
[Observation] DuckDB columnar scans are 10-100x faster than SQLite for
analytical queries on 1M+ rows.
Sources: duckdb.org/benchmarks, arxiv.org/abs/2305.xxxxx
Confidence: high (official benchmarks + independent paper)
```

## Querying Findings Later

After the bot finishes (or even while it's still running), query findings from the knowledge graph using natural language:

```
You: What did we find about DuckDB Rust bindings maturity?
```

HiveMind OS searches the knowledge graph and returns all stored observations about DuckDB's Rust ecosystem, complete with source URLs and confidence levels.

You can also ask follow-up questions — the bot's stored observations persist across sessions:

```
You: What were the licensing concerns for DuckDB vs SQLite?
```

::: tip Combine with File Output
Add `filesystem.write` approval to let the bot produce a final markdown report alongside the knowledge graph entries. This gives you both a polished document and structured data you can query later.
:::

## When to Use This Pattern

- **Technology evaluations** — compare tools, frameworks, or services with verified data
- **Competitive analysis** — gather and cross-reference public information
- **Literature reviews** — survey a technical area with source tracking
- **Due diligence** — verify claims with multiple independent sources
