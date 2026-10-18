#!/bin/sh
# Adds GIFkit to your app menu (current user only, no root needed).
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove it again
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin/gifkit"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
desktop="$apps/gifkit.desktop"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$icons" 2>/dev/null || true
    # KDE Plasma reads its own cache of menu entries
    for k in kbuildsycoca6 kbuildsycoca5; do
        if command -v "$k" >/dev/null 2>&1; then "$k" >/dev/null 2>&1 || true; break; fi
    done
}

# GIFkit used to be called GifForge: remove the old menu entry, icon and program
old_apps="$apps/gifforge.desktop"
cleanup_old() {
    rm -f "$HOME/.local/bin/gifforge" "$old_apps" "$icons/256x256/apps/gifforge.png" "$icons/scalable/apps/gifforge.svg"
}

if [ "$1" = "--uninstall" ]; then
    cleanup_old
    rm -f "$bin" "$desktop" "$icons/256x256/apps/gifkit.png" "$icons/scalable/apps/gifkit.svg"
    refresh
    echo "GIFkit removed."
    exit 0
fi

[ -f "$here/gifkit" ] || { echo "Run this from the folder that contains the gifkit program." >&2; exit 1; }
mkdir -p "$(dirname "$bin")" "$apps" "$icons/scalable/apps"
install -m 755 "$here/gifkit" "$bin"
# one scalable SVG: KDE recolours it to match your colour scheme and accent
rm -f "$icons/256x256/apps/gifkit.png"
cleanup_old
install -m 644 "$here/gifkit.svg" "$icons/scalable/apps/gifkit.svg"

# absolute paths so the entry works even if ~/.local/bin isn't on PATH
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=GIFkit
GenericName=GIF Editor
Comment=Edit and optimise GIFs on your computer
Exec="$bin" %f
TryExec=$bin
Icon=gifkit
Terminal=false
Categories=Graphics;2DGraphics;RasterGraphics;
MimeType=image/gif;image/webp;image/apng;image/png;video/mp4;video/webm;video/quicktime;video/x-matroska;
StartupWMClass=gifkit
DESKTOP
chmod 644 "$desktop"
refresh
echo "Installed. GIFkit is now in your app menu."
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) echo "Tip: add ~/.local/bin to PATH to start it from a terminal with 'gifkit'." ;;
esac
