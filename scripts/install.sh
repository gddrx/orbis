#!/bin/bash
# Installs orbis for this user: the release build of `orbis` and
# `orbis-login` into ~/.local/bin, the menu entry and the icon into
# ~/.local/share. `--uninstall` removes them. PREFIX=<dir> installs
# elsewhere (the menu sees only ~/.local and the system's folders).
#
# Needed on the system: Qt 6 with Qt Quick and Qt WebEngine, KF6
# WindowSystem, libmpv, and the programs orbis runs — yt-dlp with Node.js,
# and ffmpeg.
set -euo pipefail
cd "$(dirname "$0")/.."
PREFIX=${PREFIX:-$HOME/.local}
bin=$PREFIX/bin
apps=$PREFIX/share/applications
icon=$PREFIX/share/icons/hicolor/scalable/apps/orbis.svg

# The menu reads the new entry once KDE's cache is rebuilt.
refresh_menu() {
    if [ "$PREFIX" = "$HOME/.local" ]; then
        command -v update-desktop-database > /dev/null && update-desktop-database "$apps" || true
        command -v kbuildsycoca6 > /dev/null && kbuildsycoca6 > /dev/null 2>&1 || true
    fi
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -fv "$bin/orbis" "$bin/orbis-login" "$apps/orbis.desktop" "$icon"
    refresh_menu
    echo "orbis is removed; its settings, cache and saved sign-in stay"
    echo "(~/.config/orbis, ~/.cache/orbis, ~/.local/share/orbis, the wallet)."
    exit 0
fi

cargo build --release -p orbis -p orbis-login
# `install` replaces the files: an orbis that runs keeps its old copy.
install -Dm755 target/release/orbis "$bin/orbis"
install -Dm755 target/release/orbis-login "$bin/orbis-login"
install -Dm644 data/orbis.svg "$icon"
mkdir -p "$apps"
sed "s|^Exec=orbis$|Exec=$bin/orbis|" data/orbis.desktop > "$apps/orbis.desktop.part"
mv "$apps/orbis.desktop.part" "$apps/orbis.desktop"
refresh_menu
echo "orbis is installed: $bin/orbis; the menu has it under Multimedia."
