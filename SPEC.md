# HiveMind OS — The Ultimate Desktop AI Agent

## 1. Vision

HiveMind OS is a cross-platform (macOS, Windows & Linux) desktop AI agent that acts as a persistent, intelligent operating companion. It connects to multiple model providers, orchestrates background tasks, integrates with external tools via the Model Context Protocol (MCP), maintains a private knowledge graph of its memories and learned context, and enforces strict data-classification boundaries so that private information never leaks over public channels.

---

## 2. Core Principles

| Principle | Description |
|---|---|
| **Privacy by Default** | All data is classified. Nothing leaves a private boundary without explicit policy approval. |
| **Provider Agnostic** | Swap or combine LLM backends without changing agent behaviour. |
| **Pluggable Agency** | The agentic loop (reasoning strategy, tool selection, memory integration) is a first-class extension point. |
| **Persistent Memory** | The agent remembers across sessions via a structured, queryable knowledge graph. |
| **Open Integration** | MCP-native — any MCP server (including the Notifications API) is a first-class citizen. |

---

## 3. Platform & Distribution

### 3.1 Daemon-First Architecture

HiveMind OS runs as a **long-lived background daemon** (system service / launchd agent / systemd unit / Windows Service) that is always on, even when no UI is visible. The daemon owns all state — knowledge graph, scheduled tasks, agent instances, peering connections, messaging bridges — and exposes a local API over a Unix socket (macOS/Linux) or named pipe (Windows) plus an optional localhost HTTP/WebSocket endpoint for browser and remote clients.

```
┌────────────────────────────────────────────────────────────────────┐
│                        HiveMind OS Daemon (Rust)                         │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────┐ │
│  │ Scheduler│ │ KG Engine│ │ Workflow │ │ Peering  │ │ Msg     │ │
│  │          │ │          │ │ Engine   │ │ Transport│ │ Bridges │ │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └─────────┘ │
│  ┌────────────────────────────────────────────────────────────────┐│
│  │                    Local API (socket / HTTP+WS)               ││
│  └────────────────────────────────────────────────────────────────┘│
└──────┬─────────────┬─────────────┬─────────────┬──────────────────┘
       │             │             │             │
 ┌─────▼─────┐ ┌─────▼─────┐ ┌─────▼─────┐ ┌───▼───────────┐
 │ Tauri UI  │ │ Web       │ │ CLI       │ │ Messaging     │
 │ (webview) │ │ Console   │ │           │ │ (Slack/Discord│
 │           │ │ (browser) │ │           │ │  /Telegram)   │
 └───────────┘ └───────────┘ └───────────┘ └───────────────┘
```

All frontends — the Tauri webview, a browser-based web console, a CLI, and messaging bridges — are **equal clients** of the daemon. The daemon continues operating (running agents, syncing peers, processing scheduled tasks, monitoring messaging channels) whether or not any UI is open.

### 3.2 Runtime & Tech Stack

- **Supported OS:** macOS (Apple Silicon + Intel), Windows 10/11 (x64, ARM64), Linux (x64, ARM64).
- **Daemon:** Pure Rust binary. Handles all performance-critical paths (graph engine, crypto, scheduler, workflow engine, peering transport, messaging bridges) as async Tokio tasks.
- **Desktop UI:** [Tauri v2](https://v2.tauri.app/) — thin webview shell that connects to the daemon's local API. The frontend is built with **SolidJS** rendered in the platform webview (WebKit on macOS/Linux, WebView2 on Windows). The Tauri process starts the daemon if it isn't already running, and can detach cleanly without stopping it.
- **Why Tauri:** Sub-20 MB installer (vs ~200 MB for Electron), lower memory footprint, Rust's memory safety eliminates whole classes of security bugs, direct access to OS APIs (keychain, notifications, file system) via Tauri plugins, and IPC between frontend and Rust backend is type-safe and zero-copy where possible.

### 3.3 Lifecycle

| Event | Behaviour |
|---|---|
| **System boot** | Daemon starts automatically via launchd (macOS), systemd (Linux), or Windows Service / Task Scheduler. |
| **Open app** | Tauri UI launches, connects to running daemon. If daemon is down, starts it. |
| **Close UI window** | Daemon continues running. Agents, tasks, and messaging bridges are unaffected. |
| **Quit from tray** | User can choose: quit UI only (daemon stays) or quit everything (daemon stops gracefully after draining active tasks). |
| **System shutdown** | Daemon checkpoints all workflow state and shuts down cleanly. |

### 3.4 Distribution

- **Auto-update:** built-in updater with code-signed binaries.
- **Local-first:** all user data, the knowledge graph, and credentials stay on-device unless the user explicitly configures cloud sync.

---

## 4. Multi-Provider Model Layer

### 4.1 Provider Registry

```yaml
providers:
  - id: openai
    type: openai-compatible
    base_url: https://api.openai.com/v1
    auth: env:OPENAI_API_KEY
    models: [gpt-4o, gpt-4o-mini, o3, o4-mini]
    channel_class: public          # data sent here is PUBLIC

  - id: anthropic
    type: anthropic
    base_url: https://api.anthropic.com
    auth: env:ANTHROPIC_API_KEY
    models: [claude-sonnet-4, claude-opus-4]
    channel_class: public

  - id: ollama-local
    type: openai-compatible
    base_url: http://localhost:11434/v1
    auth: none
    models: [llama3, mistral, deepseek-coder]
    channel_class: private         # data stays local

  - id: corp-azure
    type: azure-openai
    base_url: https://myorg.openai.azure.com
    auth: env:AZURE_API_KEY
    models: [gpt-4o]
    channel_class: internal        # org-internal, not public

  - id: microsoft-foundry
    type: microsoft-foundry
    base_url: https://myorg.services.ai.azure.com
    auth: env:AZURE_FOUNDRY_API_KEY     # API key or Entra ID (azure-ad)
    channel_class: internal             # org-managed Azure tenant
    options:
      allow_model_discovery: true       # Auto-discover deployed models via /models
      default_api_version: "2025-04-01"
    models: [gpt-4o, Phi-4, Mistral-large, DeepSeek-R1, Llama-4-Maverick]

  - id: github-copilot
    type: copilot
    auth: github-oauth             # Uses GitHub session / device-flow OAuth
    models: [gpt-4o, gpt-4.1, claude-sonnet-4, o3, o4-mini]
    channel_class: internal        # Governed by org's GitHub Copilot policies
    features:
      - code-completions
      - chat
      - agent-tools              # Copilot Extensions / tool use

  - id: openrouter
    type: openai-compatible
    base_url: https://openrouter.ai/api/v1
    auth: env:OPENROUTER_API_KEY
    models: [anthropic/claude-sonnet-4, google/gemini-2.5-pro, meta-llama/llama-4-maverick]
    channel_class: public          # Third-party aggregator — treat as public
    options:
      route: fallback              # OpenRouter-specific: auto-fallback between providers
      allow_model_discovery: true  # Dynamically fetch available models from /models endpoint
```

### 4.2 Model Router

The **Model Router** selects a provider/model for each request based on:

1. **Data classification of the prompt** — if the prompt contains `CONFIDENTIAL` or `PRIVATE` data, only providers with a compatible `channel_class` are eligible.
2. **Task requirements** — coding vs. creative writing vs. summarisation can prefer different models.
3. **User preferences** — explicit model pinning per conversation or task.
4. **Cost / latency budgets** — configurable per-task ceilings.
5. **Fallback chain** — if primary is unavailable, cascade to alternates that satisfy the classification constraint.

#### Error Handling

| Error | Behaviour |
|---|---|
| **Rate limit (429)** | Exponential backoff with jitter; try next provider in fallback chain after 3 retries. |
| **Auth failure (401/403)** | Mark provider as `degraded`, surface notification to user, route to fallback. |
| **Timeout** | Per-request configurable timeout (default 120s for streaming). Retry once, then fallback. |
| **Network error** | Retry with backoff. After exhausting retries, queue request if possible or fail with user-visible error. |
| **Malformed response** | Log and retry once; if persistent, mark model as degraded for that request type. |
| **Context overflow** | Trigger context compaction (§9.12) and re-submit the shortened prompt. |

### 4.3 Streaming & Structured Output

- All provider adapters expose a unified streaming interface (`AsyncIterator<Chunk>`).
- Support for tool-call streaming, structured JSON output (via function calling or constrained decoding), and vision/multimodal inputs where the model supports it.

### 4.4 Model Roles

Not every task needs the most capable (and expensive) model. HiveMind OS lets the user assign **model roles** — purpose-specific model selections that the system uses automatically for internal tasks:

```yaml
model_roles:
  # The primary reasoning model for user-facing agentic work
  primary:
    provider: anthropic
    model: claude-sonnet-4

  # A fast, cheap model for internal housekeeping tasks
  admin:
    provider: ollama-local
    model: llama3                    # or any small/fast model
    # provider: openai
    # model: gpt-4o-mini            # cloud alternative

  # Optional: specialised roles
  coding:
    provider: github-copilot
    model: claude-sonnet-4
  vision:
    provider: openai
    model: gpt-4o
  scanner:
    provider: ollama-local
    model: llama3                    # local preferred for isolation
```

#### Admin Model Tasks

The `admin` role is used for lightweight, high-frequency tasks where latency and cost matter more than frontier reasoning:

| Task | Description |
|---|---|
| **Data classification** | Scanning content to suggest classification labels (§5.4). |
| **NL intent parsing** | Interpreting natural-language commands from messaging channels (§3.1). |
| **Message routing** | Deciding which agent instance or role should handle an inbound message. |
| **Summarisation** | Generating titles, summaries, and knowledge-graph node descriptions. |
| **Embedding generation** | Producing vector embeddings for KG similarity search (if model supports it). |
| **Notification triage** | Filtering and prioritising MCP notifications and scheduled-task results. |
| **Conflict resolution** | Resolving minor merge conflicts during federated KG sync (§12). |
| **Log analysis** | Scanning audit logs for anomalies or policy violations. |

The admin model should be fast and inexpensive — a local model (Ollama/llama.cpp) is ideal since it avoids network latency, keeps data local (`local-only` channel class), and has zero per-token cost. A small cloud model (e.g., `gpt-4o-mini`, `claude-haiku`) works as a fallback.

#### Role Resolution

When the system needs a model for an internal task, it resolves in order:

1. **Explicit role** — if a specific role is configured for the task type, use it.
2. **`admin` role** — fall back to the admin model for all internal/housekeeping tasks.
3. **`primary` role** — if no admin model is configured, use the primary model.

Users can override any role per-conversation, per-agent-instance, or per-workflow step.

### 4.5 Embedded Models (In-Process)

For tasks where even a localhost HTTP round-trip is overhead, HiveMind OS can run **small models directly inside the daemon process**. Embedded models are installed as plugins and loaded on demand — no external server required.

#### Runtime

The daemon embeds a lightweight inference engine (e.g., [llama.cpp](https://github.com/ggerganov/llama.cpp) via `llama-cpp-rs`, or [Candle](https://github.com/huggingface/candle) for a pure-Rust option). Models are loaded into memory when first needed and evicted under memory pressure (LRU with configurable ceiling).

#### Plugin Installation

Embedded models are managed like plugins — discoverable, installable, and removable via CLI or UI:

```bash
# Browse available embedded models
hive model list --embedded

# Install a model (downloads GGUF/safetensors to ~/.hivemind/models/)
hive model install smollm2-360m
hive model install all-minilm-l6-v2      # embedding model
hive model install gte-small             # embedding model

# Pin a specific quant
hive model install phi-4-mini --quant q4_k_m

# Remove
hive model uninstall smollm2-360m

# Show installed models and memory footprint
hive model status
```

#### Provider Entry

Installed embedded models appear as a built-in provider with `channel_class: local-only` (data never leaves the process):

```yaml
providers:
  - id: embedded
    type: embedded                       # Built-in provider, no URL
    channel_class: local-only            # Data never leaves the daemon process
    models:
      - id: smollm2-360m
        file: ~/.hivemind/models/smollm2-360m-q4_k_m.gguf
        capabilities: [chat, classification]
        max_context: 8192
        auto_load: true                  # Load at daemon start
      - id: all-minilm-l6-v2
        file: ~/.hivemind/models/all-minilm-l6-v2.safetensors
        capabilities: [embedding]
        dimensions: 384
        auto_load: true
```

#### Ideal Use Cases

| Use Case | Why Embedded |
|---|---|
| **Embedding generation** | Needed constantly for KG vector search; sub-millisecond in-process vs. network hop. |
| **Data classification** | High-frequency scanning of every message; must be fast and fully private. |
| **NL intent parsing** | Parse commands from messaging channels with zero latency. |
| **Tokenisation / chunking** | Split documents for ingestion — no reasoning needed, just tokenizer access. |
| **Regex-class extraction** | NER-style extraction of emails, IPs, secrets for classification labelling (§5.4). |
| **Reranking** | Rerank KG search results before passing to the primary model. |

#### Memory Management

```yaml
embedded_models:
  max_memory_mb: 2048                    # Total memory budget for all loaded models
  eviction: lru                          # Evict least-recently-used when at ceiling
  gpu_layers: auto                       # Offload layers to GPU when available (Metal/CUDA/Vulkan)
  preload: [all-minilm-l6-v2]           # Always keep these loaded
```

Embedded models integrate seamlessly with model roles (§4.4) — set `admin` to an embedded model for a fully offline, zero-cost, zero-latency housekeeping pipeline.

---

## 5. Data Classification & Security Model

### 5.1 Classification Levels

Every piece of data flowing through HiveMind OS carries a **classification label** (inspired by government / enterprise DLP tiers):

| Level | Tag | Description |
|---|---|---|
| 0 | `PUBLIC` | Safe to send anywhere. |
| 1 | `INTERNAL` | Organisation-internal. May be sent to org-managed cloud endpoints. |
| 2 | `CONFIDENTIAL` | Sensitive. Restricted to private/local channels only. |
| 3 | `RESTRICTED` | Highly sensitive (secrets, credentials, PII). Never leaves the device. |

### 5.2 Channel Classification

Every outbound channel (model provider, MCP server, webhook, export) has a declared `channel_class`:

| Channel Class | Accepts data up to level |
|---|---|
| `public` | `PUBLIC` only |
| `internal` | `PUBLIC`, `INTERNAL` |
| `private` | `PUBLIC`, `INTERNAL`, `CONFIDENTIAL` |
| `local-only` | All levels |

**Rule:** Data with classification level `L` may only traverse a channel whose accepted level is ≥ `L`. By default violations are **blocked**, logged, and surfaced to the user — but the enforcement behaviour is configurable (see §5.3).

### 5.3 Classification Override Policy

When a classification violation is detected (data level exceeds channel clearance), the system behaviour is controlled by the `override_policy`:

```yaml
security:
  override_policy:
    # Per-level override behaviour
    INTERNAL:
      action: prompt           # Ask the user before sending
    CONFIDENTIAL:
      action: prompt
    RESTRICTED:
      action: block            # Never allow, even with user consent

    # Global settings
    remember_decisions: true   # Cache user decisions for identical data/channel pairs
    decision_ttl: 24h          # How long to remember a decision
    require_reason: false      # Require the user to type a justification
    max_overrides_per_hour: 10 # Rate-limit to prevent fatigue-based approval
```

#### Override Actions

| Action | Behaviour |
|---|---|
| `block` | Silently blocked. The user is informed but cannot override. Use for `RESTRICTED` data (credentials, secrets). |
| `prompt` | The user sees a modal showing: **(1)** the data that would cross the boundary, **(2)** the source classification and why, **(3)** the target channel and its class. They can **Allow once**, **Allow and remember**, or **Deny**. |
| `allow` | Automatically permitted (not recommended for production — useful for development/testing). |
| `redact-and-send` | Automatically redact the sensitive tokens (replacing with `[REDACTED]`) and send the sanitised version without prompting. The user can review the redaction in the audit log. |

#### Prompt UX

When `action: prompt` fires, the user is presented with:

```
┌─────────────────────────────────────────────────────────────┐
│  ⚠ Classification boundary crossing                        │
│                                                             │
│  Data classified as: CONFIDENTIAL                           │
│  Reason: contains internal project codename "Titan"         │
│  Destination: openai (channel class: public)                │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ > The Titan deployment pipeline uses a custom...      │  │
│  │   [highlighted sensitive tokens]                      │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  [ Allow once ]  [ Allow & remember ]  [ Redact & send ]   │
│  [ Deny ]        [ Reclassify as PUBLIC ]                   │
└─────────────────────────────────────────────────────────────┘
```

- **Allow once** — send this payload as-is; do not remember the decision.
- **Allow & remember** — send and cache the decision so identical data/channel pairs are auto-approved until `decision_ttl` expires.
- **Redact & send** — strip the flagged tokens, show the user the sanitised version, and send.
- **Deny** — block the request; the agent receives a "classification denied" error and can rephrase or use a different provider.
- **Reclassify as PUBLIC** — the user asserts this data was mis-classified; update its label (logged for audit).

All decisions (allow, deny, redact, reclassify) are recorded in the audit log with timestamp, user identity, data hash, channel, and optional justification.

#### Organisational Lockdown

Administrators can enforce policy floors via a managed config overlay:

```yaml
# Managed by IT — user cannot override these
security:
  managed: true
  override_policy:
    RESTRICTED:
      action: block
      user_can_change: false
    CONFIDENTIAL:
      action: prompt
      user_can_change: false
```

When `managed: true`, the user cannot weaken override policies below the admin-set floor (they can only make them stricter).

### 5.4 Labelling

Data gets classified at multiple points:

1. **Ingestion rules** — regex/pattern matchers flag secrets, API keys, credit card numbers, SSNs, emails, etc. automatically as `RESTRICTED` or `CONFIDENTIAL`.
2. **Source-based defaults** — files from `~/.ssh/` are `RESTRICTED`; clipboard paste from a password manager is `RESTRICTED`; a public web page is `PUBLIC`.
3. **User annotation** — the user can explicitly tag any snippet, conversation, or knowledge-graph node.
4. **Graph inheritance** — a knowledge-graph node inherits the *highest* classification of its ancestors (see §8).
5. **LLM-assisted classification** — optionally, a local model can review content and suggest a label (always conservative — err toward higher classification).

### 5.5 Security Controls

| Control | Description |
|---|---|
| **Policy Engine** | A rule engine (OPA-style) evaluates every outbound payload against classification rules before it leaves the process. |
| **Prompt Sanitiser** | Before sending to a public model, strip or redact tokens that exceed the channel's clearance. Offer the user a diff of what was removed. |
| **Audit Log** | Every data-flow decision (allowed, redacted, blocked) is recorded in a tamper-evident local log. |
| **Credential Vault** | API keys and secrets live in the OS keychain (macOS Keychain / Linux Secret Service / Windows Credential Manager), never in plaintext config. |
| **Memory Encryption** | The knowledge graph database is encrypted at rest (AES-256) with a key derived from OS-level user authentication. |
| **Session Isolation** | Each conversation/task context runs in its own sandbox; cross-conversation data sharing requires explicit linking. |

### 5.6 Prompt Injection Defence

Incoming data from external sources (tool results, MCP server responses, web content, file reads, messaging bridges) is a vector for **indirect prompt injection** — adversarial instructions hidden in data that trick the agent into taking unintended actions.

#### Architecture

An **isolated scanner model** — a dedicated LLM instance running in its own context with no access to the agent's conversation, tools, or state — analyses incoming payloads before they reach the agentic loop.

```
External Data ──► Scanner Model (isolated) ──► Verdict ──► Gate ──► Agentic Loop
                       │                          │
                       │                          ▼
                       │                    Risk Ledger (§5.7)
                       ▼
                  No tool access
                  No conversation context
                  No state mutation
```

The scanner model is deliberately constrained:
- **No tool calls** — it can only classify, not act.
- **No conversation history** — it sees only the payload under review.
- **Separate context** — even if the payload contains injection instructions, the scanner cannot execute them.

#### Scanner Configuration

```yaml
security:
  prompt_injection:
    enabled: true
    model_role: scanner               # Uses the "scanner" model role (§4.4)
    scan_sources:                     # Which inbound channels to scan
      - tool_results
      - mcp_responses
      - web_content
      - file_reads
      - messaging_inbound
      - clipboard_paste
    action_on_detection: prompt       # block | prompt | flag | allow
    confidence_threshold: 0.7         # Flag if scanner confidence ≥ this
    max_payload_tokens: 4096          # Truncate very large payloads for scan
    batch_small_payloads: true        # Batch small results into one scan call
    cache_verdicts: true              # Cache verdicts by content hash (avoid re-scanning)
    cache_ttl: 1h
```

#### Scanner Verdict

The scanner returns a structured verdict for each payload:

```typescript
interface ScanVerdict {
  payload_hash: string;              // SHA-256 of scanned content
  source: string;                    // e.g., "tool_result:read_file"
  risk: 'clean' | 'suspicious' | 'injection_detected';
  confidence: number;                // 0.0 – 1.0
  threat_type?: string;              // e.g., "instruction_override", "role_hijack", "data_exfil_attempt"
  flagged_spans?: { start: number; end: number; reason: string }[];
  recommendation: 'pass' | 'redact' | 'block';
  scanned_at: string;               // ISO 8601 timestamp
}
```

#### Enforcement Actions

| Action | Behaviour |
|---|---|
| `block` | Payload is rejected. The agent receives a sanitised error: "Content blocked by injection scanner." Original content is preserved in the risk ledger for review. |
| `prompt` | User sees a warning with the flagged spans highlighted and can **Allow**, **Block**, or **Redact flagged spans**. |
| `flag` | Payload is delivered to the agent with a metadata annotation `{ injection_risk: true }`. The loop can inspect this flag and decide. Logged in risk ledger. |
| `allow` | Scanner runs for audit purposes only — all payloads are passed through. Verdicts are recorded but never block. |

#### Model Role

Add a `scanner` slot to the model roles (§4.4):

| Role | Purpose | Default |
|---|---|---|
| `scanner` | Prompt injection detection. Isolated — no tools, no history. | Admin model fallback, or a small fast model optimised for classification. |

The scanner model should be **local/embedded** when possible (§4.5) to avoid sending potentially sensitive inbound data to a cloud provider for scanning. If no local model is available, the scanner uses the admin model with `local-only` channel preference.

#### Performance Considerations

- **Async scanning**: Scans run in parallel with the agent's thinking. Results are awaited only when the agent would consume the payload.
- **Caching**: Identical content (by hash) is not re-scanned within the `cache_ttl`.
- **Batching**: Multiple small tool results from a single loop iteration can be batched into one scanner call.
- **Skip rules**: Known-safe sources (e.g., local filesystem reads from user-owned directories) can be excluded via `scan_sources` config.

### 5.7 Risk Scan Ledger

Every security scan — prompt injection or otherwise — is recorded in a queryable **risk scan ledger**. This provides full auditability of what data has been scanned, what risks were detected, and what action was taken.

#### Schema

```sql
CREATE TABLE risk_scans (
    id              TEXT PRIMARY KEY,      -- UUID
    scan_type       TEXT NOT NULL,         -- 'prompt_injection' | 'classification' | 'pii_detection' | 'secret_detection' | custom
    payload_hash    TEXT NOT NULL,         -- SHA-256 of scanned content
    payload_preview TEXT,                  -- First 200 chars (redacted if RESTRICTED)
    source          TEXT NOT NULL,         -- Origin: "tool_result:read_file", "mcp:github", "clipboard", etc.
    source_session  TEXT,                  -- Session ID where the scan occurred
    verdict         TEXT NOT NULL,         -- 'clean' | 'suspicious' | 'detected'
    confidence      REAL,                  -- 0.0–1.0
    threat_type     TEXT,                  -- e.g., 'instruction_override', 'secret_leak'
    flagged_spans   TEXT,                  -- JSON array of { start, end, reason }
    action_taken    TEXT NOT NULL,         -- 'passed' | 'blocked' | 'redacted' | 'user_allowed' | 'user_blocked' | 'flagged'
    user_decision   TEXT,                  -- If action was 'prompt': user's choice
    model_used      TEXT,                  -- Which model performed the scan
    scan_duration_ms INTEGER,             -- How long the scan took
    data_class      TEXT,                  -- Classification level of the scanned content
    scanned_at      TEXT NOT NULL,         -- ISO 8601
    session_id      TEXT,                  -- FK to session
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX idx_risk_scans_type ON risk_scans(scan_type);
CREATE INDEX idx_risk_scans_verdict ON risk_scans(verdict);
CREATE INDEX idx_risk_scans_source ON risk_scans(source);
CREATE INDEX idx_risk_scans_scanned_at ON risk_scans(scanned_at);
CREATE INDEX idx_risk_scans_hash ON risk_scans(payload_hash);
```

#### Extensible Scan Types

The ledger is not limited to prompt injection. Any security scan writes to the same table:

| Scan Type | Trigger | What It Detects |
|---|---|---|
| `prompt_injection` | Inbound data from external sources | Adversarial instructions, role hijacking, data exfiltration attempts |
| `classification` | All data at ingestion (§5.4) | Misclassified data, classification-level suggestions |
| `pii_detection` | Outbound data to public/internal channels | Names, emails, phone numbers, addresses, SSNs |
| `secret_detection` | All data at ingestion | API keys, tokens, passwords, private keys (regex + entropy analysis) |
| Custom | User-defined scan plugins (§14.2) | Whatever the plugin defines |

#### Querying the Ledger

Users and agents can query the ledger for risk visibility:

```sql
-- What injection attempts were detected in the last 24 hours?
SELECT * FROM risk_scans
WHERE scan_type = 'prompt_injection' AND verdict = 'detected'
AND scanned_at > datetime('now', '-24 hours')
ORDER BY scanned_at DESC;

-- What percentage of MCP responses triggered a scan flag?
SELECT
  source,
  COUNT(*) as total,
  SUM(CASE WHEN verdict != 'clean' THEN 1 ELSE 0 END) as flagged,
  ROUND(100.0 * SUM(CASE WHEN verdict != 'clean' THEN 1 ELSE 0 END) / COUNT(*), 1) as flag_pct
FROM risk_scans
WHERE scan_type = 'prompt_injection'
GROUP BY source;

-- Has this exact content been scanned before?
SELECT * FROM risk_scans WHERE payload_hash = ? ORDER BY scanned_at DESC LIMIT 1;
```

#### UI Integration

- **Audit Log view** (§13.1) gains a "Risk Scans" tab with filtering by type, verdict, source, and date range.
- **Chat view** shows a small shield icon (🛡️) on messages that were scanned, coloured by verdict (green = clean, amber = suspicious, red = detected).
- **Knowledge graph** nodes ingested from scanned sources carry a `last_scan_verdict` property.
- **Dashboard** widget: "Security Summary" — scan counts, detection rate, top risky sources.

---

## 6. MCP Integration

### 6.1 MCP Client

HiveMind OS acts as a fully-compliant **MCP client**:

- Discovers and connects to MCP servers (local stdio, local SSE/Streamable HTTP, or remote).
- Negotiates capabilities, manages lifecycle (start/stop/restart), and handles reconnection.
- Presents discovered **tools**, **resources**, and **prompts** to the agentic loop as first-class actions.

### 6.2 MCP Server Registry

```yaml
mcp_servers:
  - id: filesystem
    transport: stdio
    command: npx @modelcontextprotocol/server-filesystem /Users/me/projects
    channel_class: local-only

  - id: github
    transport: stdio
    command: npx @modelcontextprotocol/server-github
    env:
      GITHUB_TOKEN: env:GITHUB_TOKEN
    channel_class: internal

  - id: browser
    transport: stdio
    command: npx @anthropic/mcp-browser
    channel_class: public

  - id: corporate-kb
    transport: streamable-http
    url: https://internal.corp/mcp
    auth: oauth2
    channel_class: internal
```

Each MCP server also gets a `channel_class`, and the same data-classification rules from §5 apply: the agent will not send `CONFIDENTIAL` data to a server classified as `public`.

### 6.3 Notifications API

HiveMind OS implements MCP **Notifications** (server → client and client → server):

| Direction | Use |
|---|---|
| **Server → Client** | Tool list changes, resource updates, progress events, log messages. HiveMind OS reacts by refreshing its tool/resource caches and surfacing relevant updates to the user or the agentic loop. |
| **Client → Server** | Roots changed notifications, cancellation signals, initialization confirmations. |

Notifications feed into the **Event Bus** (§7.3), which the scheduler and agentic loop can subscribe to.

### 6.4 Sampling Support

HiveMind OS supports MCP's **sampling** capability — MCP servers may request the agent to perform LLM completions on their behalf. These requests are:

- Subject to user approval (configurable: always-ask, auto-approve for trusted servers, deny).
- Routed through the Model Router, inheriting the MCP server's `channel_class` as the maximum data level for the prompt.

---

## 7. Background Task Scheduler

### 7.1 Task Model

```
Task {
  id:            uuid
  name:          string
  schedule:      cron | interval | event-trigger | one-shot
  agent_config:  AgentLoopConfig      # which loop strategy, model, tools
  input:         structured payload
  data_class:    ClassificationLevel  # max level for this task's data
  status:        pending | running | paused | completed | failed
  retries:       { max: int, backoff: duration }
  timeout:       duration
  created_at:    timestamp
  last_run:      timestamp
  next_run:      timestamp
}
```

### 7.2 Schedule Types

| Type | Example |
|---|---|
| **Cron** | `0 9 * * MON-FRI` — every weekday at 09:00 |
| **Interval** | `every 30m` — polling a resource |
| **Event-trigger** | `on:mcp:github:pull_request.opened` — react to an MCP notification |
| **One-shot** | `at 2025-12-01T18:00:00Z` — deferred execution |

### 7.3 Event Bus

An internal pub/sub bus connects:

- MCP notifications → scheduler triggers
- Task completions → downstream tasks
- User actions (e.g., "remind me in 2 hours") → one-shot tasks
- Knowledge graph changes → reactive tasks (e.g., re-index on new data)

### 7.4 Resource Governance

- Configurable concurrency limits (max parallel tasks).
- Per-provider rate-limit awareness (token bucket per API key).
- Tasks inherit their creator's data-classification context; the scheduler enforces this at dispatch time.

---

## 8. Knowledge Graph

The knowledge graph is HiveMind OS's long-term memory. It stores entities, relationships, observations, and learned facts across all sessions.

### 8.1 Storage Engine

- **Primary store:** SQLite with a property-graph schema (nodes + edges tables with JSON properties).
- **Full-text search:** SQLite FTS5 virtual table on node text properties.
- **Vector index:** [`sqlite-vec`](https://github.com/asg017/sqlite-vec) extension for KNN vector search — pure C, no dependencies, stores embeddings as blobs, supports nearest-neighbour queries via virtual tables. Embeddings generated by a local in-process model (§4.5) to keep data private.
- Encrypted at rest (§5.5).

### 8.2 Graph Schema

```
┌─────────────────────────────────────────────────────────────────────┐
│                         NODE TYPES (Labels)                        │
├──────────────────┬──────────────────────────────────────────────────┤
│ Label            │ Description                                     │
├──────────────────┼──────────────────────────────────────────────────┤
│ Entity           │ A real-world thing: person, project, org, place │
│ Concept          │ An idea, topic, or domain (e.g., "Rust async")  │
│ Artifact         │ A concrete artifact: file, repo, document, URL  │
│ Event            │ Something that happened at a point in time      │
│ Task             │ A tracked task or goal (links to §7)            │
│ Observation      │ A discrete fact the agent learned               │
│ Preference       │ A user preference or behavioural pattern        │
│ Conversation     │ A reference to a past conversation/session      │
│ Skill            │ A capability or procedure the agent has learned │
│ Tool             │ A registered tool (MCP or built-in)             │
└──────────────────┴──────────────────────────────────────────────────┘
```

#### Common Node Properties

Every node carries a base set of properties:

```typescript
interface BaseNodeProperties {
  id:               string        // UUID
  name:             string        // Human-readable label
  description?:     string        // Longer description
  data_class:       DataClass     // PUBLIC | INTERNAL | CONFIDENTIAL | RESTRICTED
  source:           Source        // Where this knowledge came from
  confidence:       float         // 0.0–1.0, how certain the agent is
  created_at:       timestamp
  updated_at:       timestamp
  last_accessed_at: timestamp     // For decay / relevance scoring
  embedding?:       float[]       // Optional vector for semantic search
  tags:             string[]      // Freeform tags
  ttl?:             duration      // Optional expiry (e.g., "this token expires in 30 days")
}

type Source = {
  type:       'user' | 'observation' | 'inference' | 'tool' | 'mcp' | 'import'
  session_id?: string
  tool_id?:    string
  url?:        string
  timestamp:   timestamp
}

enum DataClass {
  PUBLIC       = 0,
  INTERNAL     = 1,
  CONFIDENTIAL = 2,
  RESTRICTED   = 3
}
```

#### Node-Specific Properties

```typescript
// Entity
interface EntityNode extends BaseNodeProperties {
  entity_type: 'person' | 'organization' | 'project' | 'location' | 'service' | 'device' | string
  aliases:     string[]       // Alternative names
  attributes:  Record<string, any>  // Flexible key-value (e.g., { email: "...", role: "..." })
}

// Observation
interface ObservationNode extends BaseNodeProperties {
  subject_id:  string         // The node this observation is about
  content:     string         // The actual observation text
  valid_from?: timestamp      // Temporal bounds
  valid_until?: timestamp
  supersedes?: string         // ID of an older observation this replaces
}

// Preference
interface PreferenceNode extends BaseNodeProperties {
  domain:     string          // e.g., "coding_style", "communication", "workflow"
  key:        string          // e.g., "indent_style"
  value:      any             // e.g., "spaces:2"
  strength:   float           // How strongly expressed (0–1)
}

// Skill
interface SkillNode extends BaseNodeProperties {
  procedure:   string         // Step-by-step instructions the agent learned
  examples:    string[]       // Example invocations
  tools_used:  string[]       // Tool IDs involved
  success_rate: float         // Tracked effectiveness
}

// Artifact
interface ArtifactNode extends BaseNodeProperties {
  artifact_type: 'file' | 'repository' | 'document' | 'url' | 'snippet' | string
  uri:           string       // Path or URL
  mime_type?:    string
  checksum?:     string       // Content hash for change detection
  content?:      string       // Optionally cached content (classification rules apply)
}

// Event
interface EventNode extends BaseNodeProperties {
  event_type:  string         // e.g., "deployment", "meeting", "error"
  occurred_at: timestamp
  duration?:   duration
  outcome?:    string
}
```

#### Edge Types (Relationships)

```
┌─────────────────────┬───────────────────────────────────────────────────────────┐
│ Relationship        │ Description                                              │
├─────────────────────┼───────────────────────────────────────────────────────────┤
│ RELATES_TO          │ General association (weighted, typed)                     │
│ IS_A                │ Type hierarchy (e.g., Rust IS_A Language)                 │
│ PART_OF             │ Composition (e.g., Module PART_OF Project)               │
│ CREATED_BY          │ Authorship                                               │
│ OBSERVED_IN         │ Links an Observation to the Conversation it came from    │
│ DEPENDS_ON          │ Dependency (tasks, artifacts)                            │
│ SUPERSEDES          │ Newer knowledge replaces older                           │
│ DERIVED_FROM        │ Inference chain (this fact was derived from these facts) │
│ USES_TOOL           │ A Skill or Task uses a specific Tool                     │
│ MENTIONED_IN        │ Entity was referenced in a Conversation or Artifact      │
│ TRIGGERED_BY        │ An Event was triggered by another Event or Task          │
│ PREFERS             │ User preference link (User → Preference)                 │
│ KNOWS_ABOUT         │ Agent's knowledge link (Agent → Concept/Entity)          │
│ SIMILAR_TO          │ Semantic similarity (with score)                         │
└─────────────────────┴───────────────────────────────────────────────────────────┘
```

#### Edge Properties

```typescript
interface EdgeProperties {
  id:          string
  rel_type:    string        // One of the types above
  weight:      float         // Strength / importance (0–1)
  data_class:  DataClass     // Inherited: max(source.data_class, target.data_class)
  source:      Source        // Provenance
  created_at:  timestamp
  metadata?:   Record<string, any>  // Relationship-specific data
}
```

### 8.3 Classification Propagation in the Graph

The **effective classification** of any node is:

```
effective_class(node) = max(
  node.data_class,
  max(effective_class(parent) for parent in ancestors(node))
)
```

This means:
- If a `CONFIDENTIAL` Entity is linked to a `PUBLIC` Observation, the observation's effective class becomes `CONFIDENTIAL`.
- When the agentic loop retrieves graph context to inject into a prompt, it filters nodes where `effective_class(node) > channel_class(target_provider)`.

### 8.4 Memory Operations

The agent interacts with the graph via a **Memory Manager** that provides high-level operations:

```
MemoryManager {
  // Write operations
  remember(observation: string, about?: NodeRef, class?: DataClass) → ObservationNode
  learn_skill(name: string, procedure: string, tools: ToolRef[]) → SkillNode
  track_entity(entity: EntityNode) → EntityNode
  record_event(event: EventNode) → EventNode
  set_preference(domain: string, key: string, value: any) → PreferenceNode
  link(source: NodeRef, target: NodeRef, rel: RelType, weight?: float) → Edge
  supersede(old: NodeRef, new_observation: string) → ObservationNode
  forget(node: NodeRef, reason: string) → void  // Soft-delete with audit trail

  // Read operations
  recall(query: string, limit?: int, class_ceiling?: DataClass) → Node[]
  query_graph(query: string, class_ceiling?: DataClass) → ResultSet
  get_context(topic: string, depth?: int, class_ceiling?: DataClass) → SubGraph
  similar(embedding: float[], limit?: int, class_ceiling?: DataClass) → Node[]
  get_observations(about: NodeRef, active_only?: bool) → ObservationNode[]

  // Maintenance
  decay() → void                      // Reduce confidence of stale nodes
  consolidate() → void                // Merge duplicate nodes, resolve contradictions
  reindex_embeddings() → void         // Rebuild vector index
  export(class_ceiling: DataClass) → GraphSnapshot   // Filtered export
}
```

### 8.5 Querying the Graph

The agent uses several query strategies:

1. **Keyword / FTS** — fast text search over node names, descriptions, and observation content.
2. **SQL graph traversal** — structured queries using recursive CTEs over the property-graph schema (e.g., "all Observations about Entity X created in the last 7 days"). A lightweight query builder translates high-level graph patterns into optimised SQL.
3. **Semantic similarity** — vector-based nearest-neighbour search via `sqlite-vec` KNN queries for fuzzy recall. **Important:** `sqlite-vec` KNN queries require an explicit `k = ?` parameter in the WHERE clause (not just `LIMIT`). Example: `SELECT node_id, distance FROM vec_nodes WHERE embedding MATCH ? AND k = 10`.
4. **Hybrid** — combine FTS hits with graph-neighbourhood expansion and re-rank by relevance + recency + confidence.

Every query method accepts a `class_ceiling` parameter. Nodes whose `effective_class` exceeds the ceiling are **excluded from results** — this is enforced at the query engine level, not as a post-filter, to prevent side-channel leakage.

### 8.6 Memory Lifecycle

```
┌──────────┐     observe      ┌─────────────┐    consolidate    ┌──────────────┐
│  Input   │ ──────────────► │  Short-term  │ ────────────────► │  Long-term   │
│ (session)│                  │  (per-conv)  │                   │  (graph DB)  │
└──────────┘                  └─────────────┘                    └──────────────┘
                                    │                                   │
                                    │ decay (low confidence,            │ forget
                                    │ low access, expired TTL)          │ (user request
                                    ▼                                   │  or policy)
                              ┌───────────┐                             ▼
                              │  Archived  │                      ┌───────────┐
                              │  / Pruned  │                      │  Deleted   │
                              └───────────┘                      │  (audit)   │
                                                                  └───────────┘
```

1. **Observe** — during a conversation, the agent extracts notable facts, entities, and preferences.
2. **Short-term buffer** — held in-memory for the session; low-confidence items may never persist.
3. **Consolidate** — at session end (or periodically), the Memory Manager merges short-term observations into the long-term graph, deduplicating and resolving contradictions.
4. **Decay** — a periodic job reduces `confidence` for nodes that haven't been accessed; very low confidence nodes are archived.
5. **Forget** — explicit user or policy-driven deletion (with an audit record that *something* was deleted, but not its content).

---

## 9. Pluggable Agentic Loop

### 9.1 Loop Architecture

The agentic loop is HiveMind OS's reasoning engine. It is not hardcoded — it is a **pluggable pipeline** of stages.

```
                        ┌──────────────────────────────────────────┐
                        │             AgentLoop Pipeline           │
                        │                                          │
  User Input ──────►    │  ┌────────┐  ┌─────────┐  ┌──────────┐ │ ──────► Response
                        │  │ Stage 1│→ │ Stage 2 │→ │ Stage N  │ │
  MCP Notification ─►   │  └────────┘  └─────────┘  └──────────┘ │
                        │         ▲          │            │        │
  Scheduled Task ───►   │         │    ┌─────▼─────┐      │        │
                        │         └────│ Middleware │──────┘        │
                        │              └───────────┘               │
                        └──────────────────────────────────────────┘
                                           │
                           ┌───────────────┼───────────────┐
                           ▼               ▼               ▼
                      Model Router    Tool Executor   Memory Manager
```

### 9.2 Built-in Loop Strategies

| Strategy | Description |
|---|---|
| **ReAct** | Reason → Act → Observe cycle. Classic tool-using agent. |
| **Plan-and-Execute** | Generate a multi-step plan, then execute each step. |
| **Reflexion** | ReAct with a self-critique step after each action to catch errors. |
| **Tree-of-Thought** | Explore multiple reasoning branches, select the best. |
| **Human-in-the-Loop** | Pause for user confirmation at configurable checkpoints. |

### 9.3 Loop Configuration

```yaml
agent_loop:
  strategy: react            # or plan-and-execute, reflexion, etc.
  max_iterations: 25
  max_tokens_per_turn: 4096
  model: openai/gpt-4o       # provider/model

  middleware:
    - classification_gate    # Checks data labels before every tool call & model call
    - memory_augmentation    # Injects relevant graph context into prompts
    - cost_tracker           # Tracks token usage and enforces budgets
    - audit_logger           # Logs every action for compliance

  tool_policy:
    auto_approve:
      - filesystem.read
      - github.get_issue
    require_confirmation:
      - filesystem.write
      - github.create_pr
    deny:
      - shell.exec          # Deny dangerous tools by default

  memory:
    auto_observe: true       # Automatically extract facts from conversations
    context_window: 20       # Number of graph nodes to inject as context
    class_ceiling: auto      # Determined by the target model's channel_class

  fallback:
    on_failure: retry_with_reflection  # or escalate_to_user, try_alternative_model
    max_retries: 3
```

### 9.4 Middleware Interface

```typescript
interface LoopMiddleware {
  name: string
  
  // Called before each LLM invocation
  beforeModelCall(context: LoopContext, request: ModelRequest): ModelRequest | Block
  
  // Called after each LLM response
  afterModelResponse(context: LoopContext, response: ModelResponse): ModelResponse
  
  // Called before each tool invocation
  beforeToolCall(context: LoopContext, call: ToolCall): ToolCall | Block
  
  // Called after each tool result
  afterToolResult(context: LoopContext, result: ToolResult): ToolResult
  
  // Called when the loop completes
  onComplete(context: LoopContext, result: FinalResult): void
}

interface LoopContext {
  session_id:       string
  conversation_id:  string
  iteration:        number
  data_class:       DataClass          // Current classification ceiling
  memory:           MemoryManager
  history:          Message[]
  active_tools:     ToolDefinition[]
  metadata:         Record<string, any>
}
```

### 9.5 Loop Authoring: Hybrid DSL + Code

Custom agentic loops are authored using a **hybrid approach**: a declarative DSL for structure and flow, with TypeScript escape hatches for custom logic.

#### Why not pure code or pure DSL?

| Approach | Problem |
|---|---|
| **Pure TypeScript** | Arbitrary code execution is a security risk — especially for community-shared loops. Hard to statically analyse, hard to visualise, hard to sandbox. |
| **Pure YAML/DSL** | Expressiveness ceiling — complex reasoning patterns (dynamic branching, custom scoring, domain-specific heuristics) become awkward or impossible. |
| **Hybrid (recommended)** | The DSL handles composition, flow, and safety properties. TypeScript handles custom logic in sandboxed isolates. Best of both worlds. |

#### The Loop DSL: `.loop.yaml`

A loop definition is a directed graph of **stages** with typed transitions, conditions, and error handlers:

```yaml
# deep-research.loop.yaml
name: deep-research
version: 1.2.0
description: Multi-pass research loop with source verification
author: hivemind-community
license: MIT

# Declare the state schema — this is what gets persisted
state:
  query:         { type: string, required: true }
  sources:       { type: array, items: string, default: [] }
  findings:      { type: array, items: object, default: [] }
  verification:  { type: object, default: {} }
  confidence:    { type: number, default: 0 }
  iteration:     { type: number, default: 0 }

# Configurable parameters users can set
params:
  min_confidence:  { type: number, default: 0.8 }
  max_iterations:  { type: number, default: 5 }
  verify_sources:  { type: boolean, default: true }
  model:           { type: string, default: "auto" }

stages:
  plan:
    type: model_call
    prompt_template: |
      Given this research query: {{state.query}}
      Previous findings: {{state.findings | json}}
      Generate a research plan with specific search queries.
    output: state.plan
    next: search

  search:
    type: parallel_tool_calls
    tools:
      - web.search: { query: "{{step.query}}" }
      - knowledge_graph.recall: { query: "{{state.query}}" }
    for_each: state.plan.queries
    output: state.sources
    next: analyse

  analyse:
    type: model_call
    prompt_template: |
      Analyse these sources for: {{state.query}}
      Sources: {{state.sources | json}}
      Extract key findings with confidence scores.
    output: state.findings
    next: verify

  verify:
    type: conditional
    when:
      - condition: "{{params.verify_sources}}"
        next: verify_sources
      - default: synthesise

  verify_sources:
    type: custom_stage            # Escape hatch to TypeScript
    handler: ./stages/verify.ts   # Sandboxed code (see §9.6)
    input: { findings: "{{state.findings}}" }
    output: state.verification
    next: evaluate

  evaluate:
    type: conditional
    when:
      - condition: "{{state.confidence >= params.min_confidence}}"
        next: synthesise
      - condition: "{{state.iteration >= params.max_iterations}}"
        next: synthesise           # Give up and synthesise what we have
      - default: plan              # Loop back — refine the plan

  synthesise:
    type: model_call
    prompt_template: |
      Synthesise a comprehensive answer for: {{state.query}}
      Verified findings: {{state.findings | json}}
      Verification results: {{state.verification | json}}
    output: state.answer
    next: done

  done:
    type: terminal
    emit:
      - type: response
        content: "{{state.answer}}"
      - type: memory_write
        operation: remember
        content: "Researched: {{state.query}} → {{state.answer | truncate: 200}}"

# Error handling
on_error:
  tool_failure:    { retry: 2, backoff: exponential, then: skip_stage }
  model_failure:   { retry: 3, fallback_model: true, then: escalate_to_user }
  classification:  { action: block, log: true }
  timeout:         { after: 5m, action: checkpoint_and_pause }

# Data classification constraints
security:
  max_data_class: auto         # Inherited from context
  allowed_channels: [private, internal]   # This loop should not use public models
```

#### DSL Primitives

| Stage Type | Description |
|---|---|
| `model_call` | Send a prompt to the model router, capture structured output. |
| `tool_call` | Invoke a single tool. |
| `parallel_tool_calls` | Invoke multiple tools concurrently. |
| `conditional` | Branch based on state predicates. |
| `loop` | Iterate over a collection in state. |
| `human_input` | Pause and prompt the user. |
| `memory_read` | Query the knowledge graph. |
| `memory_write` | Write to the knowledge graph. |
| `checkpoint` | Force a state checkpoint (see §9.7). |
| `sub_loop` | Invoke another loop definition as a nested sub-routine. |
| `custom_stage` | Escape to sandboxed TypeScript (see §9.6). |
| `agent_spawn` | Start a new agent instance for a given role; returns the instance handle. |
| `parallel_agent_spawn` | Start multiple agent instances concurrently; waits for all to complete. |
| `terminal` | End the loop, emit final events. |

### 9.6 Custom Stage Sandboxing

When a loop needs logic that the DSL can't express, `custom_stage` delegates to a TypeScript function. These are transpiled to JavaScript at load time and run in a **sandboxed QuickJS runtime** (via the `rquickjs` crate) with strict constraints:

```typescript
// stages/verify.ts
import type { StageContext, StageResult } from '@hivemind/loop-sdk'

export default async function verify(ctx: StageContext): Promise<StageResult> {
  const { findings } = ctx.input
  const verified = []

  for (const finding of findings) {
    // Can use ctx.tools to call registered tools (subject to policy)
    const result = await ctx.tools.call('web.fetch', { url: finding.source_url })
    
    // Can use ctx.model to make LLM calls (subject to classification)
    const assessment = await ctx.model.complete({
      prompt: `Does this page support the claim: "${finding.claim}"?\n\n${result.content}`,
      response_format: { type: 'json', schema: { supported: 'boolean', confidence: 'number' } }
    })

    verified.push({ ...finding, ...assessment })
  }

  return { output: { verified }, next: 'evaluate' }
}
```

#### Sandbox Constraints

| Constraint | Enforcement |
|---|---|
| **No filesystem access** | QuickJS sandbox has no `fs` module. |
| **No network access** | All external calls go through `ctx.tools` (subject to policy & classification). |
| **No process/OS access** | No `child_process`, `os`, `process` available. |
| **CPU time limit** | Configurable per-stage timeout (default 30s). |
| **Memory limit** | Configurable heap ceiling (default 128MB). |
| **Deterministic imports** | Only `@hivemind/loop-sdk` and explicitly declared dependencies. |
| **Classification enforcement** | `ctx.tools` and `ctx.model` apply the same classification gate as the DSL stages. |

#### TypeScript Transpilation

Custom stages are authored in TypeScript but execute in QuickJS (which only runs JavaScript). At loop load time, HiveMind OS transpiles `.ts` stage files to ES2020 JavaScript using [SWC](https://swc.rs/) (Rust-native, fast, no Node.js dependency). The transpiled output is cached alongside the loop definition. Type-checking is **not** performed at runtime — authors use their local TypeScript tooling for that during development.

### 9.7 Workflow State Persistence & Recovery

Agentic loops are frequently **long-running** — a deep research task might span minutes, a scheduled job might be interrupted by an OS sleep or app restart. HiveMind OS needs durable execution.

#### Event-Sourced State Machine

Every loop execution is recorded as an ordered log of events. State can be reconstructed by replaying the log.

```
┌──────────┐     event      ┌──────────────┐     persist     ┌──────────────┐
│  Stage   │ ────────────►  │  Event Log   │ ──────────────► │  SQLite WAL  │
│ Executor │                │  (in-memory)  │                 │  (encrypted) │
└──────────┘                └──────────────┘                  └──────────────┘
                                   │                                 │
                                   │ checkpoint                      │ replay
                                   ▼                                 ▼
                            ┌──────────────┐                  ┌──────────────┐
                            │  State       │                  │  Recovery    │
                            │  Snapshot    │                  │  on restart  │
                            └──────────────┘                  └──────────────┘
```

#### Event Log Schema

```typescript
interface WorkflowEvent {
  id:           string         // Monotonic event ID
  run_id:       string         // Workflow run ID
  loop_id:      string         // Which loop definition
  sequence:     number         // Ordered sequence within the run
  stage:        string         // Which stage produced this event
  type:         LoopEventType  // See below
  payload:      any            // Event-specific data
  state_patch:  JSONPatch      // Delta applied to workflow state
  data_class:   DataClass      // Classification of the event payload
  timestamp:    timestamp
  duration_ms:  number         // How long this step took
}

type LoopEventType =
  | 'stage_entered'     | 'stage_completed'    | 'stage_failed'
  | 'model_call_start'  | 'model_call_end'
  | 'tool_call_start'   | 'tool_call_end'
  | 'user_prompted'     | 'user_responded'
  | 'checkpoint'        | 'state_updated'
  | 'classification_block'   | 'classification_override'
  | 'loop_completed'    | 'loop_failed'        | 'loop_paused'
```

#### Checkpointing

- **Automatic checkpoints** — after every stage completion and before every external call (model/tool).
- **Explicit checkpoints** — the DSL `checkpoint` stage forces a full state snapshot.
- **Recovery** — on restart, HiveMind OS replays from the last checkpoint, re-executing only the incomplete stage.
- **Idempotency** — tool calls are tagged with the event sequence number; tools that support idempotency keys can safely be retried without side-effect duplication.

#### Resumable Execution

```
User: "Research quantum error correction"
  → Loop starts, completes stages: plan → search → analyse
  → OS sleep interrupts execution
  → [App restarts]
  → HiveMind OS detects incomplete run, replays from checkpoint
  → Resumes at: verify (no re-execution of plan/search/analyse)
  → Loop completes: verify → evaluate → synthesise → done
```

The user can also **manually pause and resume** long-running loops from the Tasks dashboard.

### 9.8 The Workflow Engine

The scheduler (§7) and the agentic loop together form HiveMind OS's **embedded workflow engine**. This is not a distributed system like Temporal — it's a lightweight, single-node engine purpose-built for a desktop agent.

#### Capabilities

| Capability | Description |
|---|---|
| **Durable Execution** | Event-sourced replay ensures no work is lost across restarts. |
| **Saga / Compensation** | Multi-step operations can define compensating actions (undo) for partial failure rollback. |
| **Nested Workflows** | A loop can invoke sub-loops as nested workflows, each with their own state and checkpoint stream. |
| **Timer / Schedule** | Loops can be triggered by cron, interval, or event (§7.2). |
| **Fan-out / Fan-in** | `parallel_tool_calls` and `loop` stages support concurrent execution with join. |
| **Human-in-the-Loop** | `human_input` stages pause execution, persist state, and resume when the user responds (even days later). |
| **Backpressure** | Respects concurrency limits and rate-limit budgets from §7.4. |

#### State Store

```sql
-- Workflow runs
CREATE TABLE workflow_runs (
  id              TEXT PRIMARY KEY,
  loop_id         TEXT NOT NULL,           -- Which loop definition
  loop_version    TEXT NOT NULL,           -- Pinned version
  status          TEXT NOT NULL,           -- pending | running | paused | completed | failed | cancelled
  state           BLOB NOT NULL,          -- Current state snapshot (encrypted)
  params          BLOB NOT NULL,          -- User-supplied parameters
  data_class      INTEGER NOT NULL,       -- Max classification level
  parent_run_id   TEXT,                   -- If this is a nested sub-loop
  forked_from_run TEXT,                   -- If this run was forked from another session
  fork_at_event   INTEGER,               -- Event sequence number where fork branched off
  fork_type       TEXT,                   -- head | historical | conversation
  created_at      TIMESTAMP NOT NULL,
  updated_at      TIMESTAMP NOT NULL,
  completed_at    TIMESTAMP
);

-- Event log (append-only)
CREATE TABLE workflow_events (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id          TEXT NOT NULL REFERENCES workflow_runs(id),
  sequence        INTEGER NOT NULL,
  stage           TEXT NOT NULL,
  event_type      TEXT NOT NULL,
  payload         BLOB,                   -- Encrypted
  state_patch     BLOB,                   -- JSON Patch delta
  data_class      INTEGER NOT NULL,
  timestamp       TIMESTAMP NOT NULL,
  duration_ms     INTEGER,
  UNIQUE(run_id, sequence)
);

-- Checkpoints (periodic state snapshots for fast recovery)
CREATE TABLE workflow_checkpoints (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id          TEXT NOT NULL REFERENCES workflow_runs(id),
  event_sequence  INTEGER NOT NULL,       -- Checkpoint taken after this event
  state_snapshot  BLOB NOT NULL,          -- Full state (encrypted)
  timestamp       TIMESTAMP NOT NULL
);
```

### 9.9 Loop Registry: Discovery, Distribution & Trust

#### The HiveMind OS Loop Registry

A public, searchable registry of community-authored loops — think "npm for agent strategies."

```
┌────────────────────────────────────────────────────────────────┐
│                    HiveMind OS Loop Registry                         │
│                                                                │
│  ┌─────────────────────┐     ┌──────────────────────────────┐ │
│  │  GitHub Repository   │     │  Registry Index (API)        │ │
│  │  hive-loops/        │     │  search, metadata, versions  │ │
│  │    registry/         │     │  download counts, ratings    │ │
│  │      deep-research/  │     └──────────────────────────────┘ │
│  │      code-review/    │                                      │
│  │      data-pipeline/  │     ┌──────────────────────────────┐ │
│  │      ...             │     │  Sigstore Transparency Log   │ │
│  └─────────────────────┘     │  (Rekor)                     │ │
│                               └──────────────────────────────┘ │
└────────────────────────────────────────────────────────────────┘
```

#### Registry Structure

Each loop is published as a directory with a manifest:

```
deep-research/
├── loop.yaml              # The DSL definition
├── stages/                # Custom TypeScript stages (if any)
│   ├── verify.ts
│   └── score.ts
├── hive-loop.json        # Package manifest
├── README.md              # Documentation, examples
├── SECURITY.md            # What data/tools this loop accesses
└── signatures/
    ├── loop.yaml.sig      # Sigstore signature
    └── stages.bundle.sig  # Sigstore signature for code bundle
```

#### Manifest: `hive-loop.json`

```json
{
  "name": "deep-research",
  "version": "1.2.0",
  "description": "Multi-pass research with source verification",
  "author": {
    "name": "Jane Doe",
    "github": "janedoe",
    "oidc_issuer": "https://github.com/login/oauth"
  },
  "license": "MIT",
  "hive_sdk": ">=0.5.0",
  "entry": "loop.yaml",
  "keywords": ["research", "web-search", "verification"],

  "requirements": {
    "tools": ["web.search", "web.fetch"],
    "min_model_capability": "tool_use",
    "data_class_ceiling": "INTERNAL"
  },

  "permissions": {
    "tools": ["web.search", "web.fetch", "knowledge_graph.*"],
    "model_calls": true,
    "human_input": true,
    "network": false,
    "filesystem": false
  },

  "signatures": {
    "method": "sigstore-cosign",
    "transparency_log": "rekor.sigstore.dev"
  }
}
```

#### CLI Interaction

```bash
# Search the registry
hive loop search "code review"

# Inspect before installing (shows permissions, author, signature status)
hive loop inspect hivemind-community/code-review

# Install (verifies signature first)
hive loop install hivemind-community/code-review@1.4.0

# List installed loops
hive loop list

# Use a loop
hive loop run deep-research --query "quantum error correction advances 2025"

# Publish (signs with your GitHub identity via Sigstore)
hive loop publish ./my-loop/
```

### 9.10 Supply Chain Security: Sigstore Signing

All loops in the registry are signed using **Sigstore**, providing identity-based signing without the burden of key management.

#### Why Sigstore?

| Property | Benefit |
|---|---|
| **Keyless signing** | Authors sign with their GitHub/OIDC identity — no GPG keys to manage, rotate, or lose. |
| **Transparency log** | Every signature is recorded in Rekor (an immutable, publicly auditable log). Tampering is detectable. |
| **Identity verification** | HiveMind OS can verify *who* signed a loop, not just *that* it was signed. Users can trust by identity (`github:janedoe`) or by org (`github:hive-community`). |
| **Ecosystem alignment** | npm, PyPI, Homebrew, and container registries are adopting Sigstore. Users already have the mental model. |
| **Offline verification** | Signature bundles are self-contained; HiveMind OS can verify without network access after initial trust root fetch. |

#### Signing Flow

```
Author publishes a loop:

  1. hive loop publish ./my-loop/
  2. HiveMind OS CLI triggers Sigstore keyless signing:
     a. Author authenticates via GitHub OAuth (OIDC)
     b. Sigstore issues an ephemeral signing certificate tied to the author's GitHub identity
     c. Each file (loop.yaml, *.ts stages) is signed
     d. Signatures + certificate are bundled into signatures/
     e. Signature is recorded in the Rekor transparency log
  3. Loop package is pushed to the registry with signatures attached
```

#### Verification Flow

```
User installs a loop:

  1. hive loop install hivemind-community/deep-research@1.2.0
  2. HiveMind OS downloads the loop package
  3. Verification:
     a. Check Sigstore signature bundles against file contents (integrity)
     b. Verify the signing certificate was issued to a known identity (authenticity)
     c. Confirm the signature exists in the Rekor transparency log (non-repudiation)
     d. Check the identity against the user's trust policy (authorisation)
  4. If verification fails → installation is blocked, user is warned
  5. If verification passes → loop is installed and available
```

#### Trust Policies

Users configure who they trust:

```yaml
# ~/.hivemind/config.yaml
loop_registry:
  url: https://loops.hivemind.dev
  trust:
    # Trust any loop signed by these identities
    trusted_authors:
      - github:hivemind-community/*     # Any member of the org
      - github:janedoe
      - github:my-company/*

    # Trust policies
    require_signature: true           # Never run unsigned loops
    require_transparency_log: true    # Signature must appear in Rekor
    allow_unsigned_local: true        # Allow running local dev loops without signing

    # Verification cache
    cache_ttl: 7d                     # Re-verify after this period
```

#### Unsigned / Local Development

During development, authors can run loops locally without signing:

```bash
# Run a local loop (no signature required if allow_unsigned_local: true)
hive loop run ./my-loop/ --query "test"

# But the UI shows a clear warning:
#   ⚠ Running unsigned loop from local filesystem
```

### 9.11 Loop Event Protocol

Whether authored in DSL or code, all loops communicate via a standard event stream:

```typescript
type LoopEvent =
  | { type: 'thinking',       content: string }
  | { type: 'intent',         summary: string }
  | { type: 'tool_call',      call: ToolCall }
  | { type: 'tool_result',    result: ToolResult }
  | { type: 'model_call',     request: ModelRequest }
  | { type: 'model_result',   response: ModelResponse }
  | { type: 'memory_write',   operation: MemoryOp }
  | { type: 'user_prompt',    question: string }
  | { type: 'response',       content: string }
  | { type: 'state_updated',  patch: JSONPatch }
  | { type: 'checkpoint',     snapshot: StateSnapshot }
  | { type: 'stage_entered',  stage: string }
  | { type: 'stage_completed', stage: string, duration_ms: number }
  | { type: 'error',          error: AgentError }
  | { type: 'paused',         reason: string }
  | { type: 'interrupted',    partial_results: PartialResults }
  | { type: 'resumed',        from_checkpoint: string }
  | { type: 'progress',       message: string, current: number, total: number }
  | { type: 'content_scanned', verdict: ScanVerdict }
  | { type: 'context_compacted', summary: CompactionSummary }
  | { type: 'memory_recalled',  query: string, nodes: string[], tokens: number }
```

### 9.12 Context Compaction

Long-running agent sessions inevitably outgrow the model's context window. Rather than silently dropping early turns or crashing, HiveMind OS uses a **structured compaction** strategy that extracts knowledge into the graph before pruning conversation history.

#### 9.12.1 The Problem

| Model | Context window | ~Turns before full |
|-------|---------------|--------------------|
| GPT-4o | 128K tokens | ~50–80 turns |
| Claude Sonnet | 200K tokens | ~80–120 turns |
| Local 7B GGUF | 4–8K tokens | ~3–5 turns |

Background agents running overnight or multi-day research tasks can easily reach hundreds of turns. Without compaction, the agent either loses early context or halts.

#### 9.12.2 Strategy: Extract → Summarize → Prune → Reconstruct

```
┌─────────────────────────────────────────────────────────┐
│                    Context Buffer                       │
│  [system] [compaction_summary] [...recent turns...]     │
│                                                         │
│  Token usage: ████████████░░░░ 75% ← trigger threshold  │
└───────────────────────┬─────────────────────────────────┘
                        │ threshold reached
                        ▼
┌──────────────────────────────────────┐
│         Compaction Pipeline          │
│                                      │
│  1. EXTRACT  → KG nodes & edges      │
│  2. SUMMARIZE → CompactionSummary    │
│  3. PRUNE    → drop old raw turns    │
│  4. RECONSTRUCT → on-demand recall   │
└──────────────────────────────────────┘
```

**Phase 1 — Extract.** The compaction model (typically the `admin` model role — a fast, cheap model) analyzes the turns being compacted and produces structured knowledge:

- **Entities** mentioned (people, projects, repos, URLs) → `Entity` nodes
- **Observations** (discrete facts learned) → `Observation` nodes linked via `OBSERVED_IN` to the `Conversation` node
- **Decisions** made → `Observation` nodes with `kind: decision`
- **Preferences** expressed → `Preference` nodes linked via `PREFERS`
- **Tool results** of lasting value → `Artifact` nodes with content summaries
- All edges link back to the `Conversation` node and carry the turn range and `data_class`

```sql
-- Example: a compaction extraction creates nodes
INSERT INTO nodes (id, kind, label, data_class, source, properties)
VALUES (
  'obs-abc123', 'Observation',
  'User decided to use PostgreSQL for the auth database',
  2, -- CONFIDENTIAL (inherits from conversation)
  '{"session_id": "sess-42", "turn_range": [0, 35], "confidence": 0.95}',
  '{"category": "decision", "domain": "architecture"}'
);

-- With embedding for later vector recall
INSERT INTO vec_nodes (rowid, embedding)
VALUES (last_insert_rowid(), ?); -- 384-dim embedding of the observation text
```

**Phase 2 — Summarize.** The compaction model also generates a prose summary of the compacted turns. This is stored as a `CompactionSummary` node in the KG and injected into the context buffer as a replacement for the dropped turns.

```typescript
interface CompactionSummary {
  session_id:     string
  turn_range:     [number, number]   // [first_turn, last_turn] that were compacted
  summary:        string             // Prose summary (target: 500-1000 tokens)
  extracted_nodes: string[]          // IDs of KG nodes created during extraction
  token_budget:   number             // Tokens freed by this compaction
  data_class:     DataClass          // Max classification of compacted content
  created_at:     string
}
```

**Phase 3 — Prune.** Raw turns within the compacted range are removed from `LoopContext.history`. The context buffer now contains:

1. **System prompt** (always retained)
2. **Compaction summaries** (one per past compaction, ordered chronologically)
3. **Active task state** (current plan, pending sub-tasks)
4. **Recent turns** (within the keep window)

**Phase 4 — Reconstruct (on-demand).** When the agent needs historical context that was compacted away, it queries the KG:

```
Agent: "What database did the user choose for auth?"
  → vector search: embed("database auth choice") → KNN over vec_nodes
  → FTS5: MATCH 'database AND auth AND (choice OR decision OR selected)'
  → graph traversal: find Observation nodes linked to current Conversation
  → result: "User decided to use PostgreSQL for the auth database" (confidence: 0.95)
```

This retrieved context is injected as a `memory_recall` block in the next prompt, not permanently re-added to the history.

#### 9.12.3 Why the Knowledge Graph Beats Simple Summarization

| Approach | Weakness | KG Advantage |
|----------|----------|--------------|
| Rolling summary | Lossy — details get averaged away over time | Structured nodes preserve discrete facts |
| Sliding window | No memory of early context at all | Extracted knowledge is queryable forever |
| Flat summary doc | No selective recall — all or nothing | Vector + FTS5 retrieves only what's relevant |
| External vector DB | No relationships, no structure | Graph edges capture *how* facts relate |

Additional KG benefits:
- **Cross-session**: Knowledge extracted in session A is available in session B without re-processing
- **Classification-aware**: Compacted nodes inherit `data_class`; a PUBLIC-channel agent can't recall CONFIDENTIAL observations
- **Deduplication**: The existing Memory Manager `consolidate()` operation merges duplicate observations across compactions
- **Decay**: Unused compacted knowledge naturally loses confidence via the existing decay system — the graph self-prunes stale facts
- **Audit trail**: Every compaction is logged with turn range, nodes created, and tokens freed

#### 9.12.4 Compaction Configuration

```yaml
agent_loop:
  context_compaction:
    strategy: extract-and-summarize   # extract-and-summarize | summarize-only | manual
    trigger_threshold: 0.75           # Compact when context reaches 75% of model's max
    keep_recent_turns: 10             # Always keep the last N turns in raw form
    summary_max_tokens: 800           # Target size for each compaction summary
    extraction_model: admin           # Model role used for extraction (cheap/fast)
    max_summaries_in_context: 5       # Oldest summaries are themselves compacted (recursive)
    auto_extract_entities: true       # Create Entity/Observation nodes during compaction
    auto_embed: true                  # Generate embeddings for extracted nodes
    preserve_tool_results: true       # Keep high-value tool outputs as Artifact nodes
```

**`strategy` options:**

| Strategy | Behavior | Best for |
|----------|----------|----------|
| `extract-and-summarize` | Full pipeline — extract KG nodes + prose summary | Default; long sessions; knowledge-heavy work |
| `summarize-only` | Prose summary only, no KG extraction | Low-cost mode; casual conversations |
| `manual` | Never auto-compact; user triggers via `/compact` command | Control-sensitive workflows |

#### 9.12.5 Recursive Compaction

When the number of compaction summaries in context exceeds `max_summaries_in_context`, the oldest summaries are themselves compacted into a single "epoch summary." This creates a logarithmic compression of history:

```
Epoch 0:  [turns 0-35 summary] [turns 36-70 summary] [turns 71-105 summary]
    ↓ oldest 3 summaries compacted
Epoch 1:  [epoch summary: turns 0-105] [turns 106-140 summary] [turns 141-...]
```

Each epoch summary links to its constituent `CompactionSummary` nodes in the KG via `DERIVED_FROM` edges, preserving the full compaction chain for audit.

#### 9.12.6 Middleware Integration

Compaction runs as a `LoopMiddleware` — `context_compactor` — that hooks into `beforeModelCall`:

```typescript
class ContextCompactor implements LoopMiddleware {
  name = 'context_compactor'

  beforeModelCall(ctx: LoopContext, request: ModelRequest): ModelRequest | Block {
    const usage = estimateTokens(ctx.history)
    const limit = getModelContextLimit(request.model)

    if (usage / limit >= ctx.config.compaction.trigger_threshold) {
      const result = this.compact(ctx)
      // Replace old turns with summary; inject any recalled context
      ctx.history = [
        ctx.history[0],            // system prompt
        ...result.summaries,       // compaction summaries
        ...result.recentTurns,     // kept recent turns
      ]
    }
    return request
  }
}
```

#### 9.12.7 Loop Event

Compaction events use the `context_compacted` and `memory_recalled` event types already defined in the Loop Event Protocol (§9.11). No additional event types are needed.

#### 9.12.8 User Controls

- **`/compact`** — manually trigger compaction
- **`/recall <query>`** — explicitly search compacted memories and inject into context
- **`/history`** — view full conversation including compacted regions (expandable in UI)
- **Notification** — when auto-compaction fires, the UI shows a brief "Context compacted: turns 1-35 → knowledge graph (12 facts extracted)"

### 9.13 Session Forking

Because every session is an ordered event log (§9.7), HiveMind OS can **fork** a session at any point in its history — creating a new independent session that shares ancestry up to the fork point but diverges from there. This is analogous to `git branch`.

#### Use Cases

| Scenario | Example |
|---|---|
| **Explore alternatives** | "What if we used PostgreSQL instead?" — fork, try it, compare results side by side |
| **Undo without losing work** | Fork from 20 turns ago, keep the original intact for reference |
| **Template conversations** | Set up a session with system prompt + context, fork it for each new task |
| **Multi-path research** | Fork a research session into 3 branches, each exploring a different angle |
| **Safe experimentation** | Fork before asking the agent to make destructive changes |

#### Fork Mechanics

```
Original Session (run_id: "abc-123")
  Event #1  stage_entered     {plan}
  Event #2  model_call_end    {response: "..."}
  Event #3  stage_completed   {plan}
  Event #4  stage_entered     {search}        ← fork point
  Event #5  tool_call_start   {web_search}
  Event #6  tool_call_end     {results: [...]}
  ...

Fork at event #4 → New Session (run_id: "def-456")
  parent_run_id: "abc-123"
  fork_point:    4                             ← shares events 1-4
  Event #5'  stage_entered    {code}           ← diverges here
  Event #6'  model_call_start {...}
  ...
```

#### Implementation

A fork is **copy-on-write** — the new session references the parent's events up to the fork point rather than copying them:

```typescript
interface SessionFork {
  run_id:          string    // New unique session ID
  parent_run_id:   string    // The session being forked
  fork_point:      number    // Event sequence number to fork at
  fork_reason:     string    // User-provided label (optional)
  forked_at:       timestamp
}
```

**State reconstruction** for a forked session:
1. Load the parent session's checkpoint at or before `fork_point`
2. Replay parent events from checkpoint up to `fork_point`
3. Continue replaying the fork's own events from `fork_point + 1`

**Storage**: Only the fork's divergent events are stored. The shared prefix is accessed via `parent_run_id`. This enables cheap forking — forking a 500-event session costs zero event copies.

#### Fork Types

| Type | Description |
|---|---|
| **Head fork** | Fork from the current latest event (default). Equivalent to "branch from here." |
| **Historical fork** | Fork from any past event by sequence number. Equivalent to "go back to step N and try something different." |
| **Conversation fork** | Fork the conversation context (messages, KG nodes in scope) but start a fresh workflow run. Good for "same context, different task." |

#### Fork Tree

Sessions form a tree. The UI can display this as a branch graph (like git log --graph):

```
abc-123  ─── E1 ─── E2 ─── E3 ─── E4 ─── E5 ─── E6 ─── E7
                                    │
def-456                             └─── E5' ─── E6' ─── E7'
                                                   │
ghi-789                                            └─── E7'' ─── E8''
```

**API**:
```
POST /sessions/{run_id}/fork
  { fork_point?: number, reason?: string, type: "head" | "historical" | "conversation" }

GET  /sessions/{run_id}/forks        → list child forks
GET  /sessions/{run_id}/ancestors    → walk up the fork tree
```

**User commands**:
- **`/fork`** — fork the current session at HEAD
- **`/fork @N`** — fork at event N
- **`/fork "trying a different approach"`** — fork with a label
- **`/forks`** — list all forks of the current session
- **`/switch <run_id>`** — switch to a different fork

#### Data Classification

A forked session **inherits the effective data classification** of all events up to the fork point. The fork cannot downgrade classifications — if event #3 introduced CONFIDENTIAL data, all forks from event #3 or later carry at least CONFIDENTIAL.

```
effective_class(fork) = max(
  max(class(event) for event in parent_events[0..fork_point]),
  max(class(event) for event in fork_events)
)
```

#### Knowledge Graph Integration

When a session is forked:
- KG nodes created by the parent session (up to fork point) are **visible** to the fork (read-only reference)
- KG nodes created by the fork are **owned** by the fork
- If a fork modifies a parent KG node, a **copy-on-write clone** is created with the fork's `run_id` as owner
- The `/recall` command (§9.12) searches across the fork's own nodes and its ancestor chain

### 9.14 Visual Loop Designer

While `.loop.yaml` files (§9.5) are powerful, many users prefer a **visual, drag-and-drop** approach to designing agentic loops. HiveMind OS includes a canvas-based loop designer that produces the same `.loop.yaml` output — no separate runtime, no lock-in.

#### Design Principles

| Principle | Rationale |
|---|---|
| **Canvas is a view, YAML is truth** | The designer reads and writes `.loop.yaml` — you can always switch between visual and text editing. Round-tripping is lossless. |
| **Template-first** | Users start from a library of curated templates rather than a blank canvas. Reduces the "blank page" problem. |
| **Progressive disclosure** | Simple loops look simple on the canvas. Advanced features (error handlers, security policies, custom stages) are revealed on demand. |
| **Live preview** | Run the loop against test inputs directly from the canvas — see execution flow highlighted in real time. |

#### Canvas Elements

```
┌─────────────────────────────────────────────────────────────────┐
│  Loop Designer: deep-research v1.2.0                    ▶ Run  │
├───────────┬─────────────────────────────────────────────────────┤
│           │                                                     │
│ STAGES    │    ┌──────┐     ┌────────┐     ┌─────────┐         │
│           │    │ plan │────►│ search │────►│ analyse │         │
│ ● model   │    └──────┘     └────────┘     └────┬────┘         │
│ ● tool    │                                     │               │
│ ● parallel│                               ┌─────▼─────┐        │
│ ● branch  │              ┌──────────┐     │  verify?  │        │
│ ● human   │              │ verified │◄────┤ condition │        │
│ ● memory  │              └────┬─────┘     └─────┬─────┘        │
│ ● custom  │                   │                  │ skip          │
│ ● sub-loop│              ┌────▼─────┐     ┌─────▼─────┐        │
│           │              │ evaluate │     │synthesise │        │
│ TEMPLATES │              └────┬─────┘     └───────────┘        │
│           │                   │ low confidence                  │
│ 📋 Research│                   └──────────► plan (loop back)    │
│ 📋 Code    │                                                    │
│ 📋 Chat    │                                                    │
│ 📋 RAG     │                                                    │
│ 📋 Review  │                                                    │
│           ├─────────────────────────────────────────────────────┤
│           │  PROPERTIES: plan (model_call)                      │
│           │  ┌─────────────────────────────────────────────┐    │
│           │  │ Prompt: Given this research query: {{...}}  │    │
│           │  │ Output: state.plan                          │    │
│           │  │ Model:  auto                                │    │
│           │  │ Max tokens: 4096                             │    │
│           │  └─────────────────────────────────────────────┘    │
└───────────┴─────────────────────────────────────────────────────┘
```

The canvas has four zones:

| Zone | Content |
|---|---|
| **Stage palette** (left) | Draggable stage types matching the DSL primitives (§9.5). Drag onto canvas to add. |
| **Canvas** (centre) | Node-and-edge graph of stages. Click to select, drag to reposition, draw edges between stages to define transitions. |
| **Properties panel** (bottom/right) | Edit the selected stage's configuration: prompts, tools, conditions, error handlers, security constraints. |
| **Template browser** (left, toggled) | Browse and search the template library. Click a template to load it onto the canvas. |

#### Canvas Interactions

| Action | Behaviour |
|---|---|
| **Drag stage from palette** | Creates a new stage node on the canvas |
| **Draw edge between stages** | Creates a `next` transition; conditional stages get a labelled edge per branch |
| **Click stage** | Opens properties panel with all configurable fields |
| **Double-click stage** | Inline-edit the stage name |
| **Right-click stage** | Context menu: duplicate, delete, wrap in sub-loop, convert type, set as entry point |
| **Ctrl+drag** | Pan the canvas |
| **Scroll wheel** | Zoom in/out |
| **Cmd/Ctrl+Z / Y** | Undo / redo (full history) |
| **Cmd/Ctrl+S** | Save — writes to `.loop.yaml` |

#### Template Library

Templates are pre-built `.loop.yaml` files bundled with HiveMind OS and available from the Loop Registry (§9.9). They serve as starting points:

| Template | Description | Stages |
|---|---|---|
| **Simple Chat** | Basic request→response loop | `respond` → `done` |
| **ReAct** | Reason + Act pattern with tool use | `think` → `act` → `observe` → `evaluate` |
| **Deep Research** | Multi-pass research with verification | `plan` → `search` → `analyse` → `verify` → `synthesise` |
| **Code Assistant** | Code generation with test-driven iteration | `understand` → `plan` → `code` → `test` → `refine` |
| **RAG Pipeline** | Retrieval-augmented generation | `retrieve` → `rerank` → `generate` → `cite` |
| **Code Review** | Automated code review with multi-pass analysis | `diff` → `analyse` → `annotate` → `summarise` |
| **Data Pipeline** | Extract, transform, validate data workflows | `extract` → `validate` → `transform` → `load` |
| **Creative Writing** | Outline → draft → critique → revise loop | `outline` → `draft` → `critique` → `revise` → `polish` |
| **Multi-Agent Coordinator** | Delegates sub-tasks to agent roles (§10) | `decompose` → `delegate` → `collect` → `merge` |
| **Conversational** | Stateful multi-turn with memory | `recall` → `respond` → `remember` → `await` |

Templates from the community registry (§9.9) are installable directly from the template browser. Sigstore signatures (§9.10) are verified before loading.

#### YAML ↔ Canvas Round-Tripping

The canvas is a **bidirectional editor** for `.loop.yaml`:

```
┌─────────────┐      parse       ┌───────────────┐      serialize     ┌─────────────┐
│ .loop.yaml  │ ───────────────► │ Canvas State  │ ─────────────────► │ .loop.yaml  │
│   (on disk) │                  │ (in memory)   │                    │  (updated)  │
└─────────────┘                  └───────────────┘                    └─────────────┘
                                        ▲
                                        │ user edits
                                        │ (drag, drop, configure)
```

- **Import**: Opening a `.loop.yaml` parses it into canvas nodes with auto-layout (dagre/elkjs)
- **Export**: Every canvas action immediately updates the in-memory YAML AST; saving writes it back
- **Layout metadata**: Node positions are stored in a companion `.loop.layout.json` file (not in the YAML) to keep the YAML clean
- **Validation**: The canvas enforces structural rules in real time — unreachable nodes, missing transitions, type mismatches — highlighted with red badges

#### Live Preview & Debugging

The designer includes a built-in **execution visualiser**:

1. **Test mode**: Click ▶ Run with test inputs. The canvas highlights each stage as it executes, showing:
   - Execution time per stage
   - State mutations (JSON diff)
   - Model/tool call details (expandable)
   - Data classification of each step

2. **Replay mode**: Load a past workflow run (from the event log, §9.7) and replay it on the canvas. See exactly which path the agent took, where it looped, and why.

3. **Breakpoints**: Click the left edge of a stage node to set a breakpoint. Execution pauses there, showing current state, and you can step forward or modify state before continuing.

4. **Dry run**: Execute with mocked model/tool responses to test control flow without incurring API costs.

#### Custom Stage Editing

When a `custom_stage` node is selected, the properties panel includes an **embedded code editor** (Monaco) for the TypeScript handler. The editor provides:
- Syntax highlighting and autocomplete for the HiveMind OS sandbox API (§9.6)
- Inline type checking against the stage's declared input/output schema
- A "Test this stage" button that runs just that stage in isolation

#### Data Classification Overlay

A toggle in the toolbar activates a **classification overlay** on the canvas:
- Each stage node is colour-coded by its effective data classification (green/blue/amber/red)
- Edges crossing classification boundaries are highlighted with a warning icon
- The security section of the `.loop.yaml` is editable from a dedicated canvas panel

---

## 10. Roles & Multi-Agent System

HiveMind OS supports **user-defined roles** — persistent agent personas with their own identity, tools, knowledge access, and security posture. Users can spin up conversations and background agents in any role, and live agent instances can communicate, delegate work, and collaborate.

### 10.1 Role Definitions

A role is a reusable agent configuration:

```yaml
# ~/.hivemind/roles/code-reviewer.role.yaml
name: code-reviewer
display_name: "Code Reviewer"
avatar: 🔍
description: "Reviews code changes for bugs, security issues, and style."

# Personality & behaviour
system_prompt: |
  You are a senior code reviewer. You focus on correctness, security,
  and maintainability. You are thorough but respectful. You always
  explain *why* something is a problem, not just *that* it is.

# Model preferences
model:
  preferred: anthropic/claude-sonnet-4
  fallback: [openai/gpt-4o, ollama-local/deepseek-coder]
  temperature: 0.2

# Agentic loop
loop:
  strategy: reflexion
  max_iterations: 15

# Tool access — scoped per role
tools:
  allow:
    - filesystem.read
    - filesystem.diff
    - code.lint
    - code.test
    - github.get_pr
    - github.get_diff
    - github.create_review
    - knowledge_graph.recall
    - knowledge_graph.remember
    - agents.send_message            # Can talk to other agents
    - agents.delegate_task           # Can spawn sub-agents
  deny:
    - filesystem.write               # Reviewers don't modify code
    - shell.exec

# Knowledge graph access
knowledge:
  read_scopes:  [code-patterns, project-conventions, past-reviews]
  write_scopes: [past-reviews]
  auto_observe:  true                # Remember review outcomes

# Security
security:
  data_class_ceiling: INTERNAL       # Can handle up to INTERNAL data
  allowed_channels: [private, internal]
  can_override_classification: false # Cannot approve classification boundary crossings

# Inter-agent communication
communication:
  can_initiate: true                 # Can start conversations with other agents
  can_receive: true                  # Can receive messages from other agents
  visible_roles: [developer, architect, qa-engineer]  # Which roles it can see/contact
  broadcast_channels: [team]         # Pub/sub channels it subscribes to
```

#### Built-in Role Templates

| Role | Purpose |
|---|---|
| `assistant` | Default general-purpose assistant. Full tool access, user-facing. |
| `researcher` | Deep web/knowledge research. Read-heavy, no filesystem writes. |
| `developer` | Code writing and modification. Full filesystem + shell access. |
| `reviewer` | Code review. Read-only filesystem, can create reviews. |
| `architect` | High-level design. Knowledge graph heavy, delegates implementation. |
| `ops` | DevOps tasks. Shell, deployment tools, monitoring. |
| `scribe` | Note-taking and summarisation. Writes to knowledge graph. |

Users create custom roles from scratch or by extending templates:

```yaml
# Inherit from a template and override
name: my-reviewer
extends: reviewer
system_prompt_append: |
  Also check for compliance with our internal style guide at
  https://internal.corp/style-guide
tools:
  allow_extra:
    - corporate-kb.search
```

### 10.2 Agent Instances

An **agent instance** is a live, running incarnation of a role. Multiple instances of the same role can exist simultaneously.

```typescript
interface AgentInstance {
  id:              string         // Unique instance ID
  role:            RoleDefinition // The role this agent was created from
  status:          'active' | 'idle' | 'background' | 'paused' | 'terminated'
  mode:            'conversation' | 'background' | 'daemon'
  
  // Conversation state
  conversation_id: string
  history:         Message[]
  loop_context:    LoopContext
  
  // Workflow state (if running a loop)
  workflow_run_id?: string
  
  // Communication
  inbox:           MessageQueue
  subscriptions:   string[]       // Pub/sub channels
  
  // Resource tracking
  tokens_used:     number
  cost_accrued:    number
  started_at:      timestamp
  last_active_at:  timestamp
}
```

#### Spawning Agents

```bash
# Interactive conversation in a role
hive chat --role code-reviewer

# Background agent running a loop
hive agent start --role researcher --loop deep-research \
  --param query="latest Rust async patterns" --background

# Daemon agent (always-on, event-driven)
hive agent start --role ops --loop monitor-deployments --daemon
```

Programmatically (from within another agent or loop):

```yaml
# In a loop DSL
stages:
  delegate_review:
    type: agent_spawn
    role: code-reviewer
    mode: background
    input:
      task: "Review the diff in {{state.pr_url}}"
    wait: true               # Block until the spawned agent completes
    output: state.review
```

```typescript
// In custom TypeScript stage
const reviewer = await ctx.agents.spawn({
  role: 'code-reviewer',
  mode: 'background',
  input: { task: 'Review this diff', diff: state.diff }
})
const result = await reviewer.waitForCompletion()
```

### 10.3 Inter-Agent Communication

Live agent instances communicate through a structured message-passing system.

#### Communication Patterns

```
┌──────────────────────────────────────────────────────────────────┐
│                   Inter-Agent Communication                      │
│                                                                  │
│  ┌────────────┐   direct message   ┌────────────┐               │
│  │ Developer  │ ──────────────────► │ Reviewer   │               │
│  │ Agent      │ ◄────────────────── │ Agent      │               │
│  └────────────┘   response          └────────────┘               │
│        │                                   │                     │
│        │ delegate                          │ broadcast            │
│        ▼                                   ▼                     │
│  ┌────────────┐                     ┌────────────┐               │
│  │ Researcher │                     │ "team"     │ pub/sub       │
│  │ Agent      │                     │  channel   │ channel       │
│  └────────────┘                     └────┬───────┘               │
│                                          │                       │
│                           ┌──────────────┼──────────────┐        │
│                           ▼              ▼              ▼        │
│                      Developer      Architect       QA Agent     │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │                    Shared Blackboard                         │ │
│  │  (Knowledge graph namespace visible to collaborating agents) │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

#### 1. Direct Messaging

Point-to-point communication between two agent instances:

```typescript
interface AgentMessage {
  id:           string
  from:         AgentRef         // { instance_id, role }
  to:           AgentRef
  type:         MessageType
  content:      any
  data_class:   DataClass        // Classification of message content
  reply_to?:    string           // For threaded conversations
  timeout?:     duration         // How long to wait for a response
  timestamp:    timestamp
}

type MessageType =
  | 'request'           // Ask another agent to do something
  | 'response'          // Reply to a request
  | 'inform'            // Share information (no response expected)
  | 'delegate'          // Hand off a task entirely
  | 'status_update'     // Progress update on delegated work
  | 'escalate'          // Escalate to a higher-authority role
  | 'query'             // Ask a question, expect an answer
```

Usage from within an agent:

```typescript
// Developer agent asks the reviewer for feedback
const review = await ctx.agents.send({
  to: { role: 'code-reviewer' },    // Route to any available instance of this role
  type: 'request',
  content: {
    task: 'review_code',
    diff: myDiff,
    focus: ['security', 'error-handling']
  },
  timeout: '5m'
})

// Or target a specific instance
const answer = await ctx.agents.send({
  to: { instance_id: 'agent-abc-123' },
  type: 'query',
  content: { question: 'What did you find about the auth module?' }
})
```

#### 2. Pub/Sub Channels

Broadcast communication for team-wide coordination:

```typescript
interface PubSubChannel {
  name:          string           // e.g., "team", "alerts", "code-changes"
  subscribers:   AgentRef[]
  data_class:    DataClass        // Max classification for messages on this channel
  persistent:    boolean          // Whether to retain messages for late joiners
  max_history:   number           // Messages to retain
}

// Agent subscribes and publishes
ctx.agents.subscribe('team', (msg) => {
  // React to team-wide broadcasts
})

ctx.agents.publish('team', {
  type: 'inform',
  content: { event: 'pr_merged', pr: '#42', repo: 'forge' }
})
```

#### 3. Shared Blackboard (Knowledge Graph Namespaces)

Agents that need to collaborate on a shared body of knowledge can read/write to **knowledge graph namespaces** that serve as a shared blackboard:

```yaml
# In role definition
knowledge:
  read_scopes:  [shared/project-alpha, team-findings]
  write_scopes: [shared/project-alpha]
```

```typescript
// Researcher agent writes findings to the shared namespace
await ctx.memory.remember(
  'The auth module uses PBKDF2 with 100k iterations',
  { scope: 'shared/project-alpha', about: authModuleRef }
)

// Developer agent reads from the same namespace
const findings = await ctx.memory.recall(
  'auth module security',
  { scope: 'shared/project-alpha' }
)
```

#### 4. Task Delegation

An agent can delegate an entire task to another role, creating a supervised child agent:

```typescript
interface DelegationRequest {
  to_role:        string          // Target role
  task:           string          // Natural language task description
  input:          any             // Structured input data
  loop?:          string          // Specific loop to use (optional)
  supervision:    SupervisionLevel
  timeout?:       duration
  data_class:     DataClass       // Classification ceiling for the delegated work
}

type SupervisionLevel =
  | 'autonomous'       // Run to completion, return results
  | 'check_in'         // Periodic status updates to the delegating agent
  | 'approval_gates'   // Pause at key decision points for parent approval
  | 'pair'             // Real-time collaboration (message stream)
```

```typescript
// Architect agent delegates implementation to a developer
const result = await ctx.agents.delegate({
  to_role: 'developer',
  task: 'Implement the caching layer per the design doc',
  input: { design_doc: state.design, target_dir: 'src/cache/' },
  supervision: 'approval_gates',
  data_class: DataClass.INTERNAL
})
```

### 10.4 Inter-Agent Security

Agent communication respects the same data classification model:

| Rule | Enforcement |
|---|---|
| **Message classification** | Every message carries a `data_class`. An agent cannot send a message whose classification exceeds the recipient's `data_class_ceiling`. |
| **Channel classification** | Pub/sub channels have a `data_class` ceiling. Messages exceeding it are blocked. |
| **Role visibility** | Agents can only see/contact roles listed in their `visible_roles`. This prevents a low-clearance agent from even discovering a high-clearance role. |
| **Delegation ceiling** | Delegated tasks inherit the minimum of the parent's and child's `data_class_ceiling`. A public-clearance agent cannot escalate data to a private-clearance agent to launder it. |
| **Blackboard scoping** | Knowledge graph namespace access is controlled per-role. An agent cannot read/write scopes it hasn't been granted. |
| **Audit trail** | All inter-agent messages are logged with sender, recipient, classification, and content hash. |

### 10.5 Coordination Patterns

Common multi-agent patterns that emerge from the primitives above:

#### Pipeline

```
User Request → [Researcher] → findings → [Developer] → code → [Reviewer] → approved PR
```

```yaml
# pipeline.loop.yaml
stages:
  research:
    type: agent_spawn
    role: researcher
    input: { query: "{{state.request}}" }
    wait: true
    output: state.findings

  implement:
    type: agent_spawn
    role: developer
    input: { task: "Implement based on findings", findings: "{{state.findings}}" }
    wait: true
    output: state.code

  review:
    type: agent_spawn
    role: code-reviewer
    input: { task: "Review implementation", diff: "{{state.code.diff}}" }
    wait: true
    output: state.review
```

#### Debate / Adversarial Review

```
[Advocate Agent] ←→ [Critic Agent] → consensus → [Synthesiser Agent]
```

Two agents argue opposing positions, surfacing blind spots:

```yaml
stages:
  advocate:
    type: agent_spawn
    role: advocate
    input: { position: "We should use microservices", context: "{{state.context}}" }
    wait: true
    output: state.pro_case

  critic:
    type: agent_spawn
    role: critic
    input: { position: "We should use a monolith", pro_case: "{{state.pro_case}}" }
    wait: true
    output: state.counter_case

  synthesise:
    type: agent_spawn
    role: architect
    input:
      task: "Synthesise the best approach"
      pro: "{{state.pro_case}}"
      con: "{{state.counter_case}}"
    wait: true
    output: state.decision
```

#### Swarm / Parallel Workers

Fan out work to multiple instances of the same role:

```yaml
stages:
  distribute:
    type: parallel_agent_spawn
    role: researcher
    for_each: state.questions        # One agent per question
    input: { query: "{{item}}" }
    max_concurrent: 5
    output: state.answers            # Array of results

  merge:
    type: model_call
    prompt_template: |
      Merge these research results into a coherent report:
      {{state.answers | json}}
    output: state.report
```

#### Supervisor / Worker

A long-lived supervisor monitors and coordinates workers:

```yaml
name: supervised-team
stages:
  supervise:
    type: agent_spawn
    role: architect
    mode: daemon                     # Long-lived
    input:
      task: "Coordinate the implementation of {{state.epic}}"
      team:
        - role: developer
          count: 2
        - role: reviewer
          count: 1
      supervision: approval_gates
    output: state.result
```

The supervisor agent uses `agents.delegate` and `agents.send` to assign tasks, review progress, and make decisions — all within its own agentic loop.

### 10.6 Agent Lifecycle

```
┌────────┐   spawn    ┌────────┐   input     ┌─────────┐
│  Role  │ ─────────► │ Active │ ──────────► │ Working │ ─── loop ──┐
│  Defn  │            │ (idle) │             │         │            │
└────────┘            └────────┘             └─────────┘ ◄──────────┘
                          ▲                       │
                          │ resume                │ pause / complete
                          │                       ▼
                     ┌────────┐              ┌─────────────┐
                     │ Paused │              │ Terminated  │
                     │ (state │              │ (result     │
                     │  saved)│              │  persisted) │
                     └────────┘              └─────────────┘
```

- **Active (idle)** — instance exists, waiting for input (conversation mode).
- **Working** — running an agentic loop, processing messages.
- **Paused** — workflow state checkpointed, can resume later.
- **Terminated** — completed or explicitly stopped. Results and conversation history are persisted. The instance can be "replayed" from its event log.

### 10.7 Agent Dashboard

The UI provides a live view of all running agent instances:

```
┌─────────────────────────────────────────────────────────────┐
│  Agents                                            [+ New]  │
├──────────┬──────────┬──────────┬────────────┬───────────────┤
│ Instance │ Role     │ Status   │ Task       │ Messages      │
├──────────┼──────────┼──────────┼────────────┼───────────────┤
│ agent-01 │ 🔍 Reviewer │ Working  │ Review PR #42  │ 3 sent, 1 recv │
│ agent-02 │ 💻 Developer│ Working  │ Implement cache│ 2 sent, 4 recv │
│ agent-03 │ 📚 Researcher│ Idle    │ —              │ 0              │
│ agent-04 │ 🏗️ Architect│ Working  │ Supervising    │ 7 sent, 5 recv │
└──────────┴──────────┴──────────┴────────────┴───────────────┘

[Click any agent to view its conversation, message log, and workflow state]
```

---

## 11. Rich Tooling

### 11.1 Built-in Tools

| Category | Tools |
|---|---|
| **Filesystem** | read, write, list, search, glob, diff, patch |
| **Shell** | execute commands (sandboxed, configurable approval) |
| **Web** | fetch URL, search (via configured search provider), screenshot |
| **Code** | syntax-aware edit, refactor, lint, run tests, build |
| **Communication** | send notification, email draft, calendar check |
| **Data** | SQL query (SQLite/DuckDB), CSV/JSON transform, regex |
| **Knowledge Graph** | All MemoryManager operations exposed as tools |
| **System** | clipboard read/write, screenshot, window management |

### 11.2 MCP Tools

Any MCP server's tools are automatically available. The agentic loop discovers them at connection time and can invoke them like built-in tools — subject to `tool_policy` and `classification_gate`.

### 11.3 Agent Skills

HiveMind OS natively supports the **[Agent Skills](https://agentskills.io/specification)** open standard — a portable, file-based format for packaging procedural knowledge, scripts, and reference material that agents can discover and activate on demand.

#### Why Agent Skills?

MCP provides *tools* (callable functions). Agent Skills provide *knowledge and procedures* — how to approach a task, domain-specific instructions, organisational conventions, and scripted workflows. They complement each other:

| | MCP Tools | Agent Skills |
|---|---|---|
| **Nature** | Callable functions with typed I/O | Instructions, scripts, and reference docs |
| **Loaded** | Always available once connected | Activated on demand per task |
| **Authored by** | Developers (code) | Anyone (Markdown + optional scripts) |
| **Portable** | Across MCP-compatible agents | Across Skills-compatible agents |

#### Skill Discovery & Activation

HiveMind OS implements **progressive disclosure** as defined by the spec:

1. **Startup** — HiveMind OS scans configured skill directories and loads all `SKILL.md` frontmatter (`name` + `description`, ~100 tokens each). These are held in memory as a lightweight skill index.
2. **Activation** — when a task matches a skill's description (via keyword matching, semantic similarity, or explicit user request), HiveMind OS loads the full `SKILL.md` body into the agentic loop's context.
3. **Resource loading** — files in `scripts/`, `references/`, and `assets/` are loaded on demand as the agent follows instructions in the skill.

```
┌──────────────────────────────────────────────────────────────┐
│                    Skill Activation Flow                      │
│                                                              │
│  User: "Create a presentation about Q3 results"             │
│                                                              │
│  1. Match against skill index:                               │
│     ✓ "create-presentation" (score: 0.94)                    │
│     · "data-analysis" (score: 0.31)                          │
│                                                              │
│  2. Load SKILL.md body → inject into loop context            │
│                                                              │
│  3. Agent follows instructions, loading files on demand:     │
│     → references/TEMPLATES.md                                │
│     → assets/slide-template.pptx                             │
│     → scripts/generate_charts.py                             │
└──────────────────────────────────────────────────────────────┘
```

#### Skill Configuration

```yaml
# ~/.hivemind/config.yaml
skills:
  # Directories to scan for skills
  paths:
    - ~/.hivemind/skills                  # User skills
    - /opt/corp/hive-skills           # Org-managed skills
    - ./skills                         # Project-local skills

  # Per-skill overrides
  overrides:
    create-presentation:
      data_class: INTERNAL             # Classify this skill's context
      allowed_tools: [filesystem.*, code.*]
    deploy-production:
      data_class: CONFIDENTIAL
      require_confirmation: true       # Always ask before activating

  # Auto-activation behaviour
  activation:
    mode: suggest                      # suggest | auto | manual
    max_concurrent: 3                  # Max skills active at once
    max_body_tokens: 5000              # Reject oversized SKILL.md bodies
```

Activation modes:

| Mode | Behaviour |
|---|---|
| `suggest` | HiveMind OS suggests matching skills; user confirms activation. |
| `auto` | HiveMind OS activates matching skills automatically (within `tool_policy` constraints). |
| `manual` | Skills are only activated via explicit `/skill activate <name>` command. |

#### Skills + Data Classification

Skills integrate with HiveMind OS's security model:

- Each skill gets a `data_class` label (default: `INTERNAL`, configurable per-skill).
- When a skill is activated, its classification merges with the conversation's effective class — the conversation ceiling becomes `max(conversation_class, skill_class)`.
- Scripts in `scripts/` run through the same sandboxed executor as custom loop stages (§9.6), respecting tool policies and classification gates.
- The `allowed-tools` field from the Agent Skills spec maps directly to HiveMind OS's `tool_policy` — pre-approved tools for the skill are auto-allowed; others follow the conversation's policy.

#### Skills + Knowledge Graph

When the agent uses a skill successfully, it can record the outcome in the knowledge graph:

- A **Skill node** (§8.2) links to the Agent Skills directory as its source.
- **Observations** track what worked and what didn't across uses.
- Over time, the agent learns which skills to prefer for which tasks, improving activation accuracy.

#### Skills + Roles

Roles (§10) can declare which skills they have access to:

```yaml
# In a role definition
skills:
  allow: [code-review, create-pr, run-tests]
  deny:  [deploy-production]           # This role can't deploy
```

#### CLI Integration

```bash
# List available skills
hive skill list

# Validate a skill directory
hive skill validate ./my-skill/

# Manually activate in a conversation
hive skill activate create-presentation

# Install from a remote source
hive skill install https://github.com/org/skills-repo/create-presentation

# Create a new skill from a template
hive skill init my-new-skill
```

### 11.4 Tool Metadata

```typescript
interface ToolDefinition {
  id:             string
  name:           string
  description:    string
  input_schema:   JSONSchema
  output_schema?: JSONSchema
  channel_class:  ChannelClass     // What class of data this tool may handle
  side_effects:   boolean          // Does this tool modify external state?
  approval:       'auto' | 'ask' | 'deny'
  annotations: {
    title:              string
    readOnlyHint?:      boolean
    destructiveHint?:   boolean
    idempotentHint?:    boolean
    openWorldHint?:     boolean
  }
}
```

---

## 12. HiveMind OS Peering: Trusted Cross-Machine Connections

HiveMind OS instances on different machines can establish **trusted peer connections**, enabling cross-device agent collaboration, knowledge sharing, and task delegation — all governed by the same classification model.

### 12.1 Peer Identity & Trust Establishment

Each HiveMind OS instance has a **peer identity** — a long-lived key pair generated on first run and stored in the OS keychain.

```
┌─────────────────────────────────────────────────────────────────────┐
│                      Trust Establishment                            │
│                                                                     │
│  Machine A (Alice)                        Machine B (Bob)           │
│  ┌───────────────┐                        ┌───────────────┐         │
│  │ HiveMind OS Instance│                        │ HiveMind OS Instance│         │
│  │ PeerID: hive- │   ── pairing code ──►  │ PeerID: hive- │         │
│  │  a3f8...      │   ◄── accept ────────  │  7b2c...      │         │
│  └───────────────┘                        └───────────────┘         │
│        │                                        │                   │
│        └──── mutual TLS tunnel (Noise Protocol) ┘                   │
│                                                                     │
│  Result: each side stores the other's PeerID as a trusted peer      │
└─────────────────────────────────────────────────────────────────────┘
```

#### Pairing Flow

1. **Initiator** runs `hive peer invite` — generates a short-lived pairing code (e.g., `HIVEMIND-7X2M-K9PL`) displayed as text and QR code.
2. **Acceptor** runs `hive peer join HIVEMIND-7X2M-K9PL` — connects to the initiator via a rendezvous relay (or direct LAN if discoverable).
3. **Mutual verification** — both sides display a verification fingerprint derived from the key exchange. Users confirm they match (prevents MITM).
4. **Trust stored** — each instance records the other's PeerID, display name, and trust level in its local peer registry.

```yaml
# Stored in ~/.hivemind/peers.yaml (encrypted)
peers:
  - peer_id: hivemind-7b2c9e4f
    display_name: "Bob's Workstation"
    owner: bob@example.com
    paired_at: 2025-11-15T10:30:00Z
    trust_level: full                # full | limited | one-way
    data_class_ceiling: INTERNAL     # Max data level we'll share with this peer
    status: active
    last_seen: 2025-12-01T14:22:00Z
    transport:
      preferred: direct              # direct (LAN/Tailnet) | relay
      direct_address: 192.168.1.42:9473
      relay: relay.hivemind.dev
```

#### Trust Levels

| Level | Capabilities |
|---|---|
| `full` | Bidirectional: agent messaging, knowledge sync, task delegation, skill sharing. |
| `limited` | Bidirectional messaging and knowledge queries only. No task delegation or remote tool execution. |
| `one-way` | Asymmetric: one side can query the other, but not vice versa. Useful for team leads monitoring. |

### 12.2 Transport Layer

Peer connections use encrypted, authenticated channels:

- **Protocol:** Noise Framework (IK pattern) over QUIC — provides mutual authentication, forward secrecy, and efficient multiplexing.
- **Direct connections:** When peers are on the same LAN or connected via Tailscale/WireGuard, HiveMind OS connects directly (zero relay latency).
- **Relayed connections:** When direct is not possible, traffic is relayed through a HiveMind OS relay server. The relay is **zero-knowledge** — it sees only encrypted traffic and PeerIDs, never plaintext.
- **Offline queuing:** If a peer is offline, messages and sync deltas are queued locally and delivered when the connection is re-established.

```yaml
# ~/.hivemind/config.yaml
peering:
  enabled: true
  listen_port: 9473
  discovery:
    mdns: true                       # LAN auto-discovery
    tailscale: true                  # Auto-detect Tailscale peers
  relay:
    url: relay.hivemind.dev
    fallback: true                   # Use relay when direct fails
  offline_queue:
    max_size: 50MB
    max_age: 7d
```

### 12.3 Cross-Peer Agent Communication

Once peered, agents on different machines communicate via the same messaging primitives as local agents (§10.3), extended with peer routing:

```typescript
// Send a message to an agent on a remote peer
await ctx.agents.send({
  to: {
    peer: 'hivemind-7b2c9e4f',           // Target peer
    role: 'researcher'                 // Role on that peer
  },
  type: 'request',
  content: { task: 'Look up the latest test results for Project X' },
  data_class: DataClass.INTERNAL,
  timeout: '10m'
})

// Delegate a task to a specific peer
await ctx.agents.delegate({
  to_peer: 'hivemind-7b2c9e4f',
  to_role: 'developer',
  task: 'Run the integration test suite on Linux',
  input: { branch: 'feature/auth-v2' },
  supervision: 'check_in',
  data_class: DataClass.INTERNAL
})
```

#### Cross-Peer Security

| Rule | Enforcement |
|---|---|
| **Classification ceiling** | Each peer link has a `data_class_ceiling`. Data exceeding it is never sent, even if the local agent has access. |
| **Prompt-on-cross-peer** | The override policy (§5.3) applies — if data classified higher than the peer's ceiling would be sent, the user is prompted (or blocked, per policy). |
| **Role visibility** | A peer only exposes roles explicitly listed in its peering config. Internal-only roles remain invisible. |
| **Tool restrictions** | Remote agents cannot invoke tools on the local machine unless explicitly granted in the peer config. |
| **Audit trail** | All cross-peer messages are logged on both sides with peer identity, direction, classification, and content hash. |

```yaml
# What we expose to peers
peering:
  exposed_roles: [researcher, reviewer]   # Only these roles are reachable remotely
  remote_tool_access: false               # Peers cannot invoke our local tools
  allow_remote_delegation: true           # Peers can delegate tasks to our roles
  delegation_requires_approval: true      # User must approve incoming delegations
```

### 12.4 Federated Knowledge Graph

Peered instances can selectively synchronise portions of their knowledge graphs.

#### Sync Model

Knowledge sync is **not full replication** — it's a scoped, classification-aware, bidirectional merge:

```
┌───────────────────┐                    ┌───────────────────┐
│ Alice's Graph     │                    │ Bob's Graph       │
│                   │   sync scope:      │                   │
│ ┌───────────────┐ │  "shared/project"  │ ┌───────────────┐ │
│ │shared/project │◄├────────────────────┤►│shared/project │ │
│ └───────────────┘ │                    │ └───────────────┘ │
│ ┌───────────────┐ │   NOT synced       │ ┌───────────────┐ │
│ │ personal/     │ │                    │ │ personal/     │ │
│ └───────────────┘ │                    │ └───────────────┘ │
└───────────────────┘                    └───────────────────┘
```

#### Sync Configuration

```yaml
peering:
  knowledge_sync:
    peers:
      hivemind-7b2c9e4f:
        sync_scopes:
          - scope: shared/project-alpha
            direction: bidirectional
            class_ceiling: INTERNAL      # Only sync nodes ≤ INTERNAL
            conflict_resolution: last-write-wins
          - scope: team-conventions
            direction: pull              # We read, they don't get ours
            class_ceiling: PUBLIC

    auto_sync_interval: 5m               # How often to sync
    sync_on_change: true                 # Also sync immediately on writes to synced scopes
```

#### Conflict Resolution

When both peers modify the same node between syncs:

| Strategy | Behaviour |
|---|---|
| `last-write-wins` | Most recent `updated_at` timestamp wins. Simple but may lose data. |
| `merge` | Attempt to merge non-conflicting fields. Flag true conflicts for user resolution. |
| `manual` | All conflicts are queued for manual review. Safest for critical data. |
| `higher-confidence` | The version with the higher `confidence` score wins. Good for observations. |

#### Classification Enforcement During Sync

- Nodes with `effective_class` exceeding the sync scope's `class_ceiling` are **silently excluded** from sync.
- If syncing would cause a node's effective class to change (e.g., it gets linked to a higher-classified node on the other side), the sync engine recalculates and may **withdraw** the node from future syncs.
- The sync log records what was synced, what was excluded, and why — visible in the audit log.

### 12.5 Remote Capabilities

Beyond messaging and knowledge sync, peered instances can expose specific capabilities:

#### Shared Model Access

A peer can offer its locally-running models to other peers — useful when one machine has a powerful GPU:

```yaml
peering:
  shared_resources:
    models:
      expose:
        - ollama-local/llama3          # Share this model with peers
        - ollama-local/deepseek-coder
      channel_class: internal          # Treat as internal channel (data goes over the wire)
```

The remote model appears in the peer's provider registry as `peer:hivemind-7b2c9e4f/ollama-local/llama3`, subject to the same routing and classification rules as any other provider.

#### Shared MCP Servers

A peer can proxy access to its MCP servers:

```yaml
peering:
  shared_resources:
    mcp_servers:
      expose:
        - corporate-kb                 # Share access to our corporate knowledge base
      channel_class: internal
```

#### Shared Skills

Agent Skills installed on one peer can be discovered and activated by another:

```yaml
peering:
  shared_resources:
    skills:
      expose: [code-review, deploy-staging]
```

### 12.6 Peer Management CLI

```bash
# Generate a pairing invitation
hive peer invite
# Output: Pairing code: HIVEMIND-7X2M-K9PL (expires in 10 minutes)

# Accept an invitation
hive peer join HIVEMIND-7X2M-K9PL

# List trusted peers
hive peer list

# Check peer status
hive peer status hivemind-7b2c9e4f

# Adjust trust level
hive peer trust hivemind-7b2c9e4f --level limited

# Revoke trust (immediate disconnect, keys deleted)
hive peer revoke hivemind-7b2c9e4f

# Temporarily disconnect without revoking trust
hive peer disconnect hivemind-7b2c9e4f
```

### 12.7 Peer Dashboard (UI)

```
┌──────────────────────────────────────────────────────────────────┐
│  Peers                                              [+ Invite]  │
├──────────────┬───────────┬──────────┬─────────────┬─────────────┤
│ Peer         │ Trust     │ Status   │ Sync Scopes │ Last Seen   │
├──────────────┼───────────┼──────────┼─────────────┼─────────────┤
│ 💻 Bob's WS  │ full      │ 🟢 online │ 2 scopes   │ now         │
│ 🖥️ Build Srv │ limited   │ 🟡 relay  │ 1 scope    │ 2m ago      │
│ 📱 My Laptop │ full      │ 🔴 offline│ 3 scopes   │ 3d ago      │
└──────────────┴───────────┴──────────┴─────────────┴─────────────┘

[Click any peer to view shared roles, sync status, message history]
```

---

## 13. User Interface

### 13.1 Primary Views

| View | Purpose |
|---|---|
| **Chat** | Conversational interface with streaming, tool-call visualisation, inline artifacts, command queuing, interruption controls, and live thinking/intent display (§13.6). |
| **Loop Designer** | Visual canvas for building and debugging agentic loops, with template library (§9.14). |
| **Agents** | Live dashboard of all agent instances — status, role, messages, and workflow state (§10.7). |
| **Peers** | Trusted peer connections — status, sync scopes, shared resources (§12.7). |
| **Tasks** | Dashboard of scheduled and running tasks, with logs and status. |
| **Knowledge** | Graph explorer — visualise nodes, relationships, search, and manually curate. |
| **Tools** | Registry of all available tools (built-in + MCP), with enable/disable and policy controls. |
| **Settings** | Provider configuration, security policies, loop configuration, MCP servers. |
| **Audit Log** | Searchable log of all data-flow decisions, tool calls, and classification events. |

### 13.2 Notifications

- System tray / menu bar presence with notification badges.
- Desktop notifications for: task completion, MCP server events, security alerts, scheduled reminders.
- Notification centre within the app for history and batch actions.

### 13.3 Keyboard-First

- Global hotkey to summon HiveMind OS (e.g., `Ctrl+Space` / `Cmd+Space`).
- Slash commands in the chat for quick actions (`/task`, `/remember`, `/forget`, `/classify`).
- Command palette (`Ctrl+K` / `Cmd+K`) for everything.

### 13.4 First-Run Experience (Zero-Config Onboarding)

HiveMind OS must be **usable within 60 seconds of first launch** — no config files, no terminal commands, no documentation required. The complexity of the full spec is progressively revealed, never front-loaded.

#### Design Principles

| Principle | Detail |
|---|---|
| **Zero required config** | The app launches and works immediately. No `config.yaml` needed. |
| **One decision to start** | The only thing between the user and a working agent is: "How do you want to connect to an AI model?" |
| **Smart defaults** | Everything has a production-quality default. Classification: INTERNAL. Loop: ReAct. Model role mapping: auto. KG: enabled. |
| **Progressive disclosure** | Advanced features (peering, custom loops, roles, classification policies) surface naturally as the user explores. |
| **No dead ends** | Every screen has a clear next action. Empty states have helpful prompts, not blank panels. |

#### First Launch Flow

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│                     🔥 Welcome to HiveMind OS                      │
│                                                              │
│   Your AI agent, running locally, always on.                 │
│                                                              │
│   Let's get you connected to a model:                        │
│                                                              │
│   ┌────────────────────────────────────────────────────┐     │
│   │  🔑  I have an API key                             │     │
│   │      (OpenAI, Anthropic, Google, etc.)             │     │
│   └────────────────────────────────────────────────────┘     │
│   ┌────────────────────────────────────────────────────┐     │
│   │  🐙  Sign in with GitHub                           │     │
│   │      (Use GitHub Copilot models)                   │     │
│   └────────────────────────────────────────────────────┘     │
│   ┌────────────────────────────────────────────────────┐     │
│   │  🌐  Use OpenRouter                                │     │
│   │      (Access 100+ models, pay-as-you-go)           │     │
│   └────────────────────────────────────────────────────┘     │
│   ┌────────────────────────────────────────────────────┐     │
│   │  💻  Run locally (no internet needed)              │     │
│   │      (Download a small model — ~2GB)               │     │
│   └────────────────────────────────────────────────────┘     │
│                                                              │
│   ⚙️  Advanced setup...                                      │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

Each path is **one step**:

| Path | What happens |
|---|---|
| **API key** | Paste a key → HiveMind OS auto-detects the provider (OpenAI, Anthropic, Google, Mistral, Azure, etc.) from the key format. Done. |
| **GitHub** | OAuth browser flow → token stored in OS keychain. Done. |
| **OpenRouter** | OAuth or paste key. Done. |
| **Local model** | Pick from 3 recommended models (sorted by RAM) → download starts with progress bar → ready when downloaded. No GPU config, no quantisation choices — just "Small (2GB) / Medium (4GB) / Large (8GB)". |
| **Advanced** | Full provider configuration form. For power users who want to set up Microsoft Foundry, multiple providers, model role mappings, etc. |

#### Auto-Detection & Smart Defaults

Once a model is connected, HiveMind OS bootstraps a complete working environment with **zero additional input**:

```yaml
# What HiveMind OS configures automatically on first run:
providers:
  - id: <auto-detected>
    api_key: <stored in OS keychain>

model_roles:
  primary: <best available model>
  admin:   <cheapest available model>     # auto-selected
  coding:  <best available model>
  vision:  <best available with vision>   # or null

security:
  default_data_class: INTERNAL
  override_policy:                      # Shorthand: "prompt" expands to all-prompt below
    INTERNAL:
      action: prompt
    CONFIDENTIAL:
      action: prompt
    RESTRICTED:
      action: block
  auto_classify: true

agent_loop:
  default_strategy: react                 # Simple and effective
  context_compaction:
    strategy: extract-and-summarize
    trigger_threshold: 0.75

knowledge_graph:
  storage: ~/.hivemind/knowledge.db          # Created automatically
  auto_consolidate: true

ui:
  theme: system                           # Match OS light/dark
  global_hotkey: ctrl+space               # or cmd+space on macOS
```

The user lands in a **working chat** immediately — no settings, no configuration, no next steps required.

#### Empty States & Progressive Discovery

Every view has a purposeful empty state that teaches without blocking:

| View | Empty State |
|---|---|
| **Chat** | Pre-filled greeting from the default assistant role: "Hi! I'm your HiveMind OS agent. Ask me anything, or try one of these: [Research a topic] [Summarise a document] [Help me code]" with clickable suggestions. |
| **Agents** | "You're chatting with the default assistant. Create specialised agents for different tasks → [Create an agent]" with a link to role templates. |
| **Tasks** | "No scheduled tasks yet. You can schedule recurring jobs or one-off background tasks. Try: `/task remind me to review PRs every morning at 9am`" |
| **Knowledge** | "Your knowledge graph is empty — it grows automatically as you chat. HiveMind OS remembers facts, decisions, and preferences. Try: `/remember I prefer TypeScript over JavaScript`" |
| **Tools** | Pre-populated with built-in tools (filesystem, shell, web, etc.). "Connect external tools via MCP servers → [Add MCP server]" |
| **Loop Designer** | Opens with the template browser showing. "Pick a template to start, or drag stages from the palette to build from scratch." |
| **Peers** | "Connect to HiveMind OS on another machine to share knowledge and agents → [Invite a peer]" |

#### Quick-Start Suggestions

After the first message exchange, the chat subtly shows **contextual feature discovery tips** (dismissable, never repeated):

```
💡 Tip: HiveMind OS remembers things automatically, but you can
   explicitly save something with /remember. Try it!
```

Tips are shown at most one per session, drawn from a pool:
- `/remember` for explicit memory
- `/task` for scheduling
- Custom roles for specialised agents
- The command palette (`Ctrl+K`)
- Loop designer for custom workflows
- MCP servers for tool integration

#### Guided Tours (Optional)

A "Take a tour" link on the welcome screen (and always accessible from Help → Tour) walks through the primary views with tooltip overlays. The tour is:
- Skippable at any point
- Completable in under 2 minutes
- Non-blocking — the user can start chatting and return to the tour later

#### Configuration Migration

If HiveMind OS detects existing config from known tools, it offers to import:

| Source | What's imported |
|---|---|
| `~/.openai` / env vars | API keys |
| VS Code / Cursor settings | GitHub Copilot token, model preferences |
| `~/.config/claude/` | Anthropic API key |
| Existing MCP config (`mcp.json`, `claude_desktop_config.json`) | MCP server definitions |

Import is **opt-in** — shown as a toast: "Found existing OpenAI API key. Use it? [Yes] [No]"

### 13.5 Messaging Bridges

HiveMind OS supports external messaging platforms as first-class interaction channels, enabling mobile and remote access to the agent without the desktop UI.

#### Supported Platforms

| Platform | Transport | Auth Model |
|---|---|---|
| **Discord** | Bot via gateway websocket | Bot token + pairing code per user |
| **Slack** | Bot via Socket Mode | App token + pairing code per user |
| **Telegram** | Bot API (long-polling) | Bot token + pairing code per user |

#### Architecture

Each bridge runs as an async Tokio task inside the daemon — no external process required. Bridges are **equal clients** of the daemon's internal API, just like the Tauri UI or CLI. They:

1. **Receive** inbound messages from users on the platform.
2. **Parse** natural-language intent via the admin model (§4.4).
3. **Route** to the appropriate agent instance or role.
4. **Stream** responses back to the user in the platform thread/channel.
5. **Enforce classification** — every outbound message passes through the classification gate. Messaging platforms are `public` channels by default.

#### User Linking

Users must link their messaging identity to a HiveMind OS identity before use:

1. User sends `/link` command to the HiveMind OS bot on Discord/Slack/Telegram.
2. HiveMind OS generates a one-time 6-digit pairing code.
3. The bot DMs the code to the user.
4. User enters the code in the HiveMind OS UI or CLI (`hive bridge verify <code>`).
5. The link is stored in the daemon's config — all future messages from that platform identity are authenticated.

#### Configuration

```yaml
messaging_bridges:
  discord:
    enabled: true
    bot_token: env:DISCORD_BOT_TOKEN
    channel_class: public
    allowed_guilds: [123456789]          # Restrict to specific Discord servers
  
  slack:
    enabled: false
    app_token: env:SLACK_APP_TOKEN
    bot_token: env:SLACK_BOT_TOKEN
    channel_class: public
    allowed_workspaces: [T01234567]
  
  telegram:
    enabled: false
    bot_token: env:TELEGRAM_BOT_TOKEN
    allowed_users: [123456789]           # Restrict to specific Telegram user IDs
    channel_class: public
```

#### Approval Prompts

When the classification gate triggers a `prompt` action for outbound data on a messaging bridge, the bridge renders an interactive approval UI native to the platform (Discord buttons, Slack interactive messages, Telegram inline keyboard). The user can **Allow**, **Deny**, or **Redact & Send** directly from their phone.

### 13.6 Chat Interface: Interaction Model

The chat view is HiveMind OS's primary user-facing surface. It must feel responsive even when the agent is mid-execution, and always communicate what the agent is doing and why.

#### 13.6.1 Command Queuing

Users can type and submit messages **at any time**, even while the agent is actively working. Messages are enqueued and processed in FIFO order after the current turn completes.

| Behaviour | Detail |
|---|---|
| **Queue indicator** | A subtle badge shows the number of queued messages (e.g., "2 queued"). |
| **Queue visibility** | Queued messages appear in the chat thread with a "pending" style (muted, right-aligned, clock icon). |
| **Reorder / cancel** | Right-click a queued message to cancel it or drag to reorder. |
| **Immediate delivery** | If the agent is idle, the message is delivered immediately (no queue delay). |
| **Queue-aware context** | The agent sees the full queue, so it can batch related requests if the loop supports it. |

```
┌─ Chat ────────────────────────────────────────────────┐
│  Agent: [working…]  "Analysing repository structure…" │
│  ─────────────────────────────────────────────────     │
│  You: Refactor the auth module               [active] │
│  You: Also update the tests              ⏳ [queued]  │
│  You: And bump the version               ⏳ [queued]  │
│                                                        │
│  [────────────────────────────── Send ─────]           │
└────────────────────────────────────────────────────────┘
```

#### 13.6.2 Interruption

The user can interrupt the agent mid-turn to redirect, cancel, or inject new instructions.

| Action | Trigger | Behaviour |
|---|---|---|
| **Soft interrupt** | `Esc` key or "Pause" button | Agent finishes current tool call, then pauses. Emits `paused` event. User can review partial work, then resume or redirect. |
| **Hard interrupt** | `Ctrl+C` / `Cmd+.` or "Stop" button | Agent aborts immediately. In-flight model calls are cancelled. Partial results are preserved in the event log. Emits `interrupted` event. |
| **Redirect** | Type while agent is working + hit `Enter` with `⌘` / `Ctrl` modifier | Current task is cancelled (hard interrupt), new message jumps the queue and becomes the active turn. |

After any interruption:
- The conversation history shows the interrupted turn with a clear marker (⚠ interrupted).
- Event-sourced state is consistent — interrupted tool calls are recorded with `status: interrupted`.
- The agent does **not** lose work that was already committed (KG writes, file saves, etc.).

#### 13.6.3 Thinking & Reasoning Display

The agent's internal reasoning is surfaced in real time, keeping the user informed without overwhelming them.

| Element | What It Shows | UI Treatment |
|---|---|---|
| **Thinking** | Raw model reasoning / chain-of-thought tokens | Collapsible "thinking" block below the message, dimmed text, monospace. Collapsed by default; click to expand. Updates live during streaming. |
| **Intent** | A short (≤8 word) summary of what the agent is currently doing | Persistent status line at top of chat: "🔨 Refactoring auth module". Updates whenever the agent enters a new stage or changes strategy. Always visible. |
| **Stage indicator** | Which loop stage is active (e.g., `plan → act → observe`) | Shown as a horizontal progress indicator or breadcrumb under the intent line. Highlights the active stage. |
| **Tool activity** | Tool calls in progress | Inline card: tool name, arguments (truncated), spinner while running. Expands to show result on completion. |
| **Progress** | For long operations (multi-file edit, research) | Optional progress bar or step counter emitted by the loop: "Analysing file 3/12". |

```
┌─ Chat ────────────────────────────────────────────────┐
│  🔨 Intent: Refactoring auth module                   │
│  ┄ plan ▸ act ▸ observe                               │
│                                                        │
│  💭 Thinking (click to expand)                         │
│  ┆ The auth module has 3 files. I'll start with...    │
│                                                        │
│  🔧 read_file("src/auth/login.rs")  ✓ 142 lines      │
│  🔧 edit_file("src/auth/login.rs")  ⏳ writing...     │
│                                                        │
│  Progress: ██████░░░░ 2/5 files                        │
│                                                        │
│  [─────────── Esc to pause · Ctrl+C to stop ────]     │
└────────────────────────────────────────────────────────┘
```

#### 13.6.4 Configuration

```yaml
chat:
  show_thinking: true        # Show model reasoning (can be toggled per session)
  thinking_collapsed: true   # Collapse thinking blocks by default
  show_intent: true          # Show intent status line
  show_stage: true           # Show loop stage breadcrumb
  show_tool_calls: true      # Show tool call cards
  queue_enabled: true        # Allow command queuing
  interrupt_confirmation: false  # If true, hard interrupt requires a confirmation click
```

---

## 14. Configuration & Extensibility

### 14.1 Config File

All configuration lives in `~/.hivemind/config.yaml` (and optionally project-level `.hivemind/config.yaml`):

```yaml
# ~/.hivemind/config.yaml
version: 1

providers:   [...]   # §4.1
mcp_servers: [...]   # §6.2
agent_loop:  {...}   # §9.3

security:
  default_data_class: INTERNAL
  auto_classify: true
  audit_log: ~/.hivemind/audit.log
  encryption_key_source: os-keychain

scheduler:
  max_concurrent_tasks: 5
  default_timeout: 5m

knowledge_graph:
  storage: ~/.hivemind/knowledge.db
  vector_model: local:all-MiniLM-L6-v2
  auto_consolidate: true
  decay_interval: 7d
  backup_interval: 24h

ui:
  theme: system
  global_hotkey: ctrl+space
  notifications: true
```

### 14.2 Plugin System

HiveMind OS supports plugins for:

- **Loop strategies** — custom agentic loop implementations.
- **Middleware** — custom pre/post processing for model and tool calls.
- **Tools** — additional built-in tools (beyond MCP).
- **Classifiers** — custom data-classification logic.
- **UI panels** — custom views in the app (via webview extensions).

Plugins are distributed as npm packages or local directories with a `hive-plugin.json` manifest.

---

## 15. Glossary

| Term | Definition |
|---|---|
| **Channel** | Any endpoint that data flows to: model provider, MCP server, export target, messaging bridge. |
| **Channel Class** | The maximum data classification level a channel is permitted to receive. |
| **Data Class** | The sensitivity label on a piece of data (PUBLIC → RESTRICTED). |
| **Effective Class** | A node's data class considering inherited classifications from the graph. |
| **Classification Gate** | Middleware that blocks data from flowing to under-classified channels. |
| **Memory Manager** | The subsystem that reads/writes the knowledge graph. |
| **Loop Strategy** | A pluggable algorithm that implements the agent's reasoning cycle. |
| **Session** | A single conversation between a user and the agent, consisting of turns. Also referred to as a "workflow run" when executed through the workflow engine (§9.7). |
| **Workflow Run** | The durable execution of a loop strategy, tracked via event sourcing. A session creates a workflow run. |
| **Middleware** | A hook that intercepts and can modify data at each stage of the agentic loop. |
| **Messaging Bridge** | A daemon task that connects HiveMind OS to an external messaging platform (Discord, Slack, Telegram) as an interaction channel (§13.5). |
| **Peer / Peering** | A trusted connection between two HiveMind OS instances, enabling cross-machine agent communication, knowledge sync, and resource sharing. |
| **PeerID** | A cryptographic identity for a HiveMind OS instance, derived from its long-lived key pair. |
| **Role** | A reusable agent persona definition (system prompt, tools, model prefs, security posture). |
| **Agent Instance** | A live, running incarnation of a role with its own conversation state and inbox. |
| **Delegation** | When one agent hands a task to another role, creating a supervised child agent. |
| **Agent Skill** | A portable package of instructions, scripts, and reference material that an agent activates on demand (per the [Agent Skills](https://agentskills.io) open standard). |
| **Blackboard** | A shared knowledge-graph namespace that multiple agents can read/write for collaboration. |
| **Context Compaction** | The process of extracting knowledge from old conversation turns, storing it in the KG, and pruning those turns from the active context (§9.12). |
| **Session Fork** | A copy-on-write branch of an existing session at a given event, creating a new independent session that shares history (§9.13). |

---

## 16. Future Considerations

- **Voice interface** — local speech-to-text and text-to-speech for hands-free operation.
- **Mobile companion** — a lightweight mobile client that syncs with the desktop agent (classification rules travel with the data).
- **Formal verification** — prove that the classification gate correctly prevents data leakage for a given policy configuration.
- **Agent marketplace** — community-authored roles published and shared via the same Sigstore-signed registry as loops.
- **Peer mesh networking** — automatic peer discovery and trust propagation across teams, enabling multi-hop agent routing.
