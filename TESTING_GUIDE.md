# HiveMind OS — Build, Test & Iterate Guide

This is the definitive guide for building, testing, and iterating on HiveMind OS. It covers the full development loop: tooling setup, building the daemon and UI, running tests at every layer, and verifying UI changes visually — all in a way that works for both human developers and AI agents.

---

## Table of Contents

1. [Prerequisites & Tooling](#1-prerequisites--tooling)
2. [Project Structure Quick Reference](#2-project-structure-quick-reference)
3. [Build Loop](#3-build-loop)
4. [Test Loop](#4-test-loop)
5. [Dev Mode (Hot-Reload)](#5-dev-mode-hot-reload)
6. [UI Verification via Playwright + CDP](#6-ui-verification-via-playwright--cdp)
7. [AI Agent Workflow (MCP-Based)](#7-ai-agent-workflow-mcp-based)
8. [The Build-Test-Iterate Cycle](#8-the-build-test-iterate-cycle)
9. [Debugging](#9-debugging)
10. [CI/CD Integration](#10-cicd-integration)
11. [Adversarial E2E Scenarios (200+ Suite)](#11-adversarial-e2e-scenarios-200-scenario-suite)
12. [Key Gotchas](#12-key-gotchas)

---

## 1. Prerequisites & Tooling

### Required

| Tool | Version | Install | Purpose |
|------|---------|---------|---------|
| **Rust** | stable (latest) | [rustup.rs](https://rustup.rs) | Daemon, all `hive-*` crates |
| **Node.js** | 20 LTS+ | [nodejs.org](https://nodejs.org) | Frontend build, Playwright, scripts |
| **pnpm** | 9+ | `npm install -g pnpm` | Frontend package manager (faster, stricter than npm) |
| **Tauri CLI** | v2 | `cargo install tauri-cli --version "^2"` | Build & dev for Tauri app |
| **Playwright** | latest | `pnpm add -D @playwright/test` then `npx playwright install chromium` | E2E / UI verification |

### Platform-Specific

| Platform | Additional Requirements |
|----------|------------------------|
| **Windows** | WebView2 (bundled with Windows 10+), Visual Studio Build Tools (C++ workload for native deps) |
| **macOS** | Xcode Command Line Tools (`xcode-select --install`), WebKit (bundled) |

### Recommended

| Tool | Install | Purpose |
|------|---------|---------|
| `cargo-watch` | `cargo install cargo-watch` | Auto-rebuild Rust on file changes |
| `cargo-nextest` | `cargo install cargo-nextest` | Faster test runner with better output |
| `cargo-llvm-cov` | `cargo install cargo-llvm-cov` | Code coverage reporting |
| `clippy` | Bundled with rustup | Lint Rust code |
| `rustfmt` | Bundled with rustup | Format Rust code |
| `sqlx-cli` | `cargo install sqlx-cli` | SQLite schema management (if using sqlx) |

### Verify Setup

```powershell
# Run all checks at once
rustc --version && cargo --version && node --version && pnpm --version && cargo tauri --version
```

Expected output: versions for each tool, no errors.

---

## 2. Project Structure Quick Reference

```
hivemind-os/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── hive-daemon/             # Main daemon binary (cargo run -p hive-daemon)
│   ├── hive-cli/                # CLI binary (cargo run -p hive-cli)
│   ├── hive-core/               # Shared types, traits, config, event bus
│   ├── hive-api/                # Local API server (HTTP/WS + socket)
│   ├── hive-classification/     # Data classification engine
│   ├── hive-providers/          # Model provider adapters
│   ├── hive-mcp/                # MCP client implementation
│   ├── hive-knowledge/          # Knowledge graph engine
│   ├── hive-embedded-models/    # In-process model inference
│   ├── hive-loop/               # Agentic loop engine + DSL runtime
│   ├── hive-workflow/           # Workflow engine + state persistence
│   ├── hive-scheduler/          # Background task scheduler
│   ├── hive-agents/             # Roles, instances, inter-agent comms
│   ├── hive-peering/            # Peer identity, transport, sync
│   ├── hive-messaging/          # External messaging bridges
│   ├── hive-skills/             # Agent Skills loader
│   ├── hive-tools/              # Built-in tool implementations
│   └── hive-crypto/             # Encryption, signing, keychain
├── tauri-app/
│   ├── src-tauri/                # Tauri Rust glue (thin — connects to daemon)
│   │   ├── src/main.rs
│   │   └── tauri.conf.json
│   ├── src/                      # Frontend (TypeScript + chosen framework)
│   ├── package.json
│   └── vite.config.ts
├── e2e/                          # End-to-end Playwright tests
│   ├── package.json
│   ├── playwright.config.ts
│   ├── helpers/
│   │   ├── launch.ts             # App launcher + CDP connector
│   │   └── fixtures.ts           # Shared test fixtures
│   └── tests/
│       ├── smoke.spec.ts
│       ├── conversation.spec.ts
│       ├── classification.spec.ts
│       └── ...
└── docs/
```

---

## 3. Build Loop

### Build Everything

```powershell
# Full workspace build (all Rust crates)
cargo build --workspace

# Release build (optimised, for E2E testing)
cargo build --workspace --release

# Frontend only (from tauri-app/)
cd tauri-app && pnpm install && pnpm build

# Tauri app (builds Rust backend + frontend + bundles)
cd tauri-app && cargo tauri build
```

### Build Individual Crates

```powershell
# Build a specific crate (fast iteration)
cargo build -p hive-classification
cargo build -p hive-knowledge
cargo build -p hive-loop

# Check without producing binaries (fastest feedback)
cargo check -p hive-classification
```

### Lint

```powershell
# Rust linting (all crates)
cargo clippy --workspace --all-targets -- -D warnings

# Rust formatting check
cargo fmt --all -- --check

# Frontend linting (from tauri-app/)
cd tauri-app && pnpm lint
```

---

## 4. Test Loop

### LLM Test Strategy — Mock vs. Real Models

Most tests run **without any real LLM**. The test harness provides deterministic mock providers that return canned or scripted responses. Real LLM access is only needed for nightly/release validation.

#### Test Provider Hierarchy

| Provider | When Used | Requires API Key | Deterministic |
|---|---|---|---|
| **`MockProvider`** | Unit tests, integration tests, `mock` E2E mode | No | Yes — returns scripted responses keyed by input pattern |
| **`RecordedProvider`** | Regression tests | No — replays saved responses | Yes — replays exact recorded API responses from `.recording.json` fixtures |
| **`EmbeddedProvider`** (tiny model) | CI smoke tests needing real inference | No — uses bundled ~100MB model | Mostly — small model, low variance |
| **Real providers** (OpenAI, etc.) | `live` E2E mode, nightly CI | Yes — CI secrets or local keys | No — model outputs vary |

#### MockProvider (`hive-test-utils`)

A configurable mock that plugs into the model router like any real provider:

```rust
let mock = MockProvider::new()
    // Pattern-matched responses
    .on_contains("classify this", json!({ "classification": "CONFIDENTIAL" }))
    .on_contains("summarise", "This is a summary of the document.")
    // Default fallback
    .default_response("I'm a mock assistant. How can I help?")
    // Simulate streaming (token-by-token with delays)
    .with_streaming(true, Duration::from_millis(10))
    // Simulate failures
    .fail_after(3)                          // 4th call returns error
    .with_latency(Duration::from_millis(50)) // Artificial delay
    // Capture calls for assertions
    .capture_calls();

// After test:
assert_eq!(mock.call_count(), 5);
assert!(mock.calls()[0].prompt.contains("classify"));
```

#### RecordedProvider (Record/Replay)

For tests that need realistic model behaviour without live API calls:

```powershell
# Record a session against real providers (one-time, by a developer with keys)
HIVEMIND_TEST_MODE=record cargo test -p hive-loop -- test_research_loop

# Recordings saved to: tests/fixtures/recordings/test_research_loop.recording.json
# These are committed to the repo — anyone can replay without API keys

# Replay in CI (no keys needed)
HIVEMIND_TEST_MODE=replay cargo test -p hive-loop -- test_research_loop
```

Recording files contain request/response pairs with content hashes for matching. Sensitive data (API keys, timestamps) is automatically stripped.

#### What Needs a Real LLM?

Only these test categories require real model access:

| Test | Why | Frequency |
|---|---|---|
| `live` E2E scenarios | Validate end-to-end with real model behaviour | Nightly CI |
| Prompt injection scanner accuracy | Real model classification performance | Weekly CI |
| Embedded model integration | Verify actual inference works | Per-release |
| New provider onboarding | Validate API compatibility | Ad-hoc |

#### CI Configuration

```yaml
# .github/workflows/test.yaml (excerpt)
jobs:
  test-mock:        # Every PR — no secrets needed
    env:
      HIVEMIND_TEST_MODE: mock

  test-nightly:     # Nightly — uses org secrets
    env:
      HIVEMIND_TEST_MODE: live
      OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}
      # ... other provider keys
```

**Rule:** A developer with no API keys should be able to run `cargo test --workspace` and `pnpm test:scenarios` (mock mode) and have everything pass. Real keys are never required for PR checks.

### Layer 1 — Rust Unit Tests

Fast, isolated, no external dependencies. Run after every code change.

```powershell
# All workspace tests
cargo test --workspace

# Specific crate
cargo test -p hive-classification
cargo test -p hive-knowledge

# Specific test by name
cargo test -p hive-classification -- test_gate_blocks_restricted

# With cargo-nextest (better output, parallel, retries)
cargo nextest run --workspace
cargo nextest run -p hive-classification

# With coverage
cargo llvm-cov --workspace --html    # generates target/llvm-cov/html/index.html
```

**What to test at this layer:**
- Classification logic (labelling, gate decisions, propagation)
- Knowledge graph CRUD, queries, classification enforcement
- Config parsing and validation
- Event bus pub/sub
- Model router selection logic (with mock providers)
- DSL parser (`.loop.yaml` → AST)
- Workflow state machine transitions

### Layer 2 — Frontend Unit/Component Tests

Run with Vitest (or the chosen test runner).

```powershell
cd tauri-app
pnpm test              # Run all frontend tests
pnpm test -- --watch   # Watch mode for iteration
```

**What to test at this layer:**
- Component rendering (with mock Tauri `invoke` responses)
- State management logic
- Classification badge display
- Form validation

### Layer 3 — Integration Tests

Rust integration tests that spin up subsystems together.

```powershell
# Integration tests (tests/ directory in each crate, or workspace-level)
cargo test --workspace --test '*'

# Example: test daemon API endpoints
cargo test -p hive-api --test api_integration
```

**What to test at this layer:**
- Daemon starts and responds to API calls
- Classification gate + model router work together
- MCP client connects to a test MCP server
- Workflow engine runs a simple loop end-to-end
- Knowledge graph + embedded model embeddings

### Layer 4 — End-to-End (Playwright + CDP)

Full app running, tested through the real UI. See [§6](#6-ui-verification-via-playwright--cdp) for detailed setup.

```powershell
cd e2e
pnpm test                            # Run all E2E tests
npx playwright test smoke.spec.ts    # Single test file
npx playwright test --headed         # Watch the browser (debugging)
npx playwright test --debug          # Step through with Playwright Inspector
```

**What to test at this layer:**
- User can send a message and see a streamed response
- Classification badge appears on messages
- Tool call results render correctly
- Override prompt modal appears and works
- Agent dashboard shows running agents
- Settings save and reload correctly

### Layer 5 — True E2E via CDP (Windows Only)

Tests that launch the **real built HiveMind OS Tauri binary** and connect to its WebView2 via Chrome DevTools Protocol. Unlike Layer 4, these tests exercise the actual Tauri IPC layer — no mocks, no bridges, no API shims.

**Platform support:** Windows only. WebView2 is Chromium-based and exposes CDP natively. macOS uses WKWebView which does not support CDP, and `tauri-driver` also does not support macOS.

**Prerequisites:**
- Built Tauri binary (`cargo tauri build --debug --no-bundle` or set `HIVEMIND_BINARY_PATH`)
- `test_daemon` binary (`cargo build --bin test_daemon -p hive-test-utils`)

```powershell
# From apps/hivemind-desktop/
npm run test:e2e:cdp                 # Run all CDP tests
npm run test:e2e:cdp:headed          # Watch the app (debugging)

# Or with Playwright directly
npx playwright test --config playwright.cdp.config.ts
npx playwright test --config playwright.cdp.config.ts --debug
```

**What to test at this layer:**
- Full IPC round-trip: JS `invoke()` → Tauri command handler → Rust HTTP → daemon → response → JS
- Parameter serialization correctness (snake_case keys accepted by Rust)
- Response deserialization correctness (JSON parsed correctly in JS)
- No console errors during full app navigation
- Native features (to the extent feasible in automated testing)

**How it works:**
1. Global setup starts `test_daemon` with scripted LLM responses
2. Launches `HiveMind OS.exe` with `HIVEMIND_DAEMON_URL` pointing at test daemon and `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9515`
3. Playwright connects via `chromium.connectOverCDP()`
4. Tests interact with the real app through the real WebView2

**Cross-platform coverage strategy:**
| Platform | Real Binary Tests | IPC Contract (Rust) | UI + Backend (Integration) |
|----------|-------------------|---------------------|---------------------------|
| Windows  | ✅ Layer 5 (CDP)  | ✅ `cargo test`     | ✅ Layer 4 (API bridge)    |
| macOS    | ❌ Not supported  | ✅ `cargo test`     | ✅ Layer 4 (API bridge)    |

The Rust IPC contract tests (`src-tauri/tests/tauri_ipc_snake_case.rs` and `tauri_ipc_contract.rs`) provide cross-platform coverage for IPC correctness, while CDP tests provide full-chain validation on Windows.

---

## 5. Dev Mode (Hot-Reload)

### Frontend Hot-Reload (Tauri Dev)

```powershell
cd tauri-app
cargo tauri dev
```

This starts:
1. Vite dev server with HMR (frontend changes reflect instantly)
2. Tauri app window connected to the dev server
3. Rust backend recompiles on changes (slower — Rust compile times)

### Backend Watch Mode

For faster Rust iteration without the Tauri UI:

```powershell
# Watch and rebuild daemon on changes
cargo watch -x "run -p hive-daemon"

# Watch and run tests on changes
cargo watch -x "test -p hive-classification"

# Watch specific crate, run specific test
cargo watch -w crates/hive-knowledge -x "test -p hive-knowledge -- query"
```

### Daemon + UI Separate (Recommended for Most Work)

During development, run the daemon and UI separately:

```powershell
# Terminal 1: Run the daemon
cargo run -p hive-daemon

# Terminal 2: Run the Tauri UI in dev mode (connects to running daemon)
cd tauri-app && cargo tauri dev

# Terminal 3: Or use the CLI to interact with the daemon
cargo run -p hive-cli -- daemon status
cargo run -p hive-cli -- chat "Hello, HiveMind OS"
```

---

## 6. UI Verification via Playwright + CDP

### How It Works

Tauri apps use WebView2 (Windows) or WebKitGTK (macOS/Linux) to render a web frontend that communicates with a Rust backend via `invoke()` commands. Playwright connects to the app over the Chrome DevTools Protocol (CDP).

```
┌───────────┐      CDP (port 9222)      ┌─────────────────────┐
│ Playwright │◄────────────────────────►│ Tauri App           │
│ (Test/Agent)│   connectOverCDP()      │  WebView2 + Rust    │
└───────────┘                           └─────────────────────┘
```

Every interaction goes through the real WebView, hitting the real Rust backend. No mocks.

### Step 1 — Launch the Tauri App with CDP Enabled

```powershell
# Windows (PowerShell)
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
Start-Process -FilePath "tauri-app\src-tauri\target\release\hivemind.exe"

# macOS
WEBKIT_INSPECTOR_SERVER=127.0.0.1:9222 ./tauri-app/src-tauri/target/release/hivemind
```

### Step 2 — Wait for the CDP Port

```typescript
// e2e/helpers/launch.ts
import net from "net";
import { execSync, spawn, ChildProcess } from "child_process";

export function waitForPort(port: number, host = "127.0.0.1", timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    (function tryConnect() {
      if (Date.now() - start > timeoutMs) return reject(new Error(`Timeout waiting for port ${port}`));
      const sock = net.createConnection({ port, host }, () => { sock.destroy(); resolve(); });
      sock.on("error", () => setTimeout(tryConnect, 300));
    })();
  });
}

export async function launchApp(): Promise<ChildProcess> {
  const appPath = process.platform === "win32"
    ? "tauri-app\\src-tauri\\target\\release\\hivemind.exe"
    : "tauri-app/src-tauri/target/release/hivemind";

  const env = { ...process.env };
  if (process.platform === "win32") {
    env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222";
  } else {
    env.WEBKIT_INSPECTOR_SERVER = "127.0.0.1:9222";
  }

  const child = spawn(appPath, [], { env, stdio: "pipe" });
  await waitForPort(9222);
  return child;
}
```

### Step 3 — Connect Playwright via CDP

```typescript
import { chromium, Browser, Page } from "playwright";

const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
const context = browser.contexts()[0];   // Tauri creates one context
const page    = context.pages()[0];      // with one page (the app window)
await page.waitForLoadState("domcontentloaded");
```

### Step 4 — Interact and Assert

```typescript
// Type into the chat input and send a message
await page.locator('[data-testid="chat-input"]').fill("Hello HiveMind OS");
await page.locator('[data-testid="send-button"]').click();

// Wait for and assert the response
await expect(page.locator('[data-testid="message-assistant"]').last())
  .toContainText("Hello", { timeout: 30_000 });

// Verify classification badge
await expect(page.locator('[data-testid="classification-badge"]').last())
  .toHaveAttribute("data-level", "PUBLIC");

// Take a screenshot for visual verification
await page.screenshot({ path: "screenshots/chat-response.png", fullPage: true });
```

### Step 5 — Tear Down

```typescript
await browser.close();        // Disconnect Playwright
appProcess.kill();             // Kill the Tauri process
```

### Reusable Playwright Config

```typescript
// e2e/playwright.config.ts
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 60_000,
  retries: 1,
  reporter: [["html", { open: "never" }], ["list"]],
  use: {
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  // No webServer config — we launch the Tauri app manually via globalSetup
  globalSetup: "./helpers/global-setup.ts",
  globalTeardown: "./helpers/global-teardown.ts",
});
```

```typescript
// e2e/helpers/global-setup.ts
import { launchApp } from "./launch";

let appProcess: any;

export default async function globalSetup() {
  appProcess = await launchApp();
  // Store for teardown
  (globalThis as any).__HIVEMIND_APP__ = appProcess;
}
```

```typescript
// e2e/helpers/global-teardown.ts
export default async function globalTeardown() {
  const app = (globalThis as any).__HIVEMIND_APP__;
  if (app) app.kill();
}
```

---

## 7. AI Agent Workflow (MCP-Based)

When an AI agent (e.g., Copilot) is building HiveMind OS and needs to verify UI changes, the Playwright MCP tools (`browser_snapshot`, `browser_click`, etc.) cannot call `connectOverCDP` directly. Here are the workflows:

### Workflow A — Verify via Playwright MCP Tools (Preferred for Quick Checks)

For rapid visual verification during development:

1. **Build the app in release mode:**
   ```powershell
   cd tauri-app && cargo tauri build
   ```

2. **Launch with CDP enabled (async, detached):**
   ```powershell
   $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
   Start-Process "tauri-app\src-tauri\target\release\hivemind.exe"
   ```

3. **Wait for CDP, then navigate the Playwright MCP browser to the CDP debug URL:**
   ```powershell
   # Get the WebSocket debug URL
   $response = Invoke-RestMethod "http://127.0.0.1:9222/json/version"
   $response.webSocketDebuggerUrl
   ```

4. **Use the Playwright MCP navigate tool** to go to `http://127.0.0.1:9222` — this won't give you direct app access, but you can use the debug URL to connect.

5. **Alternative — use `browser_navigate`** to `http://localhost:1420` (Vite dev server port) if running in dev mode. This gives you the same frontend served in a regular browser tab, which Playwright MCP tools can interact with directly.

### Workflow B — Verify via Node.js E2E Scripts (Preferred for Assertions)

For structured verification with assertions:

1. **Write a targeted test script:**
   ```typescript
   // e2e/tests/verify-feature.spec.ts
   import { test, expect } from "@playwright/test";
   import { chromium } from "playwright";

   test("classification badge shows on messages", async () => {
     const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
     const page = browser.contexts()[0].pages()[0];

     await page.locator('[data-testid="chat-input"]').fill("test message");
     await page.locator('[data-testid="send-button"]').click();

     const badge = page.locator('[data-testid="classification-badge"]').last();
     await expect(badge).toBeVisible();
     await expect(badge).toHaveAttribute("data-level", /PUBLIC|INTERNAL/);

     await page.screenshot({ path: "screenshots/classification-badge.png" });
     await browser.close();
   });
   ```

2. **Run it:**
   ```powershell
   cd e2e && npx playwright test verify-feature.spec.ts
   ```

3. **Check results** — Playwright HTML report at `e2e/playwright-report/index.html`, screenshots in `e2e/screenshots/`.

### Workflow C — Verify Frontend in Browser (Fastest for UI-Only Changes)

When changes are frontend-only (no Rust changes), skip building and use the Vite dev server directly:

1. **Start the Vite dev server:**
   ```powershell
   cd tauri-app && pnpm dev
   ```
   This starts on `http://localhost:1420` (default Vite port).

2. **Use the Playwright MCP `browser_navigate` tool** to go to `http://localhost:1420`.

3. **Use `browser_snapshot`** to see the accessibility tree and **`browser_click`** / **`browser_type`** to interact.

4. **Use `browser_take_screenshot`** to capture the current state.

> **Note:** In this mode, Tauri `invoke()` calls will fail because there's no Rust backend. Mock the Tauri API in dev mode:
> ```typescript
> // src/lib/tauri-mock.ts (loaded only when window.__TAURI__ is undefined)
> if (!window.__TAURI__) {
>   window.__TAURI__ = {
>     core: {
>       invoke: async (cmd: string, args?: any) => {
>         // Return mock data based on command
>         const mocks: Record<string, any> = {
>           "get_config": { /* mock config */ },
>           "send_message": { id: "1", role: "assistant", content: "Mock response" },
>           // ... add mocks as needed
>         };
>         return mocks[cmd] ?? null;
>       }
>     }
>   };
> }
> ```

### Workflow D — CLI-Based Verification (Backend-Only Changes)

For backend/daemon changes that don't require UI:

```powershell
# Build and run daemon
cargo run -p hive-daemon &

# Test via CLI
cargo run -p hive-cli -- daemon status
cargo run -p hive-cli -- chat "Hello"
cargo run -p hive-cli -- config show
cargo run -p hive-cli -- task list

# Or hit the API directly
Invoke-RestMethod -Uri "http://localhost:8420/api/status"
Invoke-RestMethod -Uri "http://localhost:8420/api/config" -Method GET
```

---

## 8. The Build-Test-Iterate Cycle

### For a Rust Backend Change

```
 ┌──────────────────────────────────────────────────────────────────────┐
 │  1. Edit Rust code                                                   │
 │  2. cargo check -p <crate>           # Fast syntax/type check        │
 │  3. cargo clippy -p <crate>          # Lint check                    │
 │  4. cargo test -p <crate>            # Unit tests                    │
 │  5. cargo test --workspace           # Cross-crate integration       │
 │  6. cargo run -p hive-daemon        # Manual smoke test via CLI     │
 │  7. (If UI-visible) cargo tauri build && run E2E                     │
 └──────────────────────────────────────────────────────────────────────┘
```

**Shortcut — cargo watch for steps 2-4:**
```powershell
cargo watch -w crates/hive-classification -s "cargo check -p hive-classification && cargo test -p hive-classification"
```

### For a Frontend Change

```
 ┌──────────────────────────────────────────────────────────────────────┐
 │  1. Edit frontend code (TypeScript/TSX)                              │
 │  2. pnpm lint                        # ESLint check                  │
 │  3. pnpm test                        # Component/unit tests          │
 │  4. pnpm dev → check in browser      # Visual check at localhost     │
 │  5. cargo tauri dev                  # Full app with hot-reload      │
 │  6. (If critical) Run E2E tests                                      │
 └──────────────────────────────────────────────────────────────────────┘
```

### For a Cross-Cutting Change (Rust + Frontend)

```
 ┌──────────────────────────────────────────────────────────────────────┐
 │  1. Edit Rust code (new/changed Tauri command)                       │
 │  2. Edit frontend code (call the command)                            │
 │  3. cargo check --workspace          # Rust compiles                 │
 │  4. cargo test --workspace           # Rust tests pass               │
 │  5. cd tauri-app && pnpm test        # Frontend tests pass           │
 │  6. cargo tauri dev                  # Manual verification           │
 │  7. Run relevant E2E tests                                           │
 └──────────────────────────────────────────────────────────────────────┘
```

### Quick Reference — Which Command When

| I just changed... | Run this | Time |
|---|---|---|
| A single Rust file | `cargo check -p <crate>` | ~2-5s |
| Rust logic with tests | `cargo test -p <crate>` | ~5-15s |
| Multiple crates | `cargo test --workspace` | ~30-60s |
| Frontend component | `pnpm test` (in tauri-app/) | ~3-10s |
| Tauri command + frontend | `cargo tauri dev` (visual check) | ~30-60s first, then hot-reload |
| Anything user-facing | E2E test for that feature | ~30-60s |
| Everything (pre-commit) | `cargo clippy --workspace && cargo test --workspace && cd tauri-app && pnpm lint && pnpm test` | ~2-5 min |

---

## 9. Debugging

### Rust Backend

```powershell
# Run with debug logging
$env:RUST_LOG = "hive_daemon=debug,hive_classification=trace"
cargo run -p hive-daemon

# Run a specific test with output
cargo test -p hive-knowledge -- test_query_classification --nocapture

# Backtrace on panic
$env:RUST_BACKTRACE = "1"
cargo run -p hive-daemon
```

### Frontend

```powershell
# Tauri dev mode opens DevTools automatically
cargo tauri dev

# Or open DevTools manually in the running app:
# Right-click → Inspect (if enabled in tauri.conf.json)
```

In `tauri.conf.json`, ensure DevTools are enabled in dev:
```json
{
  "app": {
    "windows": [{ "devtools": true }]
  }
}
```

### Playwright Tests

```powershell
# Debug mode — opens Playwright Inspector, step through actions
cd e2e && npx playwright test --debug

# Headed mode — see the browser
npx playwright test --headed

# Trace viewer — after a failed test with traces enabled
npx playwright show-trace e2e/test-results/*/trace.zip

# Generate tests interactively
npx playwright codegen http://localhost:1420
```

### API Debugging

```powershell
# Check daemon is running and responsive
Invoke-RestMethod "http://localhost:8420/api/status"

# WebSocket connection test
# Use wscat: npm install -g wscat
wscat -c "ws://localhost:8420/ws"
```

---

## 10. CI/CD Integration

### GitHub Actions Workflow

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

jobs:
  rust:
    strategy:
      matrix:
        os: [macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2

      - name: Format check
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --workspace --all-targets -- -D warnings

      - name: Tests
        run: cargo test --workspace

      - name: Build release
        run: cargo build --workspace --release

  frontend:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: tauri-app
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: pnpm
          cache-dependency-path: tauri-app/pnpm-lock.yaml

      - run: pnpm install
      - run: pnpm lint
      - run: pnpm test
      - run: pnpm build

  e2e:
    needs: [rust, frontend]
    strategy:
      matrix:
        os: [macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: pnpm/action-setup@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Build Tauri app
        run: cd tauri-app && cargo tauri build

      - name: Install Playwright
        run: cd e2e && pnpm install && npx playwright install chromium

      - name: Run E2E tests
        run: cd e2e && npx playwright test

      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: e2e-results-${{ matrix.os }}
          path: |
            e2e/test-results/
            e2e/playwright-report/
            e2e/screenshots/
```

---

## 11. Adversarial E2E Scenarios (200+ Scenario Suite)

The project includes a **minimum 200 complex E2E scenarios** designed to break things. These are full-stack, UI-driven tests executed via Playwright + CDP against a running HiveMind OS instance with a real (or mock) daemon.

### Purpose

Unit and integration tests verify correctness. This suite verifies **resilience** — what happens when users do unexpected things, data is hostile, timing is unlucky, or multiple subsystems interact under stress.

### Scenario Categories

| Category | Count (min) | Examples |
|---|---|---|
| **Classification Boundary** | 25 | Send RESTRICTED data to public channel; chain reclassifications; override policy edge cases; redact-and-send with nested sensitive tokens |
| **Prompt Injection** | 25 | Inject instructions in tool results; role hijack via MCP response; exfil attempt via code output; scanner bypass attempts; nested injections; unicode/encoding tricks |
| **Chat Interaction** | 20 | Queue 10 commands rapidly; interrupt mid-tool-call; redirect during streaming; queue + interrupt + redirect combo; send while agent is paused |
| **Agentic Loop Stress** | 20 | 200+ turn session; deep recursion in custom stages; infinite loop detection; checkpoint corruption recovery; compaction under load |
| **Session Forking** | 15 | Fork at event 0; fork at last event; double fork; fork + compact parent; KG mutation in fork; classification inheritance across 5-deep fork chain |
| **Knowledge Graph** | 15 | 1M-node graph queries; concurrent writes; FTS + vector + CTE combined; orphan node cleanup; classification propagation with cycles |
| **Multi-Agent** | 15 | 5 agents pub/sub storm; circular delegation; blackboard contention; agent crash mid-delegation; role reassignment during execution |
| **MCP** | 15 | Server disconnect mid-tool-call; malformed responses; huge payloads; server returns injection payload; timeout + retry + fallback chain |
| **Model Layer** | 15 | All providers fail; rate limit cascade; context overflow triggers compaction; streaming interruption; model role fallback chain exhausted |
| **Peering** | 10 | Peer disconnect during sync; conflict resolution; classification mismatch between peers; offline queue overflow |
| **Messaging Bridges** | 10 | Unauthenticated user; pairing code expiry; classification gate on Discord response; concurrent commands from multiple platforms |
| **Visual Loop Designer** | 10 | Drag node to invalid position; save corrupt YAML; breakpoint on non-existent stage; rapid template switching; large loop (50+ stages) |
| **First-Run & Config** | 5 | Invalid API key; switch providers mid-session; corrupt config file recovery; migrate from conflicting configs |

### Scenario Spec Format

Each scenario is defined in a YAML file under `e2e/scenarios/`:

```yaml
# e2e/scenarios/classification/restricted-data-to-public-channel.yaml
id: CLS-001
category: classification_boundary
title: "RESTRICTED data blocked from public channel"
severity: critical
preconditions:
  - daemon running with default config
  - openai provider configured (channel_class: public)
  - override_policy.RESTRICTED.action: block
steps:
  - action: send_message
    text: "Summarise this: sk-abc123secretkey456"
  - action: wait_for
    element: classification-badge
    text: "RESTRICTED"
  - action: assert
    condition: message_not_sent_to_provider
  - action: assert_audit_log
    entry:
      action: blocked
      data_class: RESTRICTED
      channel: openai
expected_outcome: "Message is blocked, user sees classification badge, audit log entry exists"
tags: [security, classification, regression]
```

### Running the Suite

```powershell
# Run all 200+ scenarios
cd e2e
pnpm test:scenarios

# Run by category
pnpm test:scenarios -- --grep "classification"
pnpm test:scenarios -- --grep "prompt_injection"

# Run by severity
pnpm test:scenarios -- --grep "@critical"

# Run with mock providers (fast, deterministic)
HIVEMIND_TEST_MODE=mock pnpm test:scenarios

# Run with real providers (slow, non-deterministic — CI nightly only)
HIVEMIND_TEST_MODE=live pnpm test:scenarios
```

### Test Modes

| Mode | Providers | Speed | Determinism | When to Use |
|---|---|---|---|---|
| `mock` | Mock LLM, mock MCP servers, mock messaging | Fast (~10 min) | High | Every PR, local dev |
| `live` | Real LLMs, real MCP servers | Slow (~60+ min) | Variable | Nightly CI, release gate |
| `chaos` | Mock + random failures injected (latency, disconnects, corrupt responses) | Medium (~20 min) | Low | Weekly CI, hardening |

### Chaos Injection (chaos mode)

When running in `chaos` mode, a fault injection layer wraps all external calls:

| Fault | Probability | What It Tests |
|---|---|---|
| Model API timeout | 5% | Fallback chain, retry logic |
| Model API 429 (rate limit) | 10% | Backoff, provider rotation |
| MCP server disconnect | 3% | Reconnect, partial result handling |
| Malformed JSON response | 2% | Parser robustness |
| Network partition (5s) | 1% | Peering offline queue, daemon resilience |
| Artificial latency (2–10s) | 15% | UI responsiveness, timeout handling |

### Reporting

The scenario runner produces:
- **JUnit XML** for CI integration
- **HTML report** with screenshots at failure points
- **Risk coverage matrix** mapping scenarios → SPEC.md security controls
- **Flaky test tracker** — any test that passes/fails non-deterministically is flagged and triaged

### Maintenance

- Every new SPEC feature must have ≥2 adversarial scenarios added before the feature is considered complete.
- The 200 minimum is a floor — the suite grows over time.
- Scenarios are tagged and searchable. Regression scenarios are auto-added when bugs are fixed.

---

## 12. Key Gotchas

| Issue | Solution |
|---|---|
| `connectOverCDP` hangs | The app isn't running or the env var wasn't set. Verify with `curl http://127.0.0.1:9222/json/version` (or `Invoke-RestMethod` on Windows). |
| Port 9222 already in use | A previous app instance is still running. Find and kill it: `Get-Process hivemind* | Stop-Process` (Windows) or `pkill hivemind` (macOS). |
| `page` is `undefined` | Call `context.pages()[0]` — Tauri always creates one page. If it's not ready yet, use `context.waitForEvent("page")`. |
| macOS CDP env var | WebKitGTK uses `WEBKIT_INSPECTOR_SERVER`, not `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. The CDP connection works the same way. |
| Tests interfere with each other | Tauri state (e.g., Mutex counters) persists for the app's lifetime. Either reset state between tests via an API call, or restart the app. |
| Rust compile times are slow | Use `cargo check` instead of `cargo build` for syntax checks. Use `sccache` or `mold` linker. Only rebuild the crate you changed: `cargo test -p <crate>`. |
| Frontend `invoke()` fails in browser mode | You're running the Vite dev server without the Tauri shell. Add a Tauri mock layer (see §7 Workflow C). |
| Playwright can't find elements | Use `data-testid` attributes consistently. Use `browser_snapshot` (accessibility tree) to find the right selectors — prefer `getByRole`, `getByText`, `getByTestId`. |
| E2E tests are flaky | Add explicit waits: `await page.waitForSelector(...)`, `await expect(...).toBeVisible({ timeout: 10_000 })`. Use `test.describe.serial()` for order-dependent tests. |
| `cargo tauri dev` fails on Windows | Ensure WebView2 is installed and Visual Studio Build Tools include the C++ workload. Run `winget install Microsoft.EdgeWebView2Runtime`. |

---

## Appendix — Data-TestID Conventions

Use consistent `data-testid` attributes throughout the frontend for reliable test selectors:

```
chat-input               # Main message input
send-button              # Send message button
message-user             # User message bubble
message-assistant        # Assistant message bubble
classification-badge     # Data classification indicator
tool-call-block          # Collapsible tool call result
tool-call-name           # Tool name in a tool call block
override-prompt-modal    # Classification override prompt
override-allow-button    # Allow button in override prompt
override-deny-button     # Deny button in override prompt
agent-list               # Agent dashboard list
agent-card               # Individual agent card
agent-status             # Agent status indicator
kg-search-input          # Knowledge graph search input
kg-node-detail           # Knowledge graph node detail panel
settings-provider-list   # Provider configuration list
settings-save-button     # Settings save button
nav-conversations        # Navigation: conversations tab
nav-agents               # Navigation: agents tab
nav-knowledge            # Navigation: knowledge graph tab
nav-tasks                # Navigation: tasks tab
nav-settings             # Navigation: settings tab
```

