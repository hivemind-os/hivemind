#!/bin/bash
# Sign and notarize a HiveMind OS macOS PKG for distribution.
#
# Usage: scripts/macos/sign-and-notarize.sh <path-to.pkg>
#
# Required environment variables:
#   APPLE_CERTIFICATE_NAME   - "Developer ID Installer: Team Name (TEAMID)"
#   APPLE_APP_CERT_NAME      - "Developer ID Application: Team Name (TEAMID)"
#   APPLE_ID                 - Apple ID email
#   APPLE_TEAM_ID            - Team ID
#   APPLE_ID_PASSWORD        - App-specific password (from appleid.apple.com)

set -euo pipefail

PKG_PATH="${1:?Usage: $0 <path-to.pkg>}"

if [ ! -f "$PKG_PATH" ]; then
    echo "ERROR: File not found: $PKG_PATH"
    exit 1
fi

# Derive the output path for the signed PKG
DIRNAME=$(dirname "$PKG_PATH")
BASENAME=$(basename "$PKG_PATH" .pkg)
SIGNED_PKG="$DIRNAME/${BASENAME}-signed.pkg"

echo "==> Signing PKG..."
productsign \
    --sign "${APPLE_CERTIFICATE_NAME:?Set APPLE_CERTIFICATE_NAME}" \
    "$PKG_PATH" \
    "$SIGNED_PKG"

echo "==> Submitting for notarization..."
xcrun notarytool submit "$SIGNED_PKG" \
    --apple-id "${APPLE_ID:?Set APPLE_ID}" \
    --team-id "${APPLE_TEAM_ID:?Set APPLE_TEAM_ID}" \
    --password "${APPLE_ID_PASSWORD:?Set APPLE_ID_PASSWORD}" \
    --wait

echo "==> Stapling notarization ticket..."
xcrun stapler staple "$SIGNED_PKG"

echo "==> Done: $SIGNED_PKG"
echo ""
echo "Verify with:"
echo "  spctl --assess --type install --verbose $SIGNED_PKG"
echo "  pkgutil --check-signature $SIGNED_PKG"
