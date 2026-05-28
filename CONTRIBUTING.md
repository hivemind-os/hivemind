# Contributing to HiveMind OS
Thanks for contributing to HiveMind OS.
This repository combines a Rust workspace with 35+ crates, a Tauri v2 + SolidJS desktop app, TypeScript packages, and the docs site. Use this guide to get set up quickly and submit changes that are easy to review.
## 1. Getting started
### Prerequisites
Verified from `C:\dev\hivemind\Cargo.toml`, `C:\dev\hivemind\README.md`, and package manifests:
- Rust **1.85+**
- Node.js **18+** (per README and package-level `engines` fields), with CI currently on Node 20
- **npm** (preferred here because the repo checks in `package-lock.json` files and CI uses `npm ci`)
- Tauri CLI **v2**
Helpful Windows extras:
- WebView2 for the desktop app
- Visual Studio C++ build tools for native Rust dependencies
#### GPU acceleration (optional)
To build with GPU support (`--gpu` flag or `cuda`/`metal` features):
- **Windows / Linux (CUDA):** Install the [NVIDIA CUDA Toolkit](https://developer.nvidia.com/cuda-downloads) (12.x recommended). Ensure `nvcc` is on your PATH after installation.
- **macOS (Metal):** No extra install needed — Metal support ships with Xcode Command Line Tools.

Without the CUDA toolkit installed, `cargo xtask run-daemon --gpu` and `cargo build --features cuda` will fail at compile time.

> **Note:** `cargo xtask` automatically sets `CMAKE_CUDA_ARCHITECTURES=75;80;86;89;90` to generate native GPU code for Turing through Hopper architectures. If you need a different set (e.g. only your local GPU), override via `$env:CMAKE_CUDA_ARCHITECTURES = "75"` before building.

### Clone, build, and test
```powershell
git clone https://github.com/hivemind-os/hivemind.git C:\dev\hivemind
Set-Location C:\dev\hivemind
# Rust workspace
cargo check --workspace
cargo build --workspace
cargo test --workspace
# Desktop app
Set-Location C:\dev\hivemind\apps\hivemind-desktop
npm ci
npm run build
npm run test:unit
```
### Run the app locally
```powershell
Set-Location C:\dev\hivemind\apps\hivemind-desktop
npm ci
npm run tauri:dev
```
Notes:
- `apps\hivemind-desktop\src-tauri\tauri.conf.json` runs `npm run dev` before launching Tauri and points at `http://localhost:3000`.
- For backend-only smoke tests, run `cargo run -p hive-daemon` from `C:\dev\hivemind`.
- The CLI is useful for quick checks: `cargo run -p hive-cli -- --help`.
## 2. Repository structure
High-level map:
- `C:\dev\hivemind\apps\hivemind-desktop\` - SolidJS frontend, Playwright tests, and the Tauri Rust wrapper under `src-tauri\`
- `C:\dev\hivemind\crates\` - Rust crates for the daemon, API, chat, workflows, tools, MCP, plugins, models, and shared infrastructure
- `C:\dev\hivemind\packages\` - TypeScript packages such as `plugin-sdk`, `test-plugin`, sample plugins, and scaffolding tools
- `C:\dev\hivemind\tests\` - workspace-level test assets and support files
- `C:\dev\hivemind\docs-site\` - VitePress source for the hosted docs
- `C:\dev\hivemind\docs\` - additional repo docs
- `C:\dev\hivemind\tools\` - supporting utilities such as the mock MCP server
- `C:\dev\hivemind\xtask\` - custom cargo tasks
For deeper system context, start with `C:\dev\hivemind\README.md`, `C:\dev\hivemind\ARCHITECTURE.md`, `C:\dev\hivemind\.github\copilot-instructions.md`, `C:\dev\hivemind\apps\hivemind-desktop\README.md`, and crate READMEs.
## 3. Development workflow
### Branch naming conventions
There is no hard documented branch naming rule.
Recent history shows both:
- short topical branches such as `providers2`, `plugins`, and `wf-dbg`
- namespaced branches such as `copilot/fix-build-issue-in-actions`
Use a short, descriptive branch name. If you want structure, `area/topic` or `type/topic` fits the repo well.
### Build and test your changes
Match validation to the scope of the change:
| Change type | Recommended commands |
|---|---|
| One Rust crate | `cargo check -p <crate>` then `cargo test -p <crate>` |
| Cross-crate Rust work | `cargo check --workspace` and `cargo test --workspace` |
| Desktop-only UI work | `npm run test:unit` and `npm run build` in `apps\hivemind-desktop` |
| Tauri bridge or UI + backend work | run both Rust and desktop checks, then smoke test with `npm run tauri:dev` |
### `cargo check` vs `cargo build` vs `cargo test`
- `cargo check` - fastest compile feedback; use it during iteration
- `cargo build` - use when you need real binaries or want to verify linking/build artifacts
- `cargo test` - use when behavior matters; this is the main regression guard
A practical Rust loop is:
```powershell
cargo check -p <crate>
cargo clippy -p <crate>
cargo test -p <crate>
```
Then expand to workspace-level commands before opening a PR.
### Frontend development workflow
From `C:\dev\hivemind\apps\hivemind-desktop`:
```powershell
npm ci
npm run dev
npm run tauri:dev
npm run test:unit
npm run build
npm run test:e2e:integration
npm run test:e2e:cdp
```
Use `npm` rather than `pnpm` unless you are deliberately working on tooling outside the current CI path.
## 4. Code style
### Rust
- Run `cargo fmt --all` before committing.
- The root `rustfmt.toml` currently sets `max_width = 100`, `newline_style = "Native"`, and `use_small_heuristics = "Max"`.
- Prefer workspace-inherited metadata and dependencies when possible.
### Clippy
Run:
```powershell
cargo clippy --workspace
```
CI runs Clippy at workspace scope, so fix warnings in the code you touch unless they are clearly unrelated baseline issues.
### TypeScript / SolidJS
- No shared checked-in ESLint or Prettier config was found under `apps\hivemind-desktop` or `packages\`.
- Preserve the surrounding style of the file you edit.
- Use package-local scripts where they exist. For example, `C:\dev\hivemind\packages\plugin-sdk\package.json` defines `npm run lint`.
### Naming conventions observed in the codebase
- Rust crates under `crates\` use the `hive-*` prefix; workspace utility and app crates (`xtask`, `hivemind-desktop`) may not.
- Rust modules and files are usually `snake_case`.
- Rust types and traits are `PascalCase`.
- Rust constants are `SCREAMING_SNAKE_CASE`.
- SolidJS components are usually `PascalCase.tsx`.
- TS stores, hooks, and helpers are usually `camelCase` or verb-based names such as `createWorkflowStore`, `useTimerCleanup`, and `toolCallTracker`.
- Frontend test files commonly use `*.spec.ts`; source-adjacent unit tests use `*.test.ts`.
## 5. Testing
For the full testing model, read `C:\dev\hivemind\TESTING_GUIDE.md`.
Short version:
- Add tests for new behavior and bug fixes.
- Start with the smallest relevant scope.
- Run workspace-level checks for cross-cutting Rust changes.
- Add frontend tests for user-visible behavior when practical.
Common commands:
```powershell
# Rust
cargo test --workspace
cargo test -p hive-api
cargo test -p hive-workflow --test integration
# Desktop frontend
Set-Location C:\dev\hivemind\apps\hivemind-desktop
npm run test:unit
npm run test:e2e:integration
npm run test:e2e:cdp
```
Observed test layout and naming:
- crate integration tests live under `crates\<crate>\tests\`
- Rust test files often use names such as `*_integration.rs`, `integration.rs`, `e2e_*.rs`, and targeted regression names such as `test_merge_bug.rs`
- desktop Playwright tests live under `apps\hivemind-desktop\tests\**\*.spec.ts`
Common integration-test pattern:
- boot a real subsystem rather than mocking everything
- use temp directories and ephemeral localhost ports
- use `tokio::test` for async flows
- use `hive-test-utils` when shared helpers or scripted providers are needed
Good examples:
- `C:\dev\hivemind\crates\hive-api\tests\knowledge_integration.rs`
- `C:\dev\hivemind\crates\hive-workflow\tests\integration.rs`
## 6. Commit messages
Recent history strongly favors Conventional Commit-style subjects:
- `feat: ...`
- `fix: ...`
- `chore: ...`
- `ci: ...`
- `fix(ci): ...`
There are a few exceptions (`wip`, ad-hoc subjects, merge commits), but contributors should prefer:
```text
type: short imperative summary
```
Examples:
- `feat: add workflow preview for managed definitions`
- `fix: handle empty MCP registry response`
- `docs: clarify plugin SDK build steps`
Avoid `wip` commits on shared branches.
## 7. Pull requests
A good PR should include:
- a clear summary of what changed and why
- linked issues or background when relevant
- test evidence (`cargo test`, `npm run test:unit`, screenshots, or E2E notes)
- screenshots or recordings for visible desktop changes
- migration or config notes when behavior changes for users or plugin authors
Review expectations:
- keep PRs focused
- update docs and tests when behavior changes
- respond to review comments with code or rationale
- call out follow-up work instead of hiding it
### CI checks that must pass
From `C:\dev\hivemind\.github\workflows\ci.yml`, CI currently covers:
- `cargo xtask check-version`
- `cargo check --workspace`
- `cargo clippy --workspace`
- `cargo test --workspace`
- desktop frontend dependency install and `npm run build`
- `tools\mock-mcp-server` build
- `packages\plugin-sdk` build
- `packages\test-plugin` build
- macOS and Windows matrix coverage
Treat that workflow as the minimum merge bar.
## 8. Adding new crates
When adding a crate:
1. Create it under `C:\dev\hivemind\crates\<new-crate>`.
2. Add it to `[workspace].members` in `C:\dev\hivemind\Cargo.toml`.
3. Follow the existing manifest pattern.
Typical crate metadata looks like this:
```toml
[package]
name = "hive-your-crate"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
description = "Short crate purpose"
```
Recommended conventions:
- keep the `hive-*` naming scheme
- prefer workspace dependencies with `*.workspace = true`
- use local path dependencies for sibling crates
- add meaningful `description` text
- add `dev-dependencies` when the crate has integration tests
Documentation expectations:
- add or update a crate README for reusable libraries, binaries, or new subsystems
- keep the crate description in `Cargo.toml` meaningful
- update `README.md`, `TESTING_GUIDE.md`, or docs-site content if contributor workflows change
## 9. Adding new features
Before making a cross-cutting change, identify which layer owns the behavior:
- desktop UI in `apps\hivemind-desktop\src\`
- Tauri glue in `apps\hivemind-desktop\src-tauri\`
- daemon and services in `crates\`
- plugin-facing TypeScript APIs in `packages\`
There is **not currently** a `C:\dev\hivemind\copilot-instructions.md` file at the repo root, but `.github\copilot-instructions.md` provides agent-oriented guidance including common task recipes. Also consult:
- `C:\dev\hivemind\ARCHITECTURE.md`
- `C:\dev\hivemind\README.md`
- `C:\dev\hivemind\TESTING_GUIDE.md`
- `C:\dev\hivemind\apps\hivemind-desktop\README.md`
- crate READMEs under `C:\dev\hivemind\crates\`
A good feature workflow is:
1. understand the owning crate or app surface
2. make the smallest coherent change
3. add or update tests close to the changed behavior
4. run targeted checks first, then broader workspace checks
5. update docs when contributor or user workflows change
Thanks again for helping improve HiveMind OS.
