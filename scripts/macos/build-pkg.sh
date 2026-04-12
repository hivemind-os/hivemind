#!/bin/bash
# Build a macOS PKG installer for HiveMind OS.
#
# Usage: scripts/macos/build-pkg.sh [aarch64|x86_64]
#
# The PKG installs shared binaries system-wide:
#   - /Applications/HiveMind OS.app          (Tauri desktop app)
#   - /usr/local/bin/hive-daemon      (background daemon)
#   - /usr/local/bin/hive-cli         (CLI tool)
#   - /usr/local/bin/hive-runtime-worker (isolated inference worker)
#
# Per-user service registration happens on first launch of the desktop app,
# NOT during install, so each user gets their own daemon instance.

set -euo pipefail

ARCH="${1:-aarch64}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

case "$ARCH" in
    aarch64) TARGET="aarch64-apple-darwin" ;;
    x86_64)  TARGET="x86_64-apple-darwin" ;;
    *)
        echo "Usage: $0 [aarch64|x86_64]"
        exit 1
        ;;
esac

VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')
PKG_ROOT=$(mktemp -d)
IDENTIFIER="com.hivemind.desktop"
OUTPUT_DIR="$REPO_ROOT/dist"
mkdir -p "$OUTPUT_DIR"

# ── Determine code-signing identity ──────────────────────────────────────────
# Priority: APPLE_APP_CERT_NAME (CI/distribution) > CODESIGN_IDENTITY (dev) > - (ad-hoc)
#
# APPLE_APP_CERT_NAME: "Developer ID Application: Team Name (TEAMID)"
#   Set in CI by the signing step.  Enables hardened runtime + proper notarization.
#
# CODESIGN_IDENTITY: local stable cert (see scripts/macos/create-dev-cert.sh).
#   Keeps TCC grants and keychain entries valid across rebuilds.
#
# Unset / empty: ad-hoc signing (-).  Fine for one-off local runs.
if [ -n "${APPLE_APP_CERT_NAME:-}" ]; then
    SIGN_IDENTITY="$APPLE_APP_CERT_NAME"
    USE_HARDENED_RUNTIME=true
elif [ -n "${CODESIGN_IDENTITY:-}" ]; then
    SIGN_IDENTITY="$CODESIGN_IDENTITY"
    USE_HARDENED_RUNTIME=false
else
    SIGN_IDENTITY="-"
    USE_HARDENED_RUNTIME=false
fi

# Helper: codesign a single binary.
# Usage: sign_binary <path> <bundle-id> [entitlements-plist]
sign_binary() {
    local path="$1" bundle_id="$2" entitlements="${3:-}"
    local args=(--force --sign "$SIGN_IDENTITY" --identifier "$bundle_id")
    if $USE_HARDENED_RUNTIME; then
        args+=(--options runtime)
        [ -n "$entitlements" ] && args+=(--entitlements "$entitlements")
    fi
    codesign "${args[@]}" "$path"
}

echo "==> Building HiveMind OS $VERSION for $TARGET (signing: ${SIGN_IDENTITY})"

# 1. Build the Tauri desktop app.
# When SIGN_IDENTITY is a real certificate, pass it as APPLE_SIGNING_IDENTITY so
# Tauri codesigns the .app bundle (using the entitlements in tauri.conf.json).
echo "==> Building Tauri desktop app..."
cd "$REPO_ROOT/apps/hivemind-desktop"
npm ci --prefer-offline
if [ "$SIGN_IDENTITY" != "-" ]; then
    APPLE_SIGNING_IDENTITY="$SIGN_IDENTITY" npx tauri build --target "$TARGET" --features service-manager
else
    npx tauri build --target "$TARGET" --features service-manager
fi
cd "$REPO_ROOT"

# 2. Build daemon and CLI
echo "==> Building hive-daemon and hive-cli..."
cargo build --release --target "$TARGET" -p hive-daemon -p hive-cli -p hive-runtime-worker --features service-manager

# 2b. Sign all CLI binaries.
#
#   hive-daemon        — bound to its embedded Info.plist so macOS TCC shows
#                        permission prompts for Calendar / Contacts access.
#                        Entitlements declare those TCC permissions for
#                        hardened-runtime builds.
#   hive-cli           — hardened runtime only; no special permissions.
#   hive-runtime-worker — hardened runtime only; no special permissions.
#
# For distribution builds (APPLE_APP_CERT_NAME set) this uses Developer ID +
# hardened runtime as required for notarization.
echo "==> Signing CLI binaries (identity: ${SIGN_IDENTITY})..."
sign_binary "$REPO_ROOT/target/$TARGET/release/hive-daemon" \
    "com.hivemind.daemon" \
    "$SCRIPT_DIR/entitlements-daemon.plist"
sign_binary "$REPO_ROOT/target/$TARGET/release/hive-cli" \
    "com.hivemind.cli" \
    "$SCRIPT_DIR/entitlements-cli.plist"
sign_binary "$REPO_ROOT/target/$TARGET/release/hive-runtime-worker" \
    "com.hivemind.runtime-worker" \
    "$SCRIPT_DIR/entitlements-cli.plist"

# 3. Assemble the PKG payload
echo "==> Assembling PKG payload..."
mkdir -p "$PKG_ROOT/Applications"
mkdir -p "$PKG_ROOT/usr/local/bin"

# Find the .app bundle — Tauri puts it in the target bundle directory
APP_BUNDLE=$(find "$REPO_ROOT/target/$TARGET/release/bundle" -name "HiveMind OS.app" -type d | head -1)
if [ -z "$APP_BUNDLE" ]; then
    echo "ERROR: Could not find HiveMind OS.app bundle"
    exit 1
fi

cp -R "$APP_BUNDLE" "$PKG_ROOT/Applications/"
cp "$REPO_ROOT/target/$TARGET/release/hive-daemon" "$PKG_ROOT/usr/local/bin/"
cp "$REPO_ROOT/target/$TARGET/release/hive-cli" "$PKG_ROOT/usr/local/bin/"
cp "$REPO_ROOT/target/$TARGET/release/hive-runtime-worker" "$PKG_ROOT/usr/local/bin/"

# 4. Build the component package
#    Generate a component plist and set BundleIsRelocatable to false so that
#    macOS Installer doesn't move the .app to wherever Spotlight/PackageKit
#    found an existing copy with the same bundle identifier.
echo "==> Building component PKG..."
chmod +x "$SCRIPT_DIR/scripts/"*

COMPONENT_PLIST=$(mktemp /tmp/component-plist.XXXXXX)
pkgbuild --analyze --root "$PKG_ROOT" "$COMPONENT_PLIST"
# Patch every BundleIsRelocatable entry to false
/usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" "$COMPONENT_PLIST" 2>/dev/null || true

pkgbuild \
    --root "$PKG_ROOT" \
    --identifier "$IDENTIFIER" \
    --version "$VERSION" \
    --scripts "$SCRIPT_DIR/scripts" \
    --install-location "/" \
    --component-plist "$COMPONENT_PLIST" \
    "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}-component.pkg"

rm -f "$COMPONENT_PLIST"

# 5. Build the distribution (product) archive, then patch the Distribution XML
#    to add <domains> for system-wide install.  productbuild's auto-generated
#    Distribution lacks this element, which can cause "incompatible" errors.
echo "==> Building distribution PKG..."
productbuild \
    --package "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}-component.pkg" \
    "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}.pkg"

PATCH_DIR=$(mktemp -d)
pkgutil --expand "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}.pkg" "$PATCH_DIR/pkg"

# Inject <domains> to force system-wide install if not already present.
if ! grep -q '<domains' "$PATCH_DIR/pkg/Distribution"; then
    sed -i '' \
        's|<options |<domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>\n    <options |' \
        "$PATCH_DIR/pkg/Distribution"
fi

pkgutil --flatten "$PATCH_DIR/pkg" "$PATCH_DIR/HiveMind-patched.pkg"
mv -f "$PATCH_DIR/HiveMind-patched.pkg" "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}.pkg"
rm -rf "$PATCH_DIR"

# Clean up intermediate files
rm -f "$OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}-component.pkg"
rm -rf "$PKG_ROOT"

echo "==> Built: $OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}.pkg"
echo ""
echo "To sign and notarize, run:"
echo "  scripts/macos/sign-and-notarize.sh $OUTPUT_DIR/HiveMind-${VERSION}-${ARCH}.pkg"
