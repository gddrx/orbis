#!/bin/bash
# Checks a release AppImage on other distributions, each in a podman
# container: unpacked (no FUSE there), with the libraries a desktop has
# installed, every ELF inside must find all its libraries, and orbis,
# ffmpeg, Node.js and yt-dlp must start. Also checks that nothing inside
# needs a glibc newer than 2.34.
#
#   scripts/release/check-distros.sh target/appimage/orbis-<version>-x86_64.AppImage
#
# Results: target/release-build/check/<distro>.log and a summary on stdout.
set -euo pipefail
cd "$(dirname "$0")/../.."
APPIMAGE=$(realpath "$1")
CHECK=target/release-build/check
rm -rf "$CHECK" && mkdir -p "$CHECK"
(cd "$CHECK" && "$APPIMAGE" --appimage-extract > /dev/null)
ROOT=$CHECK/squashfs-root

# The newest glibc symbol any ELF inside asks for.
newest=$(find "$ROOT" -type f -exec sh -c 'head -c4 "$1" | grep -q ELF && objdump -T "$1" 2>/dev/null' _ {} \; \
    | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
echo "newest glibc asked for: $newest"

# What a desktop of each distribution has: graphics, fonts, Wayland and X11
# clients, sound, D-Bus, what Qt WebEngine loads, and the libraries the
# AppImage leaves to the host (EXCLUDE in build-appimage.sh).
declare -A IMAGES=(
    [ubuntu-22.04]=docker.io/library/ubuntu:22.04
    [ubuntu-24.04]=docker.io/library/ubuntu:24.04
    [debian-12]=docker.io/library/debian:12
    [debian-13]=docker.io/library/debian:13
    [fedora]=registry.fedoraproject.org/fedora:latest
)
APT="libgl1 libegl1 libopengl0 libfontconfig1 libfreetype6 libxkbcommon0 libxkbcommon-x11-0 \
    libwayland-client0 libwayland-cursor0 libwayland-egl1 libx11-6 libx11-xcb1 libxcb1 \
    libxcb-cursor0 libxcb-icccm4 libxcb-keysyms1 libxcb-shape0 libxcb-xkb1 libxcb-randr0 \
    libxcomposite1 libxdamage1 libxrandr2 libxtst6 libxshmfence1 libgbm1 libdrm2 \
    libnss3 libdbus-1-3 libglib2.0-0 libpulse0 libpipewire-0.3-0 libasound2 libjack0 \
    libharfbuzz0b libfribidi0 libexpat1 libpng16-16 libbrotli1 libbz2-1.0 libffi8 libgraphite2-3 \
    libpcre2-8-0 libxau6 libzstd1 liblzma5 liblz4-1 libxml2 libsystemd0 libcap2 libgcrypt20 \
    libselinux1 libudev1 libmount1 libblkid1 libsndfile1 libgssapi-krb5-2 libkrb5-3 libk5crypto3 libkeyutils1 binutils file"
DNF="mesa-libGL mesa-libEGL libglvnd-opengl fontconfig freetype libxkbcommon libxkbcommon-x11 \
    libwayland-client libwayland-cursor libwayland-egl libX11 libX11-xcb libxcb xcb-util-cursor \
    xcb-util-wm xcb-util-keysyms libXcomposite libXdamage libXrandr libXtst libxshmfence \
    mesa-libgbm libdrm nss dbus-libs glib2 pulseaudio-libs pipewire-libs alsa-lib \
    pipewire-jack-audio-connection-kit-libs harfbuzz fribidi expat libpng brotli bzip2-libs libffi \
    graphite2 pcre2 libXau libzstd xz-libs lz4-libs libxml2 systemd-libs libmount libblkid libcap libgcrypt libselinux \
    libsndfile krb5-libs keyutils-libs binutils file"

for distro in "${!IMAGES[@]}"; do
    image=${IMAGES[$distro]}
    case $distro in
        fedora) install="dnf -y -q install $DNF > /dev/null" ;;
        ubuntu-24.04|debian-13) install="apt-get -qq update && DEBIAN_FRONTEND=noninteractive apt-get -qq install -y ${APT/libasound2 /libasound2t64 } > /dev/null" ;;
        *) install="apt-get -qq update && DEBIAN_FRONTEND=noninteractive apt-get -qq install -y $APT > /dev/null" ;;
    esac
    podman run --rm --security-opt label=disable -v "$PWD/$ROOT:/app:ro" "$image" bash -c "
        $install
        echo \"glibc: \$(ldd --version | head -1)\"
        missing=\$(find /app -type f -exec sh -c 'head -c4 \"\$1\" | grep -q ELF && ldd \"\$1\" 2>/dev/null | grep \"not found\" | sed \"s|^|\$1: |\"' _ {} \\; | sort -u)
        if [ -n \"\$missing\" ]; then echo \"missing libraries:\"; echo \"\$missing\"; else echo 'libraries: all found'; fi
        export PATH=/app/usr/bin:/app/opt/yt-dlp:\$PATH LD_LIBRARY_PATH=
        echo \"orbis: \$(/app/usr/bin/orbis --version 2>&1)\"
        echo \"ffmpeg: \$(ffmpeg -hide_banner -version 2>&1 | head -1)\"
        echo \"node: \$(node --version 2>&1)\"
        echo \"yt-dlp: \$(yt-dlp --version 2>&1)\"
    " > "$CHECK/$distro.log" 2>&1 || echo "container failed" >> "$CHECK/$distro.log"
    echo "== $distro"
    cat "$CHECK/$distro.log"
done
