#!/usr/bin/env bash
# Build echolink-tui and echolink-gui and install them for the current user.
# Nothing personal is installed: on first start the app asks for your own
# callsign, password, name and location.
#
#   ./install.sh            desktop app + terminal version
#   ./install.sh --tui-only terminal version only (no GUI libraries)
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null; then
    echo "Rust isn't installed. Get it from https://rustup.rs and run this again." >&2
    exit 1
fi

if [[ "$(uname)" == "Darwin" ]]; then
    # macOS: needs Homebrew's libgsm ("brew install libgsm") and Rust.
    bin="$HOME/.local/bin"
    mkdir -p "$bin"
    if [[ "${1:-}" == "--tui-only" ]]; then
        cargo build --release --no-default-features
        install -m755 target/release/echolink-tui "$bin/echolink-tui"
        echo "Installed echolink-tui to $bin. Run 'echolink-tui setup' to enter your station details."
    else
        cargo build --release
        install -m755 target/release/echolink-tui "$bin/echolink-tui"

        app="$HOME/Applications/EchoLink.app"
        rm -rf "$app"
        mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
        install -m755 target/release/echolink-gui "$app/Contents/MacOS/echolink-gui"

        # Icon: render the SVG, then build an .icns from it.
        tmp="$(mktemp -d)"
        if qlmanage -t -s 1024 -o "$tmp" packaging/echolink-gui.svg >/dev/null 2>&1 \
            && [[ -f "$tmp/echolink-gui.svg.png" ]]; then
            iset="$tmp/icon.iconset"; mkdir "$iset"
            for sz in 16 32 128 256 512; do
                sips -z $sz $sz "$tmp/echolink-gui.svg.png" --out "$iset/icon_${sz}x${sz}.png" >/dev/null
                sips -z $((sz*2)) $((sz*2)) "$tmp/echolink-gui.svg.png" --out "$iset/icon_${sz}x${sz}@2x.png" >/dev/null
            done
            iconutil -c icns "$iset" -o "$app/Contents/Resources/EchoLink.icns" || true
        fi
        rm -rf "$tmp"

        cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>EchoLink</string>
  <key>CFBundleDisplayName</key><string>EchoLink</string>
  <key>CFBundleIdentifier</key><string>org.ki6uit.echolink-gui</string>
  <key>CFBundleExecutable</key><string>echolink-gui</string>
  <key>CFBundleIconFile</key><string>EchoLink</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>EchoLink needs the microphone to transmit your voice.</string>
</dict></plist>
PLIST
        # Ad-hoc sign so macOS will let the bundle ask for microphone access.
        codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
        echo "Installed $app (find it in Launchpad or ~/Applications) and echolink-tui in $bin."
    fi
    case ":$PATH:" in
        *":$bin:"*) ;;
        *) echo "Note: $bin isn't on your PATH, so add it to run echolink-tui from a terminal." ;;
    esac
    exit 0
fi

bin="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"

if [[ "${1:-}" == "--tui-only" ]]; then
    cargo build --release --no-default-features
    install -Dm755 target/release/echolink-tui "$bin/echolink-tui"
    echo "Installed echolink-tui to $bin. Run 'echolink-tui setup' to enter your station details."
    exit 0
fi

cargo build --release
install -Dm755 target/release/echolink-gui "$bin/echolink-gui"
install -Dm755 target/release/echolink-tui "$bin/echolink-tui"
install -Dm644 packaging/echolink-gui.desktop "$data/applications/echolink-gui.desktop"
install -Dm644 packaging/echolink-gui.svg "$data/icons/hicolor/scalable/apps/echolink-gui.svg"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$data/applications" || true

echo "Installed to $bin. Open EchoLink from your app menu; on first start it asks for your callsign."
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "Note: $bin isn't on your PATH, so add it to run echolink-tui from a terminal." ;;
esac
