#!/bin/sh
# Installs GifForge for the current user (menu entry + icon). Run from this folder.
set -e
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications" "$HOME/.local/share/icons/hicolor/256x256/apps" "$HOME/.local/share/icons/hicolor/scalable/apps"
install -m 755 "$here/gifforge" "$HOME/.local/bin/gifforge"
install -m 644 "$here/gifforge.png" "$HOME/.local/share/icons/hicolor/256x256/apps/gifforge.png"
install -m 644 "$here/gifforge.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/gifforge.svg"
sed "s|^Exec=gifforge|Exec=$HOME/.local/bin/gifforge|" "$here/gifforge.desktop" > "$HOME/.local/share/applications/gifforge.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
echo "Installed. GifForge is in your app menu."
