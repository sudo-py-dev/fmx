#!/bin/sh
# Build the .deb from release binaries:
#   packaging/linux/build-deb.sh <version> <bin-dir> <out-dir>
#
# Installs fmx to /usr/bin with a menu entry, icons, and the bundled tools
# (ffmpeg, ffprobe, yt-dlp and ffmpeg's libraries) at /usr/lib/fmx, so the
# package works offline. <bin-dir>/tools is used when fetch-bundled.py
# already filled it; otherwise the tools are downloaded here.
set -eu

version="$1"
bin="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
root="$(mktemp -d)"
chmod 755 "$root"
trap 'rm -rf "$root"' EXIT

# ── Bundled ffmpeg, ffprobe, yt-dlp ──────────────────────────────────────────
install -dm755 "$root/usr/lib/fmx"
if [ -x "$bin/tools/bin/ffmpeg" ] && [ -x "$bin/tools/bin/yt-dlp" ]; then
    cp -r "$bin/tools/bin" "$bin/tools/lib" "$root/usr/lib/fmx/"
else
    python3 "$repo/packaging/fetch-bundled.py" "$root/usr/lib/fmx" linux64
fi

# ── Binaries, desktop entry, icons ───────────────────────────────────────────
install -Dm755 "$bin/fmx" "$root/usr/bin/fmx"
install -Dm644 "$here/fmx.desktop" "$root/usr/share/applications/fmx.desktop"

for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 \
        "$repo/packaging/icons/fmx-${size}.png" \
        "$root/usr/share/icons/hicolor/${size}x${size}/apps/fmx.png"
done

install -Dm644 "$repo/README.md"      "$root/usr/share/doc/fmx/README.md"
install -Dm644 "$repo/LICENSE-MIT"    "$root/usr/share/doc/fmx/copyright"
install -Dm644 "$repo/packaging/THIRD-PARTY-NOTICES.md" \
    "$root/usr/share/doc/fmx/THIRD-PARTY-NOTICES.md"

# ── Auto-detect shared-library dependencies of the FMX binary ────────────────
mkdir -p "$root/debian"
printf 'Source: fmx\n' > "$root/debian/control"
depends="$(cd "$root" && dpkg-shlibdeps -O usr/bin/fmx 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
rm -rf "$root/debian"

# ── Installed-size (in kB) ───────────────────────────────────────────────────
size="$(du -sk "$root" | cut -f1)"

# ── DEBIAN/control ───────────────────────────────────────────────────────────
mkdir -p "$root/DEBIAN"
cat > "$root/DEBIAN/control" <<CONTROL
Package: fmx
Version: $version
Section: video
Priority: optional
Architecture: amd64
Maintainer: sudo-py-dev <https://github.com/sudo-py-dev/fmx>
Installed-Size: $size
Depends: ${depends:-libc6}
Suggests: poppler-utils, ghostscript, xorriso, calibre
Homepage: https://github.com/sudo-py-dev/fmx
Description: Free Media Xpress - all-in-one media, document and disc converter
 Converts video, audio, images, documents and discs from one window.
 Ships its own ffmpeg, ffprobe and yt-dlp under /usr/lib/fmx and works
 fully offline. Updates come only as new FMX releases.
CONTROL

# ── Build the .deb ───────────────────────────────────────────────────────────
mkdir -p "$out"
# xz -9 packs the bundled tools ~8 MB smaller than the zstd default.
dpkg-deb --root-owner-group -Zxz -z9 --build "$root" "$out/fmx_${version}_amd64.deb"
