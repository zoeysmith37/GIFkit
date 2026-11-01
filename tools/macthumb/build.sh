#!/bin/sh
# Builds GIFkitThumbnail.appex (both Mac CPUs) into the app given:
#   tools/macthumb/build.sh dist/GIFkit.app 1.5.0
set -e
here=$(cd "$(dirname "$0")" && pwd)
app=$1
version=$2
ext="$app/Contents/PlugIns/GIFkitThumbnail.appex"
mkdir -p "$ext/Contents/MacOS"
for arch in arm64 x86_64; do
    swiftc -O -module-name GIFkitThumbnail -target "$arch-apple-macos11" -parse-as-library \
        -application-extension -Xlinker -e -Xlinker _NSExtensionMain \
        -framework AppKit -framework QuickLookThumbnailing \
        "$here/ThumbnailProvider.swift" -o "$here/GIFkitThumbnail-$arch"
done
lipo -create -output "$ext/Contents/MacOS/GIFkitThumbnail" "$here/GIFkitThumbnail-arm64" "$here/GIFkitThumbnail-x86_64"
rm -f "$here/GIFkitThumbnail-arm64" "$here/GIFkitThumbnail-x86_64"
sed "s/VERSION/$version/g" "$here/Info.plist" > "$ext/Contents/Info.plist"
codesign --force --sign - --entitlements "$here/thumbnail.entitlements" "$ext"
