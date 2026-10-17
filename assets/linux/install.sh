#!/bin/sh
# Adds GifForge to your app menu (current user only, no root needed).
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove it again
set -e
here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin/gifforge"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
desktop="$apps/gifforge.desktop"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$icons" 2>/dev/null || true
    # KDE Plasma reads its own cache of menu entries
    for k in kbuildsycoca6 kbuildsycoca5; do
        if command -v "$k" >/dev/null 2>&1; then "$k" >/dev/null 2>&1 || true; break; fi
    done
}

if [ "$1" = "--uninstall" ]; then
    rm -f "$bin" "$desktop" "$icons/256x256/apps/gifforge.png" "$icons/scalable/apps/gifforge.svg"
    refresh
    echo "GifForge removed."
    exit 0
fi

[ -f "$here/gifforge" ] || { echo "Run this from the folder that contains the gifforge program." >&2; exit 1; }
mkdir -p "$(dirname "$bin")" "$apps" "$icons/256x256/apps" "$icons/scalable/apps"
install -m 755 "$here/gifforge" "$bin"
install -m 644 "$here/gifforge.png" "$icons/256x256/apps/gifforge.png"
install -m 644 "$here/gifforge.svg" "$icons/scalable/apps/gifforge.svg"

# absolute paths so the entry works even if ~/.local/bin isn't on PATH
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=GifForge
GenericName=GIF Editor
Comment=Edit and optimise GIFs on your computer
Exec="$bin" %f
TryExec=$bin
Icon=$icons/256x256/apps/gifforge.png
Terminal=false
Categories=Graphics;2DGraphics;RasterGraphics;
MimeType=image/gif;image/webp;image/apng;image/png;video/mp4;video/webm;video/quicktime;video/x-matroska;
StartupWMClass=gifforge
DESKTOP
chmod 644 "$desktop"
refresh
echo "Installed. GifForge is now in your app menu."
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) echo "Tip: add ~/.local/bin to PATH to start it from a terminal with 'gifforge'." ;;
esac
