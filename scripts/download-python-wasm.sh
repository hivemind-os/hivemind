#!/usr/bin/env bash
#
# Downloads the CPython WASI runtime for HiveMind CodeAct.
#
# Fetches the CPython 3.12 WASI build from vmware-labs/webassembly-language-runtimes
# and installs it to ~/.hivemind/runtimes/python-wasm/.
#
# Usage:
#   ./download-python-wasm.sh [INSTALL_DIR]
#
set -euo pipefail

RELEASE_TAG="python/3.12.0+20231211-040d5a6"
ASSET_NAME="python-3.12.0-wasi-sdk-20.0.tar.gz"
ENCODED_TAG=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$RELEASE_TAG', safe=''))" 2>/dev/null \
    || echo "python%2F3.12.0%2B20231211-040d5a6")
DOWNLOAD_URL="https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/${ENCODED_TAG}/${ASSET_NAME}"

INSTALL_DIR="${1:-$HOME/.hivemind/runtimes/python-wasm}"

echo "HiveMind CodeAct — CPython WASI Runtime Installer"
echo "================================================="
echo ""
echo "Source:  $DOWNLOAD_URL"
echo "Target:  $INSTALL_DIR"
echo ""

# Check if already installed
WASM_BIN="$INSTALL_DIR/bin/python.wasm"
if [ -f "$WASM_BIN" ]; then
    echo "python.wasm already installed at $WASM_BIN"
    read -rp "Reinstall? (y/N) " response
    if [ "$response" != "y" ]; then
        echo "Skipped."
        exit 0
    fi
fi

# Download
TEMP_DIR=$(mktemp -d)
TAR_PATH="$TEMP_DIR/$ASSET_NAME"
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "Downloading ($ASSET_NAME)..."
if command -v curl &>/dev/null; then
    curl -fSL "$DOWNLOAD_URL" -o "$TAR_PATH"
elif command -v wget &>/dev/null; then
    wget -q "$DOWNLOAD_URL" -O "$TAR_PATH"
else
    echo "Error: curl or wget required" >&2
    exit 1
fi

SIZE_MB=$(du -m "$TAR_PATH" | cut -f1)
echo "Downloaded ${SIZE_MB} MB"

# Extract
EXTRACT_DIR="$TEMP_DIR/extracted"
mkdir -p "$EXTRACT_DIR"
echo "Extracting..."
tar -xzf "$TAR_PATH" -C "$EXTRACT_DIR"

# Install to normalized layout
echo "Installing to $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/lib"

# Find the .wasm binary (may be named python-3.12.0.wasm)
WASM_FILE=$(find "$EXTRACT_DIR/bin" -name 'python*.wasm' | head -1)
if [ -z "$WASM_FILE" ]; then
    echo "Error: Could not find python*.wasm in archive" >&2
    exit 1
fi
cp "$WASM_FILE" "$INSTALL_DIR/bin/python.wasm"

# Copy stdlib directory
STDLIB_SRC="$EXTRACT_DIR/usr/local/lib/python3.12"
if [ -d "$STDLIB_SRC" ]; then
    rm -rf "$INSTALL_DIR/lib/python3.12"
    cp -r "$STDLIB_SRC" "$INSTALL_DIR/lib/python3.12"
fi

# Also copy the zipped stdlib if present
STDLIB_ZIP="$EXTRACT_DIR/usr/local/lib/python312.zip"
if [ -f "$STDLIB_ZIP" ]; then
    cp "$STDLIB_ZIP" "$INSTALL_DIR/lib/python312.zip"
fi

# Verify
FINAL_WASM="$INSTALL_DIR/bin/python.wasm"
FINAL_STDLIB="$INSTALL_DIR/lib/python3.12"
if [ -f "$FINAL_WASM" ] && [ -d "$FINAL_STDLIB" ]; then
    WASM_SIZE=$(du -m "$FINAL_WASM" | cut -f1)
    echo ""
    echo "Installation complete!"
    echo "  python.wasm: $FINAL_WASM (${WASM_SIZE} MB)"
    echo "  stdlib:      $FINAL_STDLIB"
    echo ""
    echo "HiveMind will auto-detect this runtime on next startup."
else
    echo "Warning: Installation may be incomplete — check $INSTALL_DIR" >&2
fi
