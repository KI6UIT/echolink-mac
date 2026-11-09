#!/usr/bin/env bash
# Build a self-contained EchoLink.app and package it as a .dmg for other Macs.
#
#   packaging/build-dmg.sh
#
# Output: dist/EchoLink-<version>-<arch>.dmg (and dist/EchoLink.app)
#
# The app is built for the architecture of the Mac you run this on. libgsm is
# copied into the bundle, so people installing the DMG don't need Homebrew.
#
# Signing (optional; without it the app is ad-hoc signed and macOS asks users
# to approve it under System Settings > Privacy & Security > Open Anyway):
#
#   SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#       Sign with your Developer ID, with the hardened runtime on.
#   NOTARY_PROFILE=my-profile
#       Also notarize and staple the DMG. Create the profile once with:
#       xcrun notarytool store-credentials my-profile
#   Needs SIGN_IDENTITY as well.
set -euo pipefail
cd "$(dirname "$0")/.."

[[ "$(uname)" == "Darwin" ]] || { echo "This script only runs on macOS." >&2; exit 1; }
for tool in cargo otool install_name_tool codesign hdiutil sips iconutil qlmanage; do
    command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }
done
if [[ -n "${NOTARY_PROFILE:-}" && -z "${SIGN_IDENTITY:-}" ]]; then
    echo "NOTARY_PROFILE needs SIGN_IDENTITY too: Apple only notarizes Developer ID signed code." >&2
    exit 1
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
dist="dist"
app="$dist/EchoLink.app"
dmg="$dist/EchoLink-$version-$arch.dmg"
entitlements="$(mktemp -t echolink-entitlements).plist"
trap 'rm -f "$entitlements"' EXIT

echo "==> Building echolink-gui (release)"
cargo build --release --bin echolink-gui

echo "==> Assembling $app"
rm -rf "$app" "$dmg"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp target/release/echolink-gui "$app/Contents/MacOS/echolink-gui"
strip -x "$app/Contents/MacOS/echolink-gui" 2>/dev/null || true

# Icon: render the SVG, then build an .icns from it.
tmp="$(mktemp -d)"
if qlmanage -t -s 1024 -o "$tmp" packaging/echolink-gui.svg >/dev/null 2>&1 \
    && [[ -f "$tmp/echolink-gui.svg.png" ]]; then
    iset="$tmp/icon.iconset"; mkdir "$iset"
    for sz in 16 32 128 256 512; do
        sips -z $sz $sz "$tmp/echolink-gui.svg.png" --out "$iset/icon_${sz}x${sz}.png" >/dev/null
        sips -z $((sz*2)) $((sz*2)) "$tmp/echolink-gui.svg.png" --out "$iset/icon_${sz}x${sz}@2x.png" >/dev/null
    done
    iconutil -c icns "$iset" -o "$app/Contents/Resources/EchoLink.icns" || echo "warning: no icon" >&2
else
    echo "warning: couldn't render the icon; the app will use the default one" >&2
fi
rm -rf "$tmp"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>EchoLink</string>
  <key>CFBundleDisplayName</key><string>EchoLink</string>
  <key>CFBundleIdentifier</key><string>org.ki6uit.echolink-gui</string>
  <key>CFBundleExecutable</key><string>echolink-gui</string>
  <key>CFBundleIconFile</key><string>EchoLink</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>EchoLink needs the microphone to transmit your voice.</string>
</dict></plist>
PLIST

echo "==> Bundling libgsm"
bin="$app/Contents/MacOS/echolink-gui"
gsm_ref="$(otool -L "$bin" | awk '/libgsm/ {print $1; exit}')"
if [[ -z "$gsm_ref" ]]; then
    echo "The binary doesn't link libgsm dynamically; nothing to bundle."
else
    # Resolve through symlinks (Homebrew's opt/ path points at the Cellar).
    gsm_real="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$gsm_ref")"
    gsm_name="$(basename "$gsm_ref")"
    cp "$gsm_real" "$app/Contents/Frameworks/$gsm_name"
    chmod 644 "$app/Contents/Frameworks/$gsm_name"
    install_name_tool -id "@rpath/$gsm_name" "$app/Contents/Frameworks/$gsm_name"
    install_name_tool -change "$gsm_ref" "@rpath/$gsm_name" "$bin"
    install_name_tool -add_rpath "@executable_path/../Frameworks" "$bin"
fi

# Anything still pointing outside the system would break on a clean Mac.
if otool -L "$bin" "$app/Contents/Frameworks/"* 2>/dev/null | grep -E "/opt/homebrew|/usr/local" ; then
    echo "error: the bundle still depends on libraries outside macOS (listed above)." >&2
    exit 1
fi

echo "==> Signing"
cat > "$entitlements" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.device.audio-input</key><true/>
</dict></plist>
PLIST
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
    sign=(codesign --force --timestamp --options runtime --sign "$SIGN_IDENTITY")
else
    echo "    (no SIGN_IDENTITY set: ad-hoc signing)"
    sign=(codesign --force --sign -)
fi
# Inside out: libraries first, then the app that contains them.
for lib in "$app/Contents/Frameworks/"*; do
    [[ -e "$lib" ]] && "${sign[@]}" "$lib"
done
"${sign[@]}" --entitlements "$entitlements" "$app"
codesign --verify --strict --verbose=1 "$app"

echo "==> Creating $dmg"
stage="$(mktemp -d)"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "EchoLink" -srcfolder "$stage" -ov -format UDZO -fs HFS+ "$dmg" >/dev/null
rm -rf "$stage"
[[ -n "${SIGN_IDENTITY:-}" ]] && codesign --force --timestamp --sign "$SIGN_IDENTITY" "$dmg"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    echo "==> Notarizing (this takes a few minutes)"
    xcrun notarytool submit "$dmg" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$dmg"
fi

echo
echo "Done: $dmg"
shasum -a 256 "$dmg"
du -h "$dmg" | awk '{print "Size: " $1}'
