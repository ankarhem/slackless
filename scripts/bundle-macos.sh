#!/usr/bin/env bash
# Package Slackless into a signed macOS .app bundle (`target/bundle/Slackless.app`).
# Declares CFBundleIdentifier (`dev.slackless.gui`) and `slack://` URL scheme so macOS
# delivers native UNUserNotificationCenter banners and OS-wide slack:// deep links.
#
# Usage:
#   scripts/bundle-macos.sh              # build debug binary and assemble Slackless.app
#   scripts/bundle-macos.sh --release    # build release binary and assemble Slackless.app
#   scripts/bundle-macos.sh --no-build   # bundle existing target/debug/slackless-gui
#   scripts/bundle-macos.sh --install    # also install to ~/Applications/Slackless.app

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="debug"
DO_BUILD=1
DO_INSTALL=0

for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    --no-build) DO_BUILD=0 ;;
    --install) DO_INSTALL=1 ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ "$DO_BUILD" -eq 1 ]]; then
  if [[ "$PROFILE" == "release" ]]; then
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --release -p slackless-gui --manifest-path "$ROOT/Cargo.toml"
  else
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build -p slackless-gui --manifest-path "$ROOT/Cargo.toml"
  fi
fi

BIN="$ROOT/target/$PROFILE/slackless-gui"
if [[ ! -x "$BIN" ]]; then
  echo "Expected binary not found at $BIN" >&2
  exit 1
fi

VERSION="$(awk -F'"' '/^version *=/{print $2; exit}' "$ROOT/Cargo.toml")"
APP="$ROOT/target/bundle/Slackless.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/slackless-gui"
chmod +x "$APP/Contents/MacOS/slackless-gui"

ICON_PNG="$ROOT/apps/slackless-gui/icon.png"
if [[ -f "$ICON_PNG" ]] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
  ICONSET="$(mktemp -d)/AppIcon.iconset"
  mkdir -p "$ICONSET"
  for sz in 16 32 128 256 512; do
    sips -z "$sz" "$sz" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
    sz2=$((sz * 2))
    sips -z "$sz2" "$sz2" "$ICON_PNG" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  rm -rf "$(dirname "$ICONSET")"
fi

cat >"$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Slackless</string>
  <key>CFBundleExecutable</key>
  <string>slackless-gui</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string>dev.slackless.gui</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Slackless</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>Slack Deep Link</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>slack</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
EOF

plutil -lint "$APP/Contents/Info.plist" >/dev/null
if command -v codesign >/dev/null; then
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
fi

echo "Bundled $APP (version $VERSION)"

if [[ "$DO_INSTALL" -eq 1 ]]; then
  DEST="$HOME/Applications/Slackless.app"
  mkdir -p "$HOME/Applications"
  rm -rf "$DEST"
  cp -R "$APP" "$DEST"
  LSREG="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
  if [[ -x "$LSREG" ]]; then
    "$LSREG" -f "$DEST" >/dev/null 2>&1 || true
  fi
  echo "Installed $DEST"
fi
