#!/usr/bin/env bash
# Package Slackless into a signed macOS .app bundle (`target/bundle/Slackless.app`).
# Declares CFBundleIdentifier (`dev.slackless.gui`) and `slack://` URL scheme so macOS
# delivers native UNUserNotificationCenter banners and OS-wide slack:// deep links.
#
# Usage:
#   scripts/bundle-macos.sh                # build debug binary and assemble Slackless.app
#   scripts/bundle-macos.sh --release      # build release binary and assemble Slackless.app
#   scripts/bundle-macos.sh --debug        # force debug binary (even with --install)
#   scripts/bundle-macos.sh --no-build     # bundle existing target/<profile>/slackless-gui
#   scripts/bundle-macos.sh --install      # build release (unless --debug) & install to ~/Applications/Slackless.app
#   scripts/bundle-macos.sh --install-cli  # also build & install ~/.local/bin/slackless

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE=""
DO_BUILD=1
DO_INSTALL=0
DO_INSTALL_CLI=0

for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    --debug) PROFILE="debug" ;;
    --no-build) DO_BUILD=0 ;;
    --install) DO_INSTALL=1 ;;
    --install-cli) DO_INSTALL_CLI=1 ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$PROFILE" ]]; then
  if [[ "$DO_INSTALL" -eq 1 ]]; then
    PROFILE="release"
  else
    PROFILE="debug"
  fi
fi

if [[ "$DO_BUILD" -eq 1 ]]; then
  PKGS=(-p slackless-gui -p slackless-cli)
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
  if [[ "$PROFILE" == "release" ]]; then
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build --release "${PKGS[@]}" --manifest-path "$ROOT/Cargo.toml"
  else
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" cargo build "${PKGS[@]}" --manifest-path "$ROOT/Cargo.toml"
  fi
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BIN="$TARGET_DIR/$PROFILE/slackless-gui"
CLI_BIN="$TARGET_DIR/$PROFILE/slackless"
if [[ ! -x "$BIN" ]]; then
  echo "Expected binary not found at $BIN" >&2
  exit 1
fi

VERSION="${SLACKLESS_VERSION:-$(awk -F'"' '/^version *=/{print $2; exit}' "$ROOT/Cargo.toml")}"
BUNDLE_VERSION="${VERSION//-/.}"
APP="$TARGET_DIR/bundle/Slackless.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Frameworks"

cp "$BIN" "$APP/Contents/MacOS/slackless-gui"
chmod +x "$APP/Contents/MacOS/slackless-gui"
if [[ -x "$CLI_BIN" ]]; then
  cp "$CLI_BIN" "$APP/Contents/MacOS/slackless"
  chmod +x "$APP/Contents/MacOS/slackless"
fi

# ONNX Runtime is not linked into the binaries; they load it on demand from Contents/Frameworks.
# The pinned, checksum-verified download is cached, so only the first bundle hits the network.
ORT_SRC="$("$ROOT/scripts/fetch-onnxruntime.sh")"
ORT_DYLIB="$APP/Contents/Frameworks/libonnxruntime.dylib"
cp "$ORT_SRC" "$ORT_DYLIB"

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
  <string>${BUNDLE_VERSION}</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Slackless needs microphone access so you can speak in Slack Huddles.</string>
  <key>NSCameraUsageDescription</key>
  <string>Slackless needs camera access so you can share video in Slack Huddles.</string>
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
SIGN_LABEL=""
if [[ -z "${SLACKLESS_SIGN_IDENTITY:-}" ]] && command -v security >/dev/null; then
  # Prefer a persistent local Code Signing certificate (e.g. "Slackless Dev") so macOS TCC
  # remembers Screen Recording / Microphone / Camera permissions across rebuilds.
  # Sign by 40-hex SHA-1 hash to avoid case-insensitive ambiguity and allow local self-signed
  # certs (which `codesign` accepts and turns into a stable `certificate leaf = H"..."` DR).
  AUTO_LINE="$(security find-identity -v -p codesigning 2>/dev/null | grep -F '"Slackless Dev"' | head -n 1 || true)"
  if [[ -z "$AUTO_LINE" ]]; then
    AUTO_LINE="$(security find-identity -p codesigning 2>/dev/null | grep -F '"Slackless Dev"' | head -n 1 || true)"
  fi
  if [[ -z "$AUTO_LINE" ]]; then
    AUTO_LINE="$(security find-identity -v -p codesigning 2>/dev/null | awk '/[0-9]+\) [0-9A-F]{40} "/ { print; exit }' || true)"
  fi
  AUTO_HASH="$(printf '%s\n' "$AUTO_LINE" | sed -n 's/.*) \([0-9A-F]\{40\}\) .*/\1/p')"
  SIGN_LABEL="$(printf '%s\n' "$AUTO_LINE" | sed -n 's/.*"\([^"]*\)".*/\1/p')"
  SIGN_ID="${AUTO_HASH:--}"
else
  SIGN_ID="${SLACKLESS_SIGN_IDENTITY:--}"
fi
if command -v codesign >/dev/null; then
  # Nested code first, with the same identity, then the app that seals it.
  codesign --force --sign "$SIGN_ID" "$ORT_DYLIB"
  codesign --force --deep --sign "$SIGN_ID" "$APP"
  if [[ -n "$SIGN_LABEL" ]]; then
    echo "Signed $APP with identity: $SIGN_LABEL ($SIGN_ID)"
  else
    echo "Signed $APP with identity: $SIGN_ID"
  fi
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

if [[ "$DO_INSTALL_CLI" -eq 1 ]]; then
  if [[ -d "/usr/local/bin" && -w "/usr/local/bin" ]]; then
    CLI_DIR="/usr/local/bin"
  else
    CLI_DIR="$HOME/.local/bin"
    mkdir -p "$CLI_DIR"
  fi
  CLI_LINK="${CLI_DIR}/slackless"
  if [[ "$DO_INSTALL" -eq 1 && -x "$HOME/Applications/Slackless.app/Contents/MacOS/slackless" ]]; then
    ln -sf "$HOME/Applications/Slackless.app/Contents/MacOS/slackless" "$CLI_LINK"
    echo "Linked ${CLI_LINK} -> $HOME/Applications/Slackless.app/Contents/MacOS/slackless"
  else
    if [[ ! -x "$CLI_BIN" ]]; then
      echo "Expected CLI binary not found at $CLI_BIN" >&2
      exit 1
    fi
    cp "$CLI_BIN" "$CLI_LINK"
    chmod +x "$CLI_LINK"
    if command -v codesign >/dev/null; then
      codesign --force --sign "$SIGN_ID" "$CLI_LINK" >/dev/null 2>&1 || true
    fi
    echo "Installed ${CLI_LINK}"
  fi
fi
