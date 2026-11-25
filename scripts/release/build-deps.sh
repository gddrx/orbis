#!/bin/bash
# Runs inside the release container (appimage-release.sh starts it): builds
# into /opt/deps what the release AppImage takes from source rather than
# from AlmaLinux — KDE's KWindowSystem (with what it builds with), and the
# playback stack: libplacebo, FFmpeg and libmpv, audio only and without
# GPL or non-free parts. Each one is built once; delete /opt/deps (on the
# host target/release-build/deps) to build them again.
#
#   /sources  the archives of fetch-sources.sh, read only
#   /opt/deps the result, kept between runs
set -euo pipefail
source /scripts/versions.sh
JOBS=$(nproc)
WORK=/tmp/deps-build
mkdir -p "$WORK" /opt/deps

# built NAME: whether NAME is already in /opt/deps; mark NAME records it.
built() { [ -e "/opt/deps/.built-$1" ]; }
mark() { touch "/opt/deps/.built-$1"; }

# unpack ARCHIVE: into $WORK, printing the directory it made.
unpack() {
    local dir
    dir=$(tar -tf "$1" | head -1 | cut -d/ -f1)
    rm -rf "${WORK:?}/$dir"
    tar -xf "$1" -C "$WORK"
    echo "$WORK/$dir"
}

cmake_build() {
    local src=$1
    shift
    cmake -S "$src" -B "$src/build" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/deps \
        -DCMAKE_INSTALL_LIBDIR=lib -DBUILD_TESTING=OFF "$@"
    cmake --build "$src/build" -j "$JOBS"
    cmake --install "$src/build"
}

if ! built ecm; then
    src=$(unpack "/sources/extra-cmake-modules-$ECM_VERSION.tar.xz")
    cmake_build "$src" -DBUILD_DOC=OFF -DBUILD_QTHELP_DOCS=OFF
    mark ecm
fi

if ! built wayland-protocols; then
    src=$(unpack "/sources/wayland-protocols-$WAYLAND_PROTOCOLS_VERSION.tar.xz")
    meson setup "$src/build" "$src" --prefix=/opt/deps --libdir=lib -Dtests=false
    meson install -C "$src/build"
    mark wayland-protocols
fi

if ! built plasma-wayland-protocols; then
    src=$(unpack "/sources/plasma-wayland-protocols-$PLASMA_WAYLAND_PROTOCOLS_VERSION.tar.xz")
    cmake_build "$src"
    mark plasma-wayland-protocols
fi

if ! built kwindowsystem; then
    src=$(unpack "/sources/kwindowsystem-$KWINDOWSYSTEM_VERSION.tar.xz")
    cmake_build "$src" -DBUILD_QCH=OFF -DBUILD_PYTHON_BINDINGS=OFF -DKWINDOWSYSTEM_QML=OFF
    mark kwindowsystem
fi

if ! built libplacebo; then
    rm -rf "$WORK/libplacebo"
    cp -a "/sources/libplacebo-$LIBPLACEBO_VERSION" "$WORK/libplacebo"
    # No GPU API: orbis shows no video, libmpv only needs the library.
    meson setup "$WORK/libplacebo/build" "$WORK/libplacebo" \
        --prefix=/opt/deps --libdir=lib --buildtype=release \
        -Dvulkan=disabled -Dopengl=disabled -Dd3d11=disabled \
        -Dglslang=disabled -Dshaderc=disabled -Dlcms=disabled \
        -Ddovi=disabled -Dlibdovi=disabled -Dxxhash=disabled -Dunwind=disabled \
        -Ddemos=false -Dtests=false
    meson compile -C "$WORK/libplacebo/build" -j "$JOBS"
    meson install -C "$WORK/libplacebo/build"
    mark libplacebo
fi

if ! built ffmpeg; then
    src=$(unpack "/sources/ffmpeg-$FFMPEG_VERSION.tar.xz")
    # LGPL: no --enable-gpl, no --enable-nonfree. Only what orbis asks of it
    # (crates/download/src/format.rs and loudness.rs) and what libmpv needs
    # to play YouTube's audio: WebM/Matroska and MP4 over HTTPS, Opus and
    # AAC. The other decoders cover files orbis made itself.
    (cd "$src" && ./configure --prefix=/opt/deps --libdir=/opt/deps/lib \
        --enable-shared --disable-static --disable-doc --disable-debug \
        --disable-ffplay --disable-ffprobe \
        --disable-everything --disable-autodetect \
        --enable-zlib --enable-openssl --enable-libopus --enable-libmp3lame \
        --enable-protocol=file,pipe,http,https,tcp,udp,tls,crypto,data \
        --enable-demuxer=matroska,mov,ogg,mp3,aac,flac,wav \
        --enable-muxer=opus,ogg,mp3,ipod,mp4,null,wav \
        --enable-decoder=opus,libopus,aac,aac_latm,mp3,mp3float,vorbis,flac,pcm_s16le,pcm_f32le \
        --enable-encoder=libopus,libmp3lame,aac,pcm_s16le \
        --enable-parser=opus,aac,aac_latm,mpegaudio,vorbis,flac \
        --enable-bsf=aac_adtstoasc \
        --enable-filter=aresample,aformat,anull,abuffer,abuffersink,volume,ebur128,atrim,asetpts,scale,format,null,buffer,buffersink \
        --enable-swscale --enable-swresample --enable-avfilter --disable-avdevice)
    make -C "$src" -j "$JOBS"
    make -C "$src" install
    mark ffmpeg
fi

if ! built mpv; then
    src=$(unpack "/sources/v$MPV_VERSION.tar.gz")
    # libmpv alone, under the LGPL (-Dgpl=false), audio outputs only.
    meson setup "$src/build" "$src" \
        --prefix=/opt/deps --libdir=lib --buildtype=release \
        -Dlibmpv=true -Dcplayer=false -Dgpl=false \
        -Dlua=disabled -Djavascript=disabled -Dcdda=disabled -Ddvdnav=disabled \
        -Dlibbluray=disabled -Drubberband=disabled -Dlibarchive=disabled \
        -Dvapoursynth=disabled -Dzimg=disabled -Duchardet=disabled -Dlcms2=disabled \
        -Djpeg=disabled -Dvulkan=disabled -Dgl=disabled -Dx11=disabled \
        -Dwayland=disabled -Ddrm=disabled -Degl=disabled -Dgbm=disabled \
        -Dlibavdevice=disabled -Dsdl2-audio=disabled -Dsdl2-video=disabled -Dsdl2-gamepad=disabled \
        -Dalsa=enabled -Dpipewire=enabled -Dpulse=enabled -Djack=disabled \
        -Dmanpage-build=disabled -Dhtml-build=disabled -Dpdf-build=disabled
    meson compile -C "$src/build" -j "$JOBS"
    meson install -C "$src/build"
    mark mpv
fi

echo "deps ready in /opt/deps"
