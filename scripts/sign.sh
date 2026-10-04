#!/bin/sh
# Re-sign after every release build, refresh the app bundle, restart the app.
# The app-bundle context is REQUIRED: launchd-spawned processes cannot create
# CGEventTaps on macOS 26 regardless of TCC grants (responsibility chain).
set -e
security unlock-keychain -p "0320" "/Users/uzqw/Library/Keychains/login.keychain-db" 2>/dev/null || true
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/release/stasis"
codesign -fs stasis-dev --identifier com.uzqw.stasis --timestamp=none "$BIN"
APP="$HOME/Applications/StasisTray.app"
cp "$BIN" "$APP/Contents/MacOS/stasis"
codesign -fs stasis-dev --identifier com.uzqw.stasis --timestamp=none "$APP"
osascript -e "tell application \"System Events\" to (name of processes) contains \"stasis\"" 2>/dev/null | grep -q true   && pkill -f "StasisTray.app/Contents/MacOS/stasis" || true
sleep 2
open "$APP"
