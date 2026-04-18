# Skills

Extend what your agents can do with **Agent Skills** — portable, file-based packages of procedures, scripts, and reference material that agents discover and activate on demand. Think of them as plugins for your agent.

HiveMind OS implements the open [Agent Skills](https://agentskills.io/specification) standard. Where MCP provides *callable tools*, skills provide *knowledge and procedures*: how to approach a task, domain conventions, and scripted workflows.

## Configuration

Skills are configured in the `skills` section of your config:

```yaml
# ~/.hivemind/config.yaml
skills:
  enabled: true
  sources:
    - type: github
      url: https://github.com/org/skills-repo
  storage_path: ~/.hivemind/skills-cache
```

### SkillsConfig Fields

| Field | Type | Description |
|---|---|---|
| `enabled` | `bool` | Enable or disable skill discovery |
| `sources` | `Vec<SkillSourceConfig>` | List of skill sources (GitHub repos) |
| `storage_path` | `Option<String>` | Local cache directory for downloaded skills |

Skill discovery scans configured sources and loads each `SKILL.md` frontmatter into a lightweight index. When a task matches a skill description, the full `SKILL.md` body is injected into the agent's context.

## How Activation Works

1. **Startup** — HiveMind OS scans configured skill sources, loading each `SKILL.md` frontmatter (~100 tokens) into a lightweight index.
2. **Match** — When a task matches a skill description (keyword, semantic, or explicit request), the full `SKILL.md` body is injected into the agent's context.
3. **Resources** — Files in `scripts/`, `references/`, and `assets/` load on demand as the agent follows instructions.

## Creating Your Own Skill

### 1. Define the skill manifest

Every skill is a directory with a `SKILL.md` file. The YAML frontmatter is the manifest:

```markdown
---
name: create-presentation
description: Build slide decks from data with branded templates
license: MIT
compatibility: ">=1.0"
metadata:
  author: Alice
  category: productivity
allowed_tools: "filesystem.*, shell.*"
---

## Instructions

1. Load the template from `references/TEMPLATES.md`
2. Generate charts using `scripts/generate_charts.py`
3. Assemble slides following brand guidelines in `assets/`
```

### SkillManifest Fields

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Unique skill name |
| `description` | `String` | What the skill does |
| `license` | `Option<String>` | License identifier |
| `compatibility` | `Option<String>` | Version compatibility constraint |
| `metadata` | `Map<String, String>` | Arbitrary key-value metadata |
| `allowed_tools` | `Option<String>` | Tool glob patterns the skill may use |

### 2. Add supporting files

Add scripts and reference files alongside `SKILL.md`:

```
my-skill/
├── SKILL.md
├── scripts/
│   └── generate_charts.py
├── references/
│   └── TEMPLATES.md
└── assets/
    └── slide-template.pptx
```

### 3. Publish (optional)

Share your skill by pushing the directory to a GitHub repo and adding it as a source in your `SkillsConfig`.

## Skills + Data Classification

Skills integrate with HiveMind OS's security model. The data classification system applies to skills just as it does to conversations and connectors.

::: warning Classify Sensitive Skills
A skill that accesses external APIs or production systems should be used in sessions with appropriate classification (`CONFIDENTIAL` or higher). This prevents accidental data leakage through the skill's tool calls.
:::

## Agent Kits

Bundle personas, workflows, skills, and attachments into a portable `.agentkit` ZIP archive to share complete agent configurations with your team. See the [Agent Kits guide](/guides/agent-kits) for full details on exporting, importing, and namespace remapping.

## Learn More

- [Tools & MCP](/concepts/tools-and-mcp) — How built-in tools and MCP complement skills
- [Privacy & Security](/concepts/privacy-and-security) — Data classification and channel enforcement
- [Personas Guide](/guides/personas) — Scoping skills and tools per persona
- [Workflows Guide](/guides/workflows) — Automating multi-step agent tasks
