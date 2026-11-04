#!/bin/sh
# Packs GIFkit as an AppImage: one file that runs on most Linux systems.
#   build-appimage.sh <gifkit program> <appimagetool> <out.AppImage>
set -e
prog=$1 tool=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
dir=$tmp/GIFkit.AppDir
mkdir -p "$dir/usr/bin" "$dir/usr/share/applications" "$dir/usr/share/icons/hicolor/scalable/apps"
install -m 755 "$prog" "$dir/usr/bin/gifkit"
ln -s usr/bin/gifkit "$dir/AppRun"
# a PNG for the AppImage's own icon (it must be one), the SVG for desktops
cp "$here/../icon/gifkit-256.png" "$dir/gifkit.png"
ln -s gifkit.png "$dir/.DirIcon"
cp "$here/../icon/gifkit.svg" "$dir/usr/share/icons/hicolor/scalable/apps/gifkit.svg"
cat > "$dir/gifkit.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=GIFkit
GenericName=GIF Editor
Comment=Edit and optimise GIFs on your computer
Exec=gifkit %f
Icon=gifkit
Terminal=false
Categories=Graphics;2DGraphics;RasterGraphics;
MimeType=image/gif;image/webp;image/apng;image/png;video/mp4;video/webm;video/quicktime;video/x-matroska;
StartupWMClass=gifkit
DESKTOP
cp "$dir/gifkit.desktop" "$dir/usr/share/applications/gifkit.desktop"
# runs without FUSE (CI runners, containers)
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" --no-appstream "$dir" "$out"
rm -rf "$tmp"
