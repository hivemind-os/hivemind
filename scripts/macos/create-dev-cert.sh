#!/bin/bash
# Create a stable self-signed code-signing certificate for local development.
#
# Ad-hoc signing (--sign -) produces a different signature hash on every
# rebuild, which breaks TCC permissions and Data Protection Keychain access
# for hive-daemon between builds.  This script creates a named self-signed
# certificate that stays stable across rebuilds.
#
# Run this once per machine.  Safe to re-run — exits 0 if already set up.
#
# After running, add to your shell profile:
#   export CODESIGN_IDENTITY="HiveMind Dev"
#
# Usage: bash scripts/macos/create-dev-cert.sh

set -euo pipefail

CERT_NAME="HiveMind Dev"
KEYCHAIN_PATH="$HOME/Library/Keychains/hivemind-dev.keychain-db"
KEYCHAIN_PASS="hivemind-dev"
P12_PASS="hivemind-dev-p12"

# ── Ensure the dedicated dev keychain exists and is unlocked ──────────────────
if [ ! -f "$KEYCHAIN_PATH" ]; then
    echo "==> Creating dedicated dev keychain..."
    security create-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH"
fi

security unlock-keychain -p "$KEYCHAIN_PASS" "$KEYCHAIN_PATH"
# Auto-lock after 8 hours; prevent sleep from locking it mid-build
security set-keychain-settings -t 28800 -u "$KEYCHAIN_PATH"

# ── Import the certificate if it isn't already in the keychain ────────────────
# Use find-certificate (not find-identity -v) — a self-signed cert won't show
# up as a "valid" identity because it isn't in a trusted root chain, but it is
# perfectly usable for code signing.
if security find-certificate -c "$CERT_NAME" "$KEYCHAIN_PATH" &>/dev/null; then
    echo "Certificate '$CERT_NAME' already in keychain — skipping generation."
else
    TMPDIR=$(mktemp -d)
    trap 'rm -rf "$TMPDIR"' EXIT

    # Generate a self-signed cert with Code Signing EKU
    cat > "$TMPDIR/openssl.cnf" << EOF
[req]
distinguished_name = req_dn
x509_extensions    = v3_codesign
prompt             = no

[req_dn]
CN = $CERT_NAME

[v3_codesign]
basicConstraints       = CA:FALSE
keyUsage               = critical, digitalSignature
extendedKeyUsage       = codeSigning
subjectKeyIdentifier   = hash
EOF

    echo "==> Generating self-signed certificate..."
    openssl req \
        -newkey rsa:2048 -nodes -keyout "$TMPDIR/dev.key" \
        -x509 -days 3650 -out "$TMPDIR/dev.crt" \
        -config "$TMPDIR/openssl.cnf" \
        -extensions v3_codesign 2>/dev/null

    if ! openssl x509 -text -noout -in "$TMPDIR/dev.crt" | grep -q "Code Signing"; then
        echo "ERROR: generated certificate is missing the Code Signing EKU"
        exit 1
    fi

    # Bundle as PKCS#12.
    # OpenSSL 3 (common via Homebrew) defaults to AES-256, which macOS
    # 'security import' doesn't support — force legacy 3DES/RC2 format.
    PKCS12_LEGACY=""
    if openssl version 2>/dev/null | grep -q "^OpenSSL 3"; then
        PKCS12_LEGACY="-legacy"
    fi
    # shellcheck disable=SC2086
    openssl pkcs12 -export $PKCS12_LEGACY \
        -out "$TMPDIR/dev.p12" \
        -inkey "$TMPDIR/dev.key" \
        -in "$TMPDIR/dev.crt" \
        -passout "pass:$P12_PASS" 2>/dev/null

    echo "==> Importing certificate into keychain..."
    security import "$TMPDIR/dev.p12" \
        -k "$KEYCHAIN_PATH" \
        -T /usr/bin/codesign \
        -P "$P12_PASS"
fi

# ── Allow codesign to use the key without interactive prompts ─────────────────
# Always re-run — idempotent, and needed after a partial earlier run.
security set-key-partition-list \
    -S "apple-tool:,apple:,codesign:" \
    -s -k "$KEYCHAIN_PASS" \
    "$KEYCHAIN_PATH"

# ── Add the keychain to the user search list ──────────────────────────────────
# Prepend the dev keychain; preserves existing entries.
CURRENT_KEYCHAINS=$(security list-keychains -d user | tr -d '"' | xargs)
security list-keychains -d user -s "$KEYCHAIN_PATH" $CURRENT_KEYCHAINS

# ── Done ──────────────────────────────────────────────────────────────────────
echo ""
echo "Done!  Certificate '$CERT_NAME' is ready for code signing."
echo ""
echo "Add to your shell profile (.zshrc / .bashrc):"
echo "  export CODESIGN_IDENTITY=\"$CERT_NAME\""
echo ""
echo "Builds will use ad-hoc signing if CODESIGN_IDENTITY is not set."
