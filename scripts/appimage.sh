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

source scripts/appimage-tools.sh

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
