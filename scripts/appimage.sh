#!/bin/bash
# Builds target/appimage/orbis-<version>-local-x86_64.AppImage, the version
# being the workspace's in Cargo.toml: orbis and orbis-login (the
# Google sign-in window, Qt WebEngine) with Qt, its QML modules and
# plugins, libmpv with FFmpeg's libraries, and the programs orbis runs —
# yt-dlp (its standalone build), Node.js and ffmpeg — so that nothing has
# to be installed besides.
#
# The tools come from GitHub on the first run, into target/appimage-tools/:
# linuxdeploy and its Qt plugin (the "continuous" builds), the AppImage
# runtime (checked against the SHA-256 GitHub gives for it) and the latest
# yt-dlp (checked against its release's SHA2-256SUMS). Delete that folder to
# take newer ones (yt-dlp needs updating whenever YouTube changes).
#
# What orbis never loads is dropped before the image is made (see
# prune_appdir.py): other Qt Quick Controls styles, image formats other than
# JPEG, WebP and SVG, and copies of libraries under their full version.
#
# For this machine only, hence "local" in the name: it takes this system's
# FFmpeg, Packman's, built with --enable-nonfree, which may not be passed on
# to anyone. Do not upload it.
#
# Not inside, as with every AppImage: glibc, GL and the fonts of the host.
# libfdk-aac neither: this FFmpeg (Packman's) opens it only when the host
# has it; otherwise M4A is encoded by FFmpeg's own AAC. The AppImage runs
# where glibc is at least as new as on the system that built it.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
TOOLS=$ROOT/target/appimage-tools
WORK=$ROOT/target/appimage
APPDIR=$WORK/AppDir
mkdir -p "$TOOLS" "$WORK"

# A dropped connection fails the build instead of hanging it.
CURL=(curl -fL --retry 3 --connect-timeout 20 --max-time 600)

# Downloads `url` to the tools folder as `name`, once.
fetch() {
    local url=$1 name=$2
    if [ ! -s "$TOOLS/$name" ]; then
        "${CURL[@]}" -o "$TOOLS/$name.part" "$url"
        mv "$TOOLS/$name.part" "$TOOLS/$name"
        chmod +x "$TOOLS/$name"
    fi
}
fetch https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
    linuxdeploy-x86_64.AppImage
fetch https://github.com/linuxdeploy/linuxdeploy-plugin-qt/releases/download/continuous/linuxdeploy-plugin-qt-x86_64.AppImage \
    linuxdeploy-plugin-qt-x86_64.AppImage
# The AppImage's runtime, the part that mounts it. appimagetool would fetch
# it itself, with no timeout: it hung for an hour once GitHub dropped it.
if [ ! -s "$TOOLS/runtime-x86_64" ]; then
    api=https://api.github.com/repos/AppImage/type2-runtime/releases/tags/continuous
    sum=$("${CURL[@]}" -sS "$api" | python3 -I -c '
import json, sys
for asset in json.load(sys.stdin)["assets"]:
    if asset["name"] == "runtime-x86_64":
        print(asset["digest"].removeprefix("sha256:"))
')
    "${CURL[@]}" -o "$TOOLS/runtime-x86_64.part" \
        https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64
    echo "$sum  $TOOLS/runtime-x86_64.part" | sha256sum -c --quiet -
    mv "$TOOLS/runtime-x86_64.part" "$TOOLS/runtime-x86_64"
fi
if [ ! -s "$TOOLS/yt-dlp_linux" ]; then
    release=https://github.com/yt-dlp/yt-dlp/releases/latest/download
    "${CURL[@]}" -o "$TOOLS/yt-dlp_linux.part" "$release/yt-dlp_linux"
    "${CURL[@]}" -sS -o "$TOOLS/SHA2-256SUMS" "$release/SHA2-256SUMS"
    sum=$(awk '$2 == "yt-dlp_linux" { print $1 }' "$TOOLS/SHA2-256SUMS")
    echo "$sum  $TOOLS/yt-dlp_linux.part" | sha256sum -c --quiet -
    mv "$TOOLS/yt-dlp_linux.part" "$TOOLS/yt-dlp_linux"
    chmod +x "$TOOLS/yt-dlp_linux"
fi

cargo build --release -p orbis -p orbis-login
VERSION=$(cargo metadata --format-version 1 --no-deps | python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "orbis"))
')

rm -rf "$APPDIR"
install -Dm755 target/release/orbis "$APPDIR/usr/bin/orbis"
install -Dm755 target/release/orbis-login "$APPDIR/usr/bin/orbis-login"
install -Dm755 "$(readlink -f "$(command -v node)")" "$APPDIR/usr/bin/node"
install -Dm755 "$(command -v ffmpeg)" "$APPDIR/usr/bin/ffmpeg"
# yt-dlp's standalone build carries its own Python: outside usr/, where
# linuxdeploy would rewrite it.
install -Dm755 "$TOOLS/yt-dlp_linux" "$APPDIR/opt/yt-dlp/yt-dlp"
# KWindowSystem's Wayland plugin: the blur behind the window and the focus
# from the tray. Qt does not know orbis needs it.
qt_plugins=$(qmake6 -query QT_INSTALL_PLUGINS)
install -Dm755 "$qt_plugins/kf6/kwindowsystem/KF6WindowSystemKWaylandPlugin.so" \
    "$APPDIR/usr/plugins/kf6/kwindowsystem/KF6WindowSystemKWaylandPlugin.so"
# The AppImage's own icon (.DirIcon): appimagetool makes it only from a PNG
# at the top (orbis.svg drawn at 256 px); the menu takes the SVG.
install -Dm644 data/orbis.png "$APPDIR/orbis.png"

export QMAKE=qmake6
# The QML files say which QML modules to bring (they are compiled in).
export QML_SOURCES_PATHS="$ROOT/crates/app/src/qml:$ROOT/crates/login/src/qml"
export EXTRA_PLATFORM_PLUGINS="libqwayland.so"
export EXTRA_QT_MODULES="waylandclient"
export OUTPUT="$WORK/orbis-$VERSION-local-x86_64.AppImage"
# The version in the AppImage's desktop entry (X-AppImage-Version).
export LINUXDEPLOY_OUTPUT_VERSION=$VERSION
export LDAI_RUNTIME_FILE="$TOOLS/runtime-x86_64"
# The tools are AppImages themselves: extracted, they need no FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1
"$TOOLS/linuxdeploy-x86_64.AppImage" --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/orbis" \
    --executable "$APPDIR/usr/bin/orbis-login" \
    --executable "$APPDIR/usr/bin/node" \
    --executable "$APPDIR/usr/bin/ffmpeg" \
    --deploy-deps-only "$APPDIR/usr/plugins/kf6/kwindowsystem" \
    --desktop-file data/orbis.desktop \
    --icon-file data/orbis.svg \
    --custom-apprun data/AppRun \
    --plugin qt
python3 -I scripts/prune_appdir.py "$APPDIR"
# The image of the pruned AppDir; without the Qt plugin nothing comes back.
"$TOOLS/linuxdeploy-x86_64.AppImage" --appdir "$APPDIR" --output appimage
ls -lh "$OUTPUT"
