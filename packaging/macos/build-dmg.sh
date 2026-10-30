#!/usr/bin/env bash
# Builds FMX.app (Intel + Apple silicon) and packs it into a disk image.
# Run on macOS from the repository root: packaging/macos/build-dmg.sh
# Writes dist/fmx-<version>-macos-universal.dmg.
set -euo pipefail

version=$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')
targets=(aarch64-apple-darwin x86_64-apple-darwin)
for target in "${targets[@]}"; do
  rustup target add "$target"
  cargo build --release --locked --target "$target"
done

app=dist/FMX.app
rm -rf "$app" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/fmx" \
  target/aarch64-apple-darwin/release/fmx target/x86_64-apple-darwin/release/fmx
cp packaging/icons/fmx.icns "$app/Contents/Resources/fmx.icns"
cp packaging/THIRD-PARTY-NOTICES.md "$app/Contents/Resources/"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
# Bundled ffmpeg and yt-dlp go in tools/bin beside the executable, where FMX looks first.
python3 packaging/fetch-bundled.py "$app/Contents/MacOS/tools" macos-universal
# Ad-hoc signature: required on Apple silicon. Not notarized (needs a paid
# Apple developer ID), so the first launch needs right-click → Open.
codesign --force --deep --sign - "$app"

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
dmg="dist/fmx-$version-macos-universal.dmg"
rm -f "$dmg"
hdiutil create -volname "FMX $version" -srcfolder "$staging" -ov -format UDZO -imagekey zlib-level=9 "$dmg"
rm -rf "$staging"
echo "$dmg"
