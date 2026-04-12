# xtask

Developer workflow automation for the HiveMind OS project, using the [cargo-xtask](https://github.com/matklad/cargo-xtask) convention.

## Usage

```sh
cargo xtask <command>
```

## Commands

### `fetch-models`

Downloads the BGE-small-en-v1.5 embedding model files from Hugging Face into `vendor/bge-small-en-v1.5/`. Files that are already present are skipped.

```sh
cargo xtask fetch-models
```

### `build-installer`

Builds a platform installer for the HiveMind OS desktop application. If `--target` is omitted, the current platform is auto-detected.

```sh
cargo xtask build-installer [--target <TARGET>]
```

| Target           | Description                  | Output                                          |
|------------------|------------------------------|--------------------------------------------------|
| `macos-aarch64`  | macOS Apple Silicon PKG      | `dist/HiveMind-<version>-aarch64.pkg`               |
| `macos-x86_64`   | macOS Intel PKG              | `dist/HiveMind-<version>-x86_64.pkg`                |
| `windows-x64`    | Windows x64 NSIS installer   | `target/<triple>/release/bundle/nsis/HiveMind_*.exe` |
| `windows-arm64`  | Windows ARM64 NSIS installer | `target/<triple>/release/bundle/nsis/HiveMind_*.exe` |

## Prerequisites

- **Rust 1.85+**
- **Node.js 20+** (for Tauri build)
- **Tauri CLI v2** (`npx tauri`)
- **macOS:** Xcode Command Line Tools, `pkgbuild`, `productbuild`
- **Windows:** Visual Studio Build Tools (MSVC), WebView2
