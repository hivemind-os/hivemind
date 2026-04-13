#!/bin/bash
# Build a macOS PKG installer for HiveMind OS.
#
# Usage: scripts/macos/build-pkg.sh [aarch64|x86_64]
#
# The PKG installs shared binaries system-wide:
#   - /Applications/HiveMind OS.app               (Tauri desktop app)
#       Contains daemon, CLI, and worker in Contents/Resources/ via
#       Tauri resource bundling, so in-app updates deliver them too.
#   - /usr/local/bin/hive-daemon                  (background daemon)
#   - /usr/local/bin/hive-cli                     (CLI tool)
#   - /usr/local/bin/hive-runtime-worker          (isolated inference worker)
#   - /usr/local/lib/libonnxruntime.<ver>.dylib   (ONNX Runtime; x86_64 only —
#       ort provides no prebuilt for x86_64-apple-darwin, so we download the
#       official Microsoft release and bundle it with the package.)
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

# ── ONNX Runtime for x86_64 ──────────────────────────────────────────────────
# ort v2.0.0-rc.11 ships no prebuilt ONNX Runtime binaries for x86_64-apple-darwin.
# For this target we download the official Microsoft release, patch its install
# name to the final installation path (/usr/local/lib), link dynamically at
# build time, and bundle the dylib inside the PKG payload so it is present at
# runtime after installation.
ORT_VERSION="1.23.1"
if [ "$ARCH" = "x86_64" ]; then
    ORT_CACHE_DIR="${HOME}/.cache/ort/onnxruntime-osx-x86_64-${ORT_VERSION}"
    if [ ! -d "$ORT_CACHE_DIR" ]; then
        echo "==> Downloading ONNX Runtime ${ORT_VERSION} for x86_64-apple-darwin..."
        mkdir -p "$(dirname "$ORT_CACHE_DIR")"
        curl -fL \
            "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-osx-x86_64-${ORT_VERSION}.tgz" \
            | tar -xz -C "$(dirname "$ORT_CACHE_DIR")"
        # Patch the dylib's own install name so that any binary linked against it
        # will look for /usr/local/lib/libonnxruntime.<ver>.dylib at runtime.
        # This matches the path where the PKG installs the dylib.
        install_name_tool -id \
            "/usr/local/lib/libonnxruntime.${ORT_VERSION}.dylib" \
            "${ORT_CACHE_DIR}/lib/libonnxruntime.${ORT_VERSION}.dylib"
    fi
    export ORT_LIB_LOCATION="${ORT_CACHE_DIR}/lib"
    export ORT_PREFER_DYNAMIC_LINK=1
fi

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

# 1. Build daemon and CLI first so they can be bundled into the .app.
echo "==> Building hive-daemon and hive-cli..."
cargo build --release --target "$TARGET" -p hive-daemon -p hive-cli -p hive-runtime-worker --features service-manager

# 1b. Sign all CLI binaries.
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

# 2. Stage signed binaries so Tauri bundles them as resources inside the .app.
#    Tauri places resources in Contents/Resources/, which resolve_daemon_binary()
#    knows how to find.
echo "==> Staging daemon binaries for Tauri resource bundling..."
DESKTOP_DIR="$REPO_ROOT/apps/hivemind-desktop"
STAGING_DIR="$DESKTOP_DIR/src-tauri/bin"
mkdir -p "$STAGING_DIR"
# Ensure staged binaries are cleaned up even on failure.
cleanup_staging() { rm -rf "$STAGING_DIR"; }
trap cleanup_staging EXIT
for bin in hive-daemon hive-cli hive-runtime-worker; do
    cp "$REPO_ROOT/target/$TARGET/release/$bin" "$STAGING_DIR/$bin"
    echo "  Staged $bin"
done

# 3. Build the Tauri desktop app with the macOS resource config overlay.
# When SIGN_IDENTITY is a real certificate, pass it as APPLE_SIGNING_IDENTITY so
# Tauri codesigns the .app bundle (using the entitlements in tauri.conf.json).
echo "==> Building Tauri desktop app..."
cd "$DESKTOP_DIR"
npm ci --prefer-offline
TAURI_BUILD_ARGS=(--target "$TARGET" --config src-tauri/tauri.macos-resources.conf.json --features service-manager)
if [ "$SIGN_IDENTITY" != "-" ]; then
    APPLE_SIGNING_IDENTITY="$SIGN_IDENTITY" npx tauri build "${TAURI_BUILD_ARGS[@]}"
else
    npx tauri build "${TAURI_BUILD_ARGS[@]}"
fi
cd "$REPO_ROOT"

# Verify the built .app bundle signature (catches any resource/signing issues early).
APP_BUNDLE=$(find "$REPO_ROOT/target/$TARGET/release/bundle" -name "HiveMind OS.app" -type d | head -1)
if [ -n "$APP_BUNDLE" ]; then
    echo "==> Verifying .app bundle signature..."
    codesign --verify --deep --strict "$APP_BUNDLE"
    echo "  Signature verified"
fi

# Rename updater artifacts to include architecture so aarch64 and x86_64
# don't collide when uploaded to the same GitHub release.
BUNDLE_DIR="$REPO_ROOT/target/$TARGET/release/bundle/macos"
for f in "$BUNDLE_DIR"/*.app.tar.gz "$BUNDLE_DIR"/*.app.tar.gz.sig; do
    [ -f "$f" ] || continue
    NEW_NAME=$(echo "$(basename "$f")" | sed "s/\.app\.tar\.gz/_${ARCH}.app.tar.gz/")
    mv "$f" "$BUNDLE_DIR/$NEW_NAME"
    echo "  Renamed $(basename "$f") -> $NEW_NAME"
done

# 4. Assemble the PKG payload
echo "==> Assembling PKG payload..."
mkdir -p "$PKG_ROOT/Applications"
mkdir -p "$PKG_ROOT/usr/local/bin"

# Bundle the ONNX Runtime dylib on x86_64: ort has no prebuilt for this target,
# so we ship the Microsoft dylib alongside the other binaries and install it to
# /usr/local/lib/ so hive-runtime-worker can load it at runtime.
if [ "$ARCH" = "x86_64" ]; then
    mkdir -p "$PKG_ROOT/usr/local/lib"
    cp "${ORT_CACHE_DIR}/lib/libonnxruntime.${ORT_VERSION}.dylib" \
        "$PKG_ROOT/usr/local/lib/"
    # Re-sign: install_name_tool above invalidated the original Microsoft signature.
    sign_binary "$PKG_ROOT/usr/local/lib/libonnxruntime.${ORT_VERSION}.dylib" \
        "com.hivemind.onnxruntime"
fi

APP_BUNDLE=$(find "$REPO_ROOT/target/$TARGET/release/bundle" -name "HiveMind OS.app" -type d | head -1)
if [ -z "$APP_BUNDLE" ]; then
    echo "ERROR: Could not find HiveMind OS.app bundle"
    exit 1
fi
cp -R "$APP_BUNDLE" "$PKG_ROOT/Applications/"
cp "$REPO_ROOT/target/$TARGET/release/hive-daemon" "$PKG_ROOT/usr/local/bin/"
cp "$REPO_ROOT/target/$TARGET/release/hive-cli" "$PKG_ROOT/usr/local/bin/"
cp "$REPO_ROOT/target/$TARGET/release/hive-runtime-worker" "$PKG_ROOT/usr/local/bin/"

# 5. Build the component package
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

# 6. Build the distribution (product) archive, then patch the Distribution XML
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
