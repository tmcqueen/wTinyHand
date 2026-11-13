#!/bin/sh
# Builds TinyHand and installs it for the current user (no sudo):
#   ~/.local/bin/tinyhand, ~/.local/share/tinyhand/assets, and a desktop entry that
#   adds the app to the launcher and registers tinyhand:// links.
set -eu
cd "$(dirname "$0")"
cargo build --release

BIN="$HOME/.local/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
mkdir -p "$BIN" "$DATA/tinyhand" "$DATA/applications" "$DATA/icons/hicolor/512x512/apps"
install -m755 target/release/tinyhand "$BIN/tinyhand"
rm -rf "$DATA/tinyhand/assets"
cp -r assets "$DATA/tinyhand/assets"
cp assets/icon.png "$DATA/icons/hicolor/512x512/apps/tinyhand.png"

cat > "$DATA/applications/tinyhand.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=TinyHand
Comment=A tiny hand on a stick that points at things on your screen
Exec=$BIN/tinyhand %u
Icon=tinyhand
Categories=Utility;
MimeType=x-scheme-handler/tinyhand;
StartupNotify=false
Actions=Toggle;Clear;

[Desktop Action Toggle]
Name=Toggle pointer
Exec=$BIN/tinyhand toggle
X-KDE-Shortcuts=Ctrl+Alt+H

[Desktop Action Clear]
Name=Clear drawing
Exec=$BIN/tinyhand clear
X-KDE-Shortcuts=Ctrl+Alt+X
DESKTOP
update-desktop-database "$DATA/applications" 2>/dev/null || true
xdg-mime default tinyhand.desktop x-scheme-handler/tinyhand 2>/dev/null || true

echo "Installed. Start it with: tinyhand"
echo "Bind a global shortcut (e.g. Ctrl+Alt+H) to 'tinyhand toggle' in your desktop's shortcut settings."
