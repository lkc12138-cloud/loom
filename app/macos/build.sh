#!/bin/sh
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
if command -v cargo >/dev/null 2>&1; then
    cd "$ROOT"
    exec cargo run --locked -- app --build-only --rebuild "$@"
fi

APP_DIR="${1:-$HOME/Applications}"
BUNDLE="$APP_DIR/Loom.app"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$ROOT/app/macos/Info.plist" "$BUNDLE/Contents/Info.plist"
/usr/bin/swiftc -O -swift-version 5 -framework Cocoa -framework WebKit \
    "$ROOT/app/macos/LoomApp.swift" -o "$BUNDLE/Contents/MacOS/Loom"
chmod 755 "$BUNDLE/Contents/MacOS/Loom"
if command -v codesign >/dev/null 2>&1; then
    /usr/bin/codesign --force --deep --sign - "$BUNDLE" >/dev/null 2>&1 || true
fi
echo "已生成桌面应用：$BUNDLE"
