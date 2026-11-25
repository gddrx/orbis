#!/bin/bash
# Builds target/appimage/orbis-<version>-x86_64.AppImage, the AppImage for
# releases, in a podman container on AlmaLinux 9 (scripts/release/):
#
# - it runs where glibc is 2.34 or newer (Ubuntu 22.04, Debian 12, Fedora,
#   RHEL 9 and the like), as Qt's own Linux binaries do;
# - Qt 6.12 comes from qt.io; KWindowSystem, libplacebo, FFmpeg and libmpv
#   are built from source (build-deps.sh): FFmpeg and libmpv under the LGPL,
#   audio only, nothing non-free — everything inside may be passed on;
# - the licenses of all it carries are in it (licenses.py), and
#   orbis-<version>-THIRD-PARTY.md beside it lists them.
#
# The sources are downloaded once into target/release-build/sources/ and
# checked against the hashes in scripts/release/versions.sh; the Rust
# crates are vendored into target/release-build/vendor/, so the container
# builds without network. The first run takes a while (the container, then
# the libraries); later ones only rebuild orbis.
#
# For this machine, scripts/appimage.sh is quicker; its AppImage is not for
# passing on.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
TOOLS=$ROOT/target/appimage-tools
BUILD=$ROOT/target/release-build
mkdir -p "$BUILD/deps" "$ROOT/target/appimage"

source scripts/appimage-tools.sh
scripts/release/fetch-sources.sh
podman build -q -t orbis-release-base -f scripts/release/Containerfile scripts/release > /dev/null
cargo vendor --locked --versioned-dirs "$BUILD/vendor" > /dev/null

# Root in a rootless container is this user: what it writes stays ours.
podman run --rm --security-opt label=disable \
    -v "$ROOT:/src" \
    -v "$BUILD/sources:/sources:ro" \
    -v "$BUILD/deps:/opt/deps" \
    -v "$ROOT/scripts/release:/scripts:ro" \
    -v "$TOOLS:/tools:ro" \
    orbis-release-base bash -c 'bash /scripts/build-deps.sh && bash /scripts/build-appimage.sh'

VERSION=$(cargo metadata --format-version 1 --no-deps | python3 -I -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "orbis"))
')
cd target/appimage
sha256sum "orbis-$VERSION-x86_64.AppImage" > "orbis-$VERSION-x86_64.AppImage.sha256"
cat "orbis-$VERSION-x86_64.AppImage.sha256"
