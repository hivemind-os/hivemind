#!/bin/bash
# Uninstall HiveMind OS from macOS.
#
# Usage: sudo scripts/macos/uninstall.sh
#
# This removes:
#   - The LaunchAgent (stops auto-restart)
#   - The daemon process
#   - /Applications/HiveMind OS.app
#   - /usr/local/bin/hive-daemon
#   - /usr/local/bin/hive-cli
#   - /usr/local/bin/hive-runtime-worker
#   - The installer receipt
#
# Does NOT remove user data in ~/.hivemind (secrets, config, logs).
# To remove user data as well, pass --purge.

set -euo pipefail

PURGE=false
if [ "${1:-}" = "--purge" ]; then
    PURGE=true
fi

LABEL="com.hivemind.daemon"
IDENTIFIER="com.hivemind.desktop"

echo "==> Uninstalling HiveMind OS..."

# 1. Stop and unload the LaunchAgent for every logged-in user.
#    When run via sudo, $HOME may point to /var/root, so we also check
#    the SUDO_USER's home directory.
for USER_HOME in "$HOME" "${SUDO_USER:+$(eval echo "~$SUDO_USER")}" ; do
    [ -z "$USER_HOME" ] && continue
    PLIST="${USER_HOME}/Library/LaunchAgents/${LABEL}.plist"
    if [ -f "$PLIST" ]; then
        echo "    Unloading LaunchAgent: $PLIST"
        # Try to get the UID of the plist owner for the correct domain
        PLIST_OWNER_UID=$(stat -f %u "$PLIST" 2>/dev/null || echo "")
        if [ -n "$PLIST_OWNER_UID" ]; then
            launchctl bootout "gui/${PLIST_OWNER_UID}" "$PLIST" 2>/dev/null || true
        fi
        rm -f "$PLIST"
    fi
done

# 2. Kill any lingering daemon processes
echo "    Stopping daemon processes..."
killall hive-daemon 2>/dev/null || true
sleep 1

# 3. Remove installed files
echo "    Removing application and binaries..."
rm -rf "/Applications/HiveMind OS.app"
rm -f /usr/local/bin/hive-daemon
rm -f /usr/local/bin/hive-cli
rm -f /usr/local/bin/hive-runtime-worker

# 4. Forget the installer receipt so re-installs work cleanly
echo "    Removing installer receipt..."
pkgutil --forget "$IDENTIFIER" 2>/dev/null || true

# 5. Optionally remove user data
if [ "$PURGE" = true ]; then
    for USER_HOME in "$HOME" "${SUDO_USER:+$(eval echo "~$SUDO_USER")}" ; do
        [ -z "$USER_HOME" ] && continue
        if [ -d "${USER_HOME}/.hivemind" ]; then
            echo "    Removing user data: ${USER_HOME}/.hivemind"
            rm -rf "${USER_HOME}/.hivemind"
        fi
    done
fi

echo ""
echo "==> HiveMind OS has been uninstalled."
if [ "$PURGE" = false ]; then
    echo "    User data in ~/.hivemind was preserved."
    echo "    Run with --purge to also remove user data."
fi
