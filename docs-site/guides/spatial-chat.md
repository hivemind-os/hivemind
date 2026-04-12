# Spatial Chat

HiveMind OS offers two chat modes: **linear** (classic top-to-bottom conversation) and **spatial** (an infinite 2D canvas). Spatial mode turns conversations into visual maps where the arrangement of ideas carries meaning.

## The Canvas

The spatial canvas is an infinite 2D workspace. Every message — yours and the agent's — becomes a **card** positioned in space. Cards connect via **edges** that show relationships. Drag, group, and arrange cards freely to organize your thinking.

## Card Types

| Card Type | Description |
|---|---|
| **Prompt card** | Your question or instruction |
| **Response card** | The agent's reply, linked to its prompt |
| **Artifact card** | Code, images, documents — first-class objects, not inline blobs |
| **Reference card** | Pinned external context (files, URLs, data) |
| **Cluster card** | A named group for related cards — like a folder on the canvas |

## Edges & Relationships

Cards connect via typed edges:

- **Reply-to** — conversational flow (prompt → response)
- **References** — "this card draws on context from that card"
- **Contradicts** — flags tension between two ideas
- **Evolves** — a later card supersedes or refines an earlier one

The agent auto-creates edges as the conversation flows, but you can also draw edges manually to express relationships the agent might not infer.

## Contextual Prompting

Instead of a single global input box, you can **drop a prompt card near relevant context**. The agent automatically scopes its response to nearby cards. Ask a question near your database schema cards, and it knows you're asking about data modeling — no re-explaining needed.

You can also explicitly select cards to include in the next prompt's context: select them, then type your question. Only the selected cards are sent — giving you precise control over what the agent "sees."

::: tip Three ways to prompt
- **Global prompt** — type at the bottom bar; card appears at canvas center
- **Contextual prompt** — click empty canvas space; inline text field appears at that position
- **Card-attached prompt** — click the reply button on any card for a follow-up
:::

## Fork & Explore

Drag a card to an empty region to start a **tangent**. The agent treats it as a sub-conversation while retaining awareness of the parent thread. This lets you explore two approaches side-by-side without cluttering a single thread.

This works naturally with [session forking](/concepts/sessions-and-conversations) — fork the conversation at any card to create an independent branch.

## Gather & Synthesize

Select multiple cards, right-click → **Synthesize**. The agent reads all selected cards and produces a **summary card** that distills the key points. This is perfect for converging after divergent exploration — research three options, then synthesize the trade-offs into one card.

## Agent Behaviors on the Canvas

The agent is a co-organizer, not just a responder:

- **Auto-clustering** — notices thematic overlap and suggests grouping
- **Gap detection** — spots two clusters with no connection and asks "should these relate?"
- **Conflict surfacing** — draws a red edge between contradictory cards
- **Spatial memory** — remembers where things are; you can say "go back to what we discussed in the top-left"

## When to Use Spatial vs. Linear

| Scenario | Mode |
|---|---|
| Focused single-thread Q&A | Linear |
| Quick task or command | Linear |
| Brainstorming with multiple threads | **Spatial** |
| Comparing approaches side-by-side | **Spatial** |
| Research with many sub-topics | **Spatial** |
| Architecture discussions | **Spatial** |
| Visual mapping of agent reasoning chains | **Spatial** |

Switch between modes at any time from the session toolbar. Your conversation history is preserved — switching to spatial lays out existing messages as cards, and switching back to linear presents them chronologically.
