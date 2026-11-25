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

# Libraries every desktop has, which the host's own libraries also load —
# freetype, harfbuzz, Mesa, PipeWire and systemd pull them in. Copies from
# AlmaLinux next to the host's newer ones break: the host's libbrotlidec
# meeting our libbrotlicommon stopped orbis on Tumbleweed. They stay out
# and come from the host, with the PulseAudio client and Kerberos, which
# bring the same ones along.
EXCLUDE=(
    'libbrotli*' 'libbz2.so*' 'libffi.so*' 'libpng16.so*' 'libgraphite2.so*' 'libXau.so*'
    'libglib-2.0.so*' 'libgobject-2.0.so*' 'libgio-2.0.so*' 'libgmodule-2.0.so*' 'libgthread-2.0.so*'
    'libpcre2-*.so*' 'libselinux.so*' 'libzstd.so*' 'liblzma.so*' 'liblz4.so*' 'libxml2.so*'
    'libsystemd.so*' 'libudev.so*' 'libmount.so*' 'libblkid.so*' 'libcap.so*' 'libgcrypt.so*' 'libdbus-1.so*'
    'libpulse*.so*' 'libsndfile.so*' 'libFLAC.so*' 'libvorbis*.so*' 'libogg.so*' 'libgsm.so*' 'libasyncns.so*'
    'libgssapi_krb5.so*' 'libkrb5*.so*' 'libk5crypto.so*' 'libkeyutils.so*'
)
EXCLUDE_ARGS=()
for pattern in "${EXCLUDE[@]}"; do EXCLUDE_ARGS+=(--exclude-library "$pattern"); done
export LINUXDEPLOY_EXCLUDED_LIBRARIES=$(IFS=';'; echo "${EXCLUDE[*]}")

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
    "${EXCLUDE_ARGS[@]}" \
    --plugin qt
python3 -I scripts/prune_appdir.py "$APPDIR"
# What the Qt plugin brought of them anyway.
for pattern in "${EXCLUDE[@]}"; do
    find "$APPDIR/usr/lib" -maxdepth 1 -name "$pattern" -print -delete
done
python3 -I /scripts/licenses.py "$APPDIR" "$WORK/node" "$VERSION"
/tools/linuxdeploy-x86_64.AppImage --appdir "$APPDIR" "${EXCLUDE_ARGS[@]}" --output appimage
for pattern in "${EXCLUDE[@]}"; do
    if [ -n "$(find "$APPDIR/usr/lib" -maxdepth 1 -name "$pattern")" ]; then
        echo "excluded library $pattern is in the AppImage" >&2
        exit 1
    fi
done
ls -lh "$OUTPUT"
