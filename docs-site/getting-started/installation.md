# Installation

HiveMind OS installs as a single desktop app — the Rust daemon starts automatically in the background, so there's nothing extra to configure.

## Download

Pick the installer for your platform:

| Platform | Download | Format |
|---|---|---|
| **macOS** (Apple Silicon) | [HiveMind-aarch64-signed.pkg](https://github.com/hivemind-os/hivemind/releases/latest/download/HiveMind-0.1.3-aarch64-signed.pkg) | `.pkg` |
| **macOS** (Intel) | [HiveMind-x86_64-signed.pkg](https://github.com/hivemind-os/hivemind/releases/latest/download/HiveMind-0.1.3-x86_64-signed.pkg) | `.pkg` |
| **Windows** (x64) | [HiveMind.OS-x64-setup.exe](https://github.com/hivemind-os/hivemind/releases/latest/download/HiveMind.OS_0.1.3_x64-setup.exe) | `.exe` |
| **Windows** (ARM64) | [HiveMind.OS-arm64-setup.exe](https://github.com/hivemind-os/hivemind/releases/latest/download/HiveMind.OS_0.1.3_arm64-setup.exe) | `.exe` |

All installers are available on the [GitHub Releases page](https://github.com/hivemind-os/hivemind/releases).

## Install

::: code-group

```sh [macOS]
# Open the downloaded .pkg installer and follow the prompts
open HiveMind-*-signed.pkg
```

```sh [Windows]
# Run the NSIS installer — follow the wizard prompts
.\HiveMind.OS_*-setup.exe
```

:::

## What Happens on First Launch

1. **The daemon starts automatically.** The Rust daemon (`hive-daemon`) launches in the background and exposes a local HTTP API. You don't need to start it manually — the desktop app handles this for you.

2. **A system tray icon appears.** Look for the HiveMind OS icon in your menu bar (macOS) or system tray (Windows/Linux). Right-click it for quick access to settings, logs, and quit.

3. **The main window opens.** The Tauri desktop app (SolidJS frontend) connects to the daemon's local API. You're ready to chat, configure providers, and create personas.

::: tip No daemon? No problem.
If you ever need to start the daemon manually — for example, when using just the CLI — run:
```sh
hive daemon start
```
:::

## Verify Your Installation

After launching, confirm everything is working:

```sh
# Check that the daemon is running
hive daemon status
```

You should see output like:

```
✔ HiveMind OS daemon is running
  API: http://localhost:9180
  Version: 0.1.0
  Uptime: 12s
```

You can also verify from the desktop app — open **Settings → About** to see the daemon version and connection status.

## Next Steps

You're all set! Head to the [Quickstart](/getting-started/quickstart) to add your first provider and start chatting, or jump to [First Five Minutes](/getting-started/first-five-minutes) for a guided walkthrough.
