# Fetches into $TOOLS, once, what both AppImage builds need: linuxdeploy
# and its Qt plugin (the "continuous" builds), the AppImage runtime
# (checked against the SHA-256 GitHub gives for it) and the latest yt-dlp
# (checked against its release's SHA2-256SUMS). Sourced by appimage.sh and
# appimage-release.sh; delete $TOOLS to take newer ones.
mkdir -p "$TOOLS"

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
