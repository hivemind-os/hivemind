# HiveMind OS Installer — Code Signing & Release Guide

This document covers how to set up code signing, build installers, and publish
releases for the HiveMind OS desktop application.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Prerequisites](#prerequisites)
3. [Tauri Update Signing Keys](#tauri-update-signing-keys)
4. [Apple Code Signing & Notarization](#apple-code-signing--notarization)
5. [Windows Code Signing](#windows-code-signing)
6. [GitHub Actions Secrets](#github-actions-secrets)
7. [Building Locally](#building-locally)
8. [Publishing a Release](#publishing-a-release)
9. [How Updates Work](#how-updates-work)
10. [Multi-User Architecture](#multi-user-architecture)
11. [Known Limitations](#known-limitations)
12. [Troubleshooting](#troubleshooting)

---

## Architecture Overview

HiveMind OS consists of three binaries:

| Binary | Purpose |
|--------|---------|
| `hivemind-desktop` | Tauri GUI — thin HTTP client to the daemon |
| `hive-daemon` | Axum HTTP API server — the backend |
| `hive-cli` | CLI tool for daemon management |

The installer places shared binaries system-wide. Each user gets their own
daemon instance via per-user auto-start:

- **macOS**: LaunchAgent in `~/Library/LaunchAgents/`
- **Windows**: Registry Run key in `HKCU\...\CurrentVersion\Run`

The desktop app registers the auto-start on first launch. No admin privileges
are needed for per-user service registration.

### System Tray / Menu Bar

The desktop app lives in the system tray (Windows) or menu bar (macOS):

- **Open HiveMind OS** — shows the main window
- **Daemon: running on 127.0.0.1:9180** — live status (polled every 10s)
- **Quit HiveMind OS** — exits the app

Closing the window hides it to the tray instead of quitting. The daemon
continues running.

### Dynamic Port Discovery

The daemon writes its actual bound address to `~/.hivemind/run/daemon.addr`. This
enables multiple users on the same machine to each run their own daemon on
dynamically assigned ports (set `api.bind: "127.0.0.1:0"` in config).

---

## Prerequisites

- Rust 1.85+
- Node.js 20+
- Tauri CLI v2: `npm install -g @tauri-apps/cli`
- Platform-specific:
  - **macOS**: Xcode Command Line Tools, `pkgbuild`, `productbuild`
  - **Windows**: Visual Studio Build Tools (MSVC), WebView2

---

## Tauri Update Signing Keys

The Tauri updater uses Ed25519 signatures to verify update integrity.

### Generate keys

```bash
npx tauri signer generate -w ~/.tauri/hivemind.key
```

This creates:
- `~/.tauri/hivemind.key` — **private key** (NEVER commit this)
- `~/.tauri/hivemind.key.pub` — **public key**

### Configure

1. Copy the public key content
2. Paste it into `apps/hivemind-desktop/src-tauri/tauri.conf.json`:
   ```json
   {
     "plugins": {
       "updater": {
         "pubkey": "dW50cnVzdGVkIGNvbW1lbnQgc2lnbmF0dXJlOi..."
       }
     }
   }
   ```
3. Store the private key as a CI secret (see [GitHub Actions Secrets](#github-actions-secrets))

> **Note**: The updater plugin is only activated when `pubkey` is non-empty.
> During development, leave it empty and the updater will be skipped.

---

## Apple Code Signing & Notarization

### 1. Enroll in Apple Developer Program

- Go to [developer.apple.com/programs](https://developer.apple.com/programs/)
- Cost: $99/year (individual or organization)
- Required for distribution outside the Mac App Store

### 2. Create Certificates

In the Apple Developer portal → Certificates, Identifiers & Profiles:

1. **Developer ID Application** — for signing `.app` bundles and binaries
2. **Developer ID Installer** — for signing `.pkg` installers

### 3. Export Certificates

1. Open Keychain Access
2. Find each certificate under "My Certificates"
3. Right-click → Export → save as `.p12` with a password
4. Base64-encode for CI: `base64 -i cert.p12 | pbcopy`

### 4. Create an App-Specific Password

1. Go to [appleid.apple.com](https://appleid.apple.com)
2. Sign In → Security → App-Specific Passwords → Generate
3. Save the password for CI use

### 5. Local Signing (optional)

```bash
# Sign binaries
codesign --deep --force --options runtime \
  --sign "Developer ID Application: Your Team (TEAMID)" \
  target/release/hive-daemon

# Sign and notarize a PKG
bash scripts/macos/sign-and-notarize.sh dist/HiveMind-0.1.0-aarch64.pkg
```

Required environment variables for `sign-and-notarize.sh`:
- `APPLE_CERTIFICATE_NAME` — e.g., `"Developer ID Installer: Your Team (TEAMID)"`
- `APPLE_ID` — your Apple ID email
- `APPLE_TEAM_ID` — your 10-character Team ID
- `APPLE_ID_PASSWORD` — the app-specific password

---

## Windows Code Signing

### 1. Obtain a Certificate

Purchase from a Certificate Authority:

| Type | Cost | SmartScreen | Notes |
|------|------|-------------|-------|
| **EV** (Extended Validation) | $300-500/yr | Instant reputation | Hardware token required |
| **OV** (Organization Validation) | $70-200/yr | Builds over time | Software .pfx file |

Recommended CAs: DigiCert, Sectigo, SSL.com, GlobalSign

### 2. Export as PFX

If you received a `.cer` file, combine with private key into `.pfx`:
```powershell
# If you have a .cer and private key in the cert store:
certutil -exportPFX -p "password" My "Your Certificate CN" cert.pfx
```

### 3. Base64-Encode for CI

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx")) | Set-Clipboard
```

### 4. Local Signing (optional)

```powershell
signtool sign /fd SHA256 /t http://timestamp.digicert.com `
  /f cert.pfx /p "password" `
  target\release\hive-daemon.exe
```

---

## GitHub Actions Secrets

Configure these in **Settings → Secrets and variables → Actions**:

### Required for any release

| Secret | Description |
|--------|-------------|
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri Ed25519 private key (from `npx tauri signer generate`) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the private key (if set) |

### Required for macOS signing & notarization

| Secret | Description |
|--------|-------------|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` containing both App + Installer certs |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` file |
| `APPLE_CERTIFICATE_NAME` | Full name: `"Developer ID Installer: Team (TEAMID)"` |
| `APPLE_APP_CERT_NAME` | Full name: `"Developer ID Application: Team (TEAMID)"` |
| `APPLE_ID` | Apple Developer account email |
| `APPLE_TEAM_ID` | 10-character Team ID |
| `APPLE_ID_PASSWORD` | App-specific password |

### Required for Windows signing

| Secret | Description |
|--------|-------------|
| `WINDOWS_CERTIFICATE` | Base64-encoded `.pfx` code signing cert |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx` file |

---

## Building Locally

### Auto-detect platform

```bash
cargo xtask build-installer
```

### Specify target

```bash
cargo xtask build-installer --target macos-aarch64
cargo xtask build-installer --target macos-x86_64
cargo xtask build-installer --target windows-x64
cargo xtask build-installer --target windows-arm64
```

### macOS PKG only

```bash
bash scripts/macos/build-pkg.sh aarch64   # or x86_64
```

### Output locations

- macOS: `dist/HiveMind-<version>-<arch>.pkg`
- Windows: `target/<target>/release/bundle/nsis/HiveMind_<version>_*.exe`

---

## Publishing a Release

### 1. Bump version

Update the version in:
- `Cargo.toml` (workspace `version`)
- `apps/hivemind-desktop/src-tauri/tauri.conf.json` (`version` field)

### 2. Tag and push

```bash
git tag v0.2.0
git push origin v0.2.0
```

### 3. CI builds automatically

The `release.yml` workflow:
1. Builds 4 platform installers (macOS aarch64/x86_64, Windows x64/arm64)
2. Signs and notarizes (if secrets are configured)
3. Generates `latest.json` update manifest
4. Creates a GitHub Release with all artifacts

### 4. Verify the release

- Check the GitHub Release page for all expected artifacts
- Download and test on each platform
- Verify auto-updater detects the new version

---

## How Updates Work

### Desktop app (auto-update)

The desktop app checks for updates automatically in the background:

1. On startup (after a 30-second delay), the Rust backend emits an `update:check`
   event. The frontend handles this by calling the Tauri updater plugin's `check()`
   API against `latest.json` at the configured GitHub Releases endpoint.
2. If a newer version is found, an update dialog is shown with the new version
   number and release notes.
3. The user can choose **Update Now** (downloads and installs immediately with
   a progress bar) or **Remind Me Later** (dismissed until the next check cycle).
4. After installation, the user is prompted to restart the app.
5. On restart, the desktop app's service registration runs again (idempotent),
   updating the daemon binary path in the LaunchAgent/registry if it changed.
6. The daemon auto-restarts with the new binary via the per-user auto-start.

Background checks repeat every 6 hours. Users can also trigger a check
manually via the **Check for Updates** item in the system tray menu.

> **Note**: Auto-update is only active when a signing `pubkey` is configured in
> `tauri.conf.json`. Dev builds leave this empty, so the updater is disabled.

### CLI (`hive update`)

The CLI provides a manual update check:

```bash
hive update
```

This fetches `latest.json`, compares the version with the running binary, and
prints download instructions if a newer version is available.

### Update manifest (`latest.json`)

The release workflow generates this file. It contains platform-specific
entries with download URLs and Ed25519 signatures. The Tauri updater matches
the current platform to the correct entry.

---

## Multi-User Architecture

Each user on a shared machine gets full isolation:

| Resource | Location | Per-user? |
|----------|----------|-----------|
| Config | `~/.hivemind/config.yaml` | ✅ |
| Data (sessions, workflows) | `~/.hivemind/` | ✅ |
| Auth token | OS keyring | ✅ |
| Daemon PID | `~/.hivemind/run/hive-daemon.pid` | ✅ |
| Daemon address | `~/.hivemind/run/daemon.addr` | ✅ |
| Auto-start | LaunchAgent / Registry Run | ✅ |
| App binary | `/Applications/` or `C:\Program Files\` | Shared |

### Enabling dynamic ports

For multi-user setups, each user should configure a dynamic port:

```yaml
# ~/.hivemind/config.yaml
api:
  bind: "127.0.0.1:0"
```

This lets the OS assign a free port. The daemon writes the actual address to
`~/.hivemind/run/daemon.addr`, and all clients (desktop, CLI) auto-discover it.

---

## Known Limitations

### Stale discovery file after daemon crash

If the daemon crashes without clean shutdown, `~/.hivemind/run/daemon.addr`
retains the old address. Clients will attempt to connect, fail, and fall
through to the config-based URL. For fixed ports (default 9180), this is
harmless since the new daemon binds the same port. For dynamic ports,
`daemon_start()` may briefly poll the wrong address until the new daemon
overwrites the file.

**Workaround**: Delete `~/.hivemind/run/daemon.addr` manually, or fix by having
`daemon_start()` re-resolve the URL after spawning the daemon process.

### Windows ARM64 cross-compilation

The release workflow uses `windows-latest` (x86_64) to build ARM64 targets.
This requires the ARM64 MSVC build tools, which may not be pre-installed on
GitHub Actions runners. You may need to:
- Add a step to install ARM64 build tools
- Use a self-hosted ARM64 runner
- Or temporarily disable the ARM64 build

### macOS binary signing before PKG assembly

The current `build-pkg.sh` copies binaries into the PKG payload without
signing them individually. For proper notarization, each binary
(`hive-daemon`, `hive-cli`) and the `.app` bundle should be code-signed
before the PKG is assembled. Add codesign steps to `build-pkg.sh` after
the binary copy steps.

---

## Troubleshooting

### macOS: "HiveMind OS.app is damaged and can't be opened"

The app is not signed or notarized. Either:
- Sign and notarize the PKG (see above)
- Or bypass Gatekeeper (development only): `xattr -cr /Applications/HiveMind OS.app`

### Windows: "Windows protected your PC" (SmartScreen)

The installer is not signed with a trusted certificate. Either:
- Sign with an EV certificate (instant reputation)
- Sign with an OV certificate (reputation builds over time)
- Or click "More info" → "Run anyway" (development only)

### Daemon doesn't auto-start at login

- **macOS**: Check `launchctl list | grep hivemind`. If missing, re-launch the
  desktop app to re-register. Check logs: `~/.hivemind/logs/launchd-daemon.*.log`
- **Windows**: Check `reg query "HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" /v HiveMindDaemon`.
  If missing, re-launch the desktop app.

### Port conflict (multi-user)

If two users both use the default port 9180, the second daemon will fail to
start. Set `api.bind: "127.0.0.1:0"` in each user's `~/.hivemind/config.yaml`.
