# What the release AppImage is built from: one place for the versions and
# the SHA-256 of every source archive (see fetch-sources.sh). Sourced by the
# release scripts.

QT_VERSION=6.12.0

ECM_VERSION=6.32.0
ECM_URL=https://download.kde.org/stable/frameworks/6.32/extra-cmake-modules-$ECM_VERSION.tar.xz
ECM_SHA256=305f4e2b82eb073be6ac3e50ace85578b3c0c409be262ccc51d445449e3d9312

KWINDOWSYSTEM_VERSION=6.32.0
KWINDOWSYSTEM_URL=https://download.kde.org/stable/frameworks/6.32/kwindowsystem-$KWINDOWSYSTEM_VERSION.tar.xz
KWINDOWSYSTEM_SHA256=43a32eb940c740f5acc917ce24c4939ba08b9e08c26091dbb5577ad0ef74f3a0

# Newer than AlmaLinux's 1.37: KWindowSystem wants 1.46 or later.
WAYLAND_PROTOCOLS_VERSION=1.50
WAYLAND_PROTOCOLS_URL=https://gitlab.freedesktop.org/wayland/wayland-protocols/-/releases/$WAYLAND_PROTOCOLS_VERSION/downloads/wayland-protocols-$WAYLAND_PROTOCOLS_VERSION.tar.xz
WAYLAND_PROTOCOLS_SHA256=4460c20cfcf68b994a5ee206ed15fc19ad312f16c96075186e83b0e3f4aa48a5

PLASMA_WAYLAND_PROTOCOLS_VERSION=1.23.0
PLASMA_WAYLAND_PROTOCOLS_URL=https://download.kde.org/stable/plasma-wayland-protocols/plasma-wayland-protocols-$PLASMA_WAYLAND_PROTOCOLS_VERSION.tar.xz
PLASMA_WAYLAND_PROTOCOLS_SHA256=16c5ad917bde2ed795942dacba76654819ddc6a1566842325ef34e0b553ef138

FFMPEG_VERSION=8.1.3
FFMPEG_URL=https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz
FFMPEG_SHA256=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3

MPV_VERSION=0.42.0
MPV_URL=https://github.com/mpv-player/mpv/archive/refs/tags/v$MPV_VERSION.tar.gz
MPV_SHA256=bcfebbfa2b65a6d88dc97ea18117831072373e38b5785a2afb874ed4df5070dd

# libplacebo comes from git: its release archives lack the submodules it
# builds with. The commit of the tag is checked instead of a hash.
LIBPLACEBO_VERSION=7.387.0
LIBPLACEBO_GIT=https://code.videolan.org/videolan/libplacebo.git
LIBPLACEBO_COMMIT=275f7dea27924cdd34ab2e6c20e448b6bc4fab65

NODE_VERSION=24.22.3
NODE_URL=https://nodejs.org/dist/v$NODE_VERSION/node-v$NODE_VERSION-linux-x64.tar.xz
NODE_SHA256=4f2b175a6d53a5c617a80dba2676238a0688cef28c6da4eba35c7d844e66fcfb
