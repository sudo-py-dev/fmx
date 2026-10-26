#!/bin/sh
# Build the .deb from release binaries:
#   packaging/linux/build-deb.sh <version> <bin-dir> <out-dir>
#
# Installs fmx to /usr/bin with a menu entry and icons.
set -eu

version="$1"
bin="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../.."
root="$(mktemp -d)"
chmod 755 "$root"
trap 'rm -rf "$root"' EXIT

# ── Binaries, desktop entry, icons ──────────────────────────────────────────
install -Dm755 "$bin/fmx" "$root/usr/bin/fmx"
install -Dm644 "$here/fmx.desktop" "$root/usr/share/applications/fmx.desktop"

for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 \
        "$repo/packaging/icons/fmx-${size}.png" \
        "$root/usr/share/icons/hicolor/${size}x${size}/apps/fmx.png"
done

install -Dm644 "$repo/README.md"      "$root/usr/share/doc/fmx/README.md"
install -Dm644 "$repo/LICENSE-MIT"    "$root/usr/share/doc/fmx/copyright"

# ── Auto-detect shared-library dependencies ──────────────────────────────────
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
Suggests: ffmpeg, poppler-utils, ghostscript, xorriso, calibre
Homepage: https://github.com/sudo-py-dev/fmx
Description: Free Media Xpress — all-in-one media, document and disc converter
 Converts video, audio, images, documents and discs from one window.
 Downloads its own verified ffmpeg and yt-dlp on first use; nothing
 needs to be installed system-wide.
CONTROL

# ── Build the .deb ───────────────────────────────────────────────────────────
mkdir -p "$out"
dpkg-deb --root-owner-group --build "$root" "$out/fmx_${version}_amd64.deb"
