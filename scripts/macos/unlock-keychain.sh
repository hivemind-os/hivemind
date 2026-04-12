#!/bin/bash
# Unlock the HiveMind dev keychain so codesign can run without password prompts.
#
# The keychain auto-locks after 8 hours of inactivity.  Run this before
# `cargo xtask run-daemon` or `cargo xtask build-daemon` if you hit a
# password prompt during codesigning.
#
# Usage: bash scripts/macos/unlock-keychain.sh

set -euo pipefail

KEYCHAIN_PATH="$HOME/Library/Keychains/hivemind-dev.keychain-db"
KEYCHAIN_PASS="hivemind-dev"

if [ ! -f "$KEYCHAIN_PATH" ]; then
    echo "Dev keychain not found. Run scripts/macos/create-dev-cert.sh first."
    exit 1
fi

security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH"
security set-key-partition-list \
    -S "apple-tool:,apple:,codesign:" \
    -s -k "$KEYCHAIN_PASS" \
    "$KEYCHAIN_PATH" >/dev/null

echo "✓ Keychain unlocked — codesign will work without prompts."
