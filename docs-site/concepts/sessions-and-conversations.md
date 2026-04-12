# Sessions & Conversations

Every interaction with HiveMind OS happens in a session — a persistent conversation that remembers where you left off.

## Chat Modes

HiveMind OS offers two ways to interact with the agent:

**`linear`** mode is the classic chat experience. Messages flow top to bottom — you type, the agent responds, and the conversation grows downward. Simple, familiar, and perfect for focused tasks.

**`spatial`** mode puts your conversation on an infinite canvas. Each message becomes a card, and edges connect related ideas. Drag, group, and arrange cards freely — turning a conversation into a visual map of your thinking.

These are the two session modalities (`linear` and `spatial`).

::: tip When to go spatial
Use spatial mode when a conversation branches into several subtopics or when you want to visually compare approaches. It's great for architecture discussions, research, and brainstorming where relationships between ideas matter as much as the ideas themselves.
:::

## Session Management

Every conversation is a **session**. Sessions persist across app restarts — close the window, reboot, and your conversation is right where you left it.

- **Switch sessions** from the sidebar to jump between conversations
- **Workspace association** — link a session to a project directory so the agent has context about your codebase
- **Multiple active sessions** — work on different tasks in parallel, each with its own history

## Command Queuing

You don't have to wait for the agent to finish before typing your next message. HiveMind OS supports **command queuing** — submit messages at any time and they'll execute in order.

Queued messages appear with a pending indicator. Right-click to cancel or drag to reorder. The agent is queue-aware, so it can batch related requests when possible.

## Interrupting the Agent

Sometimes the agent heads in the wrong direction. Two interrupt modes:

| Mode | How | What happens |
|---|---|---|
| **`soft`** | `Esc` or Pause button | Agent finishes its current tool call, then stops. You can review partial work and decide whether to resume. |
| **`hard`** | `Ctrl+C` / `Cmd+.` or Stop button | Immediate abort. In-flight operations are cancelled, but completed work (file saves, knowledge writes) is preserved. |

After any interruption, the conversation history marks the interrupted turn with ⚠ so you never lose track of what happened.

## Learn More

- [Spatial Chat Guide](/guides/spatial-chat) — Deep dive into canvas-based conversations
- [Agentic Loops](./agentic-loops) — How the agent reasons through complex tasks
- [Knowledge Graph](./knowledge-graph) — How memory persists across sessions
