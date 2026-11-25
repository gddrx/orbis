#!/bin/bash
# Runs inside the release container after build-deps.sh (appimage-release.sh
# starts both): builds orbis and orbis-login against Qt 6.12 and the
# libraries in /opt/deps, gathers them with yt-dlp, Node.js and FFmpeg's
# ffmpeg into an AppDir, writes the licenses of everything inside, and makes
# the AppImage.
#
#   /src      the repository (target/ is written)
#   /sources  fetch-sources.sh's archives, read only
#   /opt/deps build-deps.sh's result
#   /tools    target/appimage-tools: linuxdeploy, its Qt plugin, the
#             AppImage runtime and yt-dlp_linux
set -euo pipefail
source /scripts/versions.sh
cd /src
export CARGO_TARGET_DIR=/src/target/release-container
VERSION=$(cargo metadata --offline --format-version 1 --no-deps | python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "orbis"))
')
WORK=/src/target/release-build/appimage
APPDIR=$WORK/AppDir
OUTPUT=/src/target/appimage/orbis-$VERSION-x86_64.AppImage
mkdir -p "$WORK" /src/target/appimage

# Qt for qtbridge and the plugins, libmpv and KWindowSystem from /opt/deps
# for the linker. The crates come from target/release-build/vendor
# (appimage-release.sh fills it): no network inside.
export QMAKE=/opt/qt/$QT_VERSION/gcc_64/bin/qmake
export LIBRARY_PATH=/opt/deps/lib
cargo build --release --offline --locked \
    --config "source.crates-io.replace-with='vendored'" \
    --config "source.vendored.directory='/src/target/release-build/vendor'" \
    -p orbis -p orbis-login

rm -rf "$APPDIR"
install -Dm755 "$CARGO_TARGET_DIR/release/orbis" "$APPDIR/usr/bin/orbis"
install -Dm755 "$CARGO_TARGET_DIR/release/orbis-login" "$APPDIR/usr/bin/orbis-login"
install -Dm755 /opt/deps/bin/ffmpeg "$APPDIR/usr/bin/ffmpeg"
node_dir=$WORK/node
rm -rf "$node_dir" && mkdir -p "$node_dir"
tar -xJf "/sources/node-v$NODE_VERSION-linux-x64.tar.xz" -C "$node_dir" --strip-components=1
install -Dm755 "$node_dir/bin/node" "$APPDIR/usr/bin/node"
# yt-dlp's standalone build carries its own Python: outside usr/, where
# linuxdeploy would rewrite it.
install -Dm755 /tools/yt-dlp_linux "$APPDIR/opt/yt-dlp/yt-dlp"
install -Dm755 /opt/deps/lib/plugins/kf6/kwindowsystem/KF6WindowSystemKWaylandPlugin.so \
    "$APPDIR/usr/plugins/kf6/kwindowsystem/KF6WindowSystemKWaylandPlugin.so"
install -Dm644 data/orbis.png "$APPDIR/orbis.png"

export QML_SOURCES_PATHS="/src/crates/app/src/qml:/src/crates/login/src/qml"
export EXTRA_PLATFORM_PLUGINS="libqwayland.so"
export EXTRA_QT_MODULES="waylandclient"
export OUTPUT
export LINUXDEPLOY_OUTPUT_VERSION=$VERSION
export LDAI_RUNTIME_FILE=/tools/runtime-x86_64
export APPIMAGE_EXTRACT_AND_RUN=1
/tools/linuxdeploy-x86_64.AppImage --appdir "$APPDIR" \
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
python3 -I /scripts/licenses.py "$APPDIR" "$WORK/node" "$VERSION"
/tools/linuxdeploy-x86_64.AppImage --appdir "$APPDIR" --output appimage
ls -lh "$OUTPUT"
