#!/usr/bin/env bash
# Builds the macOS release into target/dist/ (experimental: built by CI, not yet tested by us):
#   MisterDownloadManager-<version>-macos-arm64-experimental.dmg   Apple Silicon
#   MisterDownloadManager-<version>-macos-x86_64-experimental.dmg  Intel
# One app per processor, each half the size of a universal one (#25), ad-hoc signed: without an
# Apple Developer ID, macOS asks before the first start (right-click the app -> Open).
# The app carries the Safari extension (tools/safari). Safari loads an ad-hoc signed one only with
# Develop -> Allow Unsigned Extensions (each time Safari starts); signed with a Developer ID
# (MDM_MACOS_IDENTITY="Developer ID Application: ..."), it is an ordinary extension.
# MDM_NOTARY_PROFILE=<name> (a `xcrun notarytool store-credentials` profile) also has Apple notarize
# the app and the .dmg: Safari ignores a Developer ID extension that isn't.
# Needs: macOS with Xcode's command line tools (swiftc), rustup, pwsh (the extension).
#   tools/installer/build-macos.sh [--no-build]
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
dist="$root/target/dist"
version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/Cargo.toml" | head -1)"
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
programs=(misterdownloadmanager misterdownloadmanager-nmhost misterdownloadmanager-cli misterdownloadmanager-mcp)
# Rust's target and the processor's name in the release files (as macOS calls it), with ':'.
targets=(aarch64-apple-darwin:arm64 x86_64-apple-darwin:x86_64)

if [ "${1:-}" != "--no-build" ]; then
  for pair in "${targets[@]}"; do
    t="${pair%%:*}"
    rustup target add "$t"
    # The build machine's paths out of the programs (#25), as build-linux.sh does.
    CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo"$'\x1f'"--remap-path-prefix=$root=mdm" \
      cargo build --release --target "$t" -p mdm-app -p mdm-nmhost -p mdm-cli -p mdm-mcp
  done
fi
pwsh -NoProfile -File "$root/tools/extension/pack.ps1"
sign="${MDM_MACOS_IDENTITY:--}"
# A real identity also gets the hardened runtime (notarization needs it).
sign_opts=()
[ "$sign" = "-" ] || sign_opts=(--options runtime --timestamp)

mkdir -p "$dist"
stage="$dist/stage-macos"
rm -rf "$stage"
mkdir -p "$stage"

# The icon: every size macOS asks for, from the 1254 px artwork.
set_dir="$stage/AppIcon.iconset"
mkdir -p "$set_dir"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$root/art/Mister Download Manager.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" "$root/art/Mister Download Manager.png" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o "$stage/AppIcon.icns"

dmgs=()
for pair in "${targets[@]}"; do
  t="${pair%%:*}"
  arch="${pair##*:}"
  name="MisterDownloadManager-$version-macos-$arch-experimental"
  app="$stage/$arch/dmg/Mister Download Manager.app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

  # Crash reports: the release's symbols for this processor (the -symbols zip, one per .dmg) from
  # the programs' debug info, which stays in the build's object files until dsymutil gathers it.
  syms=()
  for p in "${programs[@]}"; do
    dsymutil "$root/target/$t/release/$p" -o "$stage/$arch/dsym/$p.dSYM"
    syms+=("$stage/$arch/dsym/$p.dSYM")
  done
  python3 "$root/tools/installer/symbols.py" "macos-$arch" "$version" "$dist" "${syms[@]}"
  # Then the programs without their debug map and local symbols (the UUID that matches them to
  # the symbols stays).
  for p in "${programs[@]}"; do
    cp "$root/target/$t/release/$p" "$app/Contents/MacOS/$p"
    strip -S -x "$app/Contents/MacOS/$p"
  done
  cp -r "$root/target/extension/chromium" "$app/Contents/Resources/extension-chromium"
  cp "$root/target/extension/mdm-firefox.xpi" "$root/LICENSE" "$root/ATTRIBUTION.md" "$app/Contents/Resources/"
  cp "$stage/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"

  cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Mister Download Manager</string>
  <key>CFBundleDisplayName</key><string>Mister Download Manager</string>
  <key>CFBundleIdentifier</key><string>io.github.anefzaoui.misterdownloadmanager</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>misterdownloadmanager</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- MDM opens .torrent files and magnet links (#698): Finder offers it for them, and hands
       them over as Apple events (crates/app/src/unix.rs open_events). The .torrent type is
       imported: an app that knows it better (one that exports it) keeps its description. -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>BitTorrent Document</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Default</string>
      <key>LSItemContentTypes</key><array><string>org.bittorrent.torrent</string></array>
    </dict>
  </array>
  <key>UTImportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>org.bittorrent.torrent</string>
      <key>UTTypeDescription</key><string>BitTorrent Document</string>
      <key>UTTypeConformsTo</key><array><string>public.data</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key><array><string>torrent</string></array>
        <key>public.mime-type</key><array><string>application/x-bittorrent</string></array>
      </dict>
    </dict>
  </array>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>Magnet link</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>CFBundleURLSchemes</key><array><string>magnet</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF
  plutil -lint "$app/Contents/Info.plist"
  # The Safari extension (it carries its own copy of the helper, signed for its sandbox).
  "$root/tools/safari/build-appex.sh" "$app" "$arch-apple-macos14.0" "$root/target/extension/safari" \
    "$app/Contents/MacOS/misterdownloadmanager-nmhost" "$version" "$sign"
  # The helper programs, then the app, which signs its own program (not --deep: that would sign
  # the extension again, without the entitlements it was given). Signing the app's own program
  # alone asks for the others to be signed already, which Intel builds made on an Apple Silicon
  # Mac are not (the linker signs only its own processor's).
  for p in "${programs[@]}"; do
    [ "$p" = misterdownloadmanager ] || codesign --force ${sign_opts[@]+"${sign_opts[@]}"} --sign "$sign" "$app/Contents/MacOS/$p"
  done
  codesign --force ${sign_opts[@]+"${sign_opts[@]}"} --sign "$sign" "$app"
  codesign --verify --strict --deep "$app"
  if [ -n "${MDM_NOTARY_PROFILE:-}" ]; then
    # Apple scans a zip of the app; the ticket is stapled to the app, so it also opens offline.
    ditto -c -k --keepParent "$app" "$stage/$arch/notarize.zip"
    xcrun notarytool submit "$stage/$arch/notarize.zip" --keychain-profile "$MDM_NOTARY_PROFILE" --wait
    xcrun stapler staple "$app"
    rm -f "$stage/$arch/notarize.zip"
  fi

  # The disk image: the app next to a link to Applications, to drag it there. LZMA (ULMO, macOS
  # 10.15 and later; the app needs 11) packs it tighter than zlib (UDZO).
  ln -s /Applications "$stage/$arch/dmg/Applications"
  rm -f "$dist/$name.dmg"
  hdiutil create -volname "Mister Download Manager" -srcfolder "$stage/$arch/dmg" -ov -format ULMO \
    "$dist/$name.dmg"
  if [ -n "${MDM_NOTARY_PROFILE:-}" ]; then
    codesign --force --sign "$sign" --timestamp "$dist/$name.dmg"
    xcrun notarytool submit "$dist/$name.dmg" --keychain-profile "$MDM_NOTARY_PROFILE" --wait
    xcrun stapler staple "$dist/$name.dmg"
  fi
  dmgs+=("$name.dmg")
done

rm -rf "$stage"
(cd "$dist" && shasum -a 256 "${dmgs[@]}" > SHA256SUMS-macos.txt)
ls -l "$dist"
