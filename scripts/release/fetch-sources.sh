#!/bin/bash
# Downloads the sources of the release AppImage (versions.sh) into
# target/release-build/sources/, once, and checks each against its SHA-256
# in versions.sh. An empty hash there prints the one computed, to be
# written in after checking it elsewhere: FFmpeg's archive against its
# signature (ffmpeg-<version>.tar.xz.asc), wayland-protocols' against its
# .sig, Node.js's against its signed SHASUMS256.txt, KDE's against
# download.kde.org's own .sha256.
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/release/versions.sh
SOURCES=target/release-build/sources
mkdir -p "$SOURCES"
CURL=(curl -fL --retry 3 --connect-timeout 20 --max-time 900)
missing=0

# fetch NAME URL SHA256
fetch() {
    local name=$1 url=$2 sha=$3 file="$SOURCES/$(basename "$2")"
    if [ ! -s "$file" ]; then
        "${CURL[@]}" -sS -o "$file.part" "$url"
        mv "$file.part" "$file"
    fi
    local got
    got=$(sha256sum "$file" | cut -d' ' -f1)
    if [ -z "$sha" ]; then
        echo "$name: no hash in versions.sh; computed $got" >&2
        missing=1
    elif [ "$got" != "$sha" ]; then
        echo "$name: SHA-256 mismatch: $got, expected $sha" >&2
        exit 1
    fi
}

fetch ECM "$ECM_URL" "$ECM_SHA256"
fetch KWINDOWSYSTEM "$KWINDOWSYSTEM_URL" "$KWINDOWSYSTEM_SHA256"
fetch WAYLAND_PROTOCOLS "$WAYLAND_PROTOCOLS_URL" "$WAYLAND_PROTOCOLS_SHA256"
fetch PLASMA_WAYLAND_PROTOCOLS "$PLASMA_WAYLAND_PROTOCOLS_URL" "$PLASMA_WAYLAND_PROTOCOLS_SHA256"
fetch FFMPEG "$FFMPEG_URL" "$FFMPEG_SHA256"
fetch MPV "$MPV_URL" "$MPV_SHA256"
fetch NODE "$NODE_URL" "$NODE_SHA256"

placebo=$SOURCES/libplacebo-$LIBPLACEBO_VERSION
if [ ! -d "$placebo" ]; then
    git clone -q --depth 1 --branch "v$LIBPLACEBO_VERSION" \
        --recurse-submodules --shallow-submodules "$LIBPLACEBO_GIT" "$placebo.part"
    mv "$placebo.part" "$placebo"
fi
got=$(git -C "$placebo" rev-parse HEAD)
if [ -z "$LIBPLACEBO_COMMIT" ]; then
    echo "LIBPLACEBO: no commit in versions.sh; v$LIBPLACEBO_VERSION is $got" >&2
    missing=1
elif [ "$got" != "$LIBPLACEBO_COMMIT" ]; then
    echo "LIBPLACEBO: v$LIBPLACEBO_VERSION is $got, expected $LIBPLACEBO_COMMIT" >&2
    exit 1
fi

exit $missing
