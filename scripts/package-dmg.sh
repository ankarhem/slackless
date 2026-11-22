#!/usr/bin/env bash
# Package Slackless.app into a distributable compressed macOS disk image (.dmg)
# under target/bundle/Slackless-<version>-<arch>.dmg with a SHA-256 checksum.
#
# Usage:
#   scripts/package-dmg.sh             # build release Slackless.app and create DMG
#   scripts/package-dmg.sh --no-build  # wrap existing target/bundle/Slackless.app

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DO_BUILD=1

for arg in "$@"; do
  case "$arg" in
    --no-build) DO_BUILD=0 ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

APP="$ROOT/target/bundle/Slackless.app"
if [[ "$DO_BUILD" -eq 1 ]] || [[ ! -d "$APP" ]]; then
  "$ROOT/scripts/bundle-macos.sh" --release
fi

if [[ ! -d "$APP" ]]; then
  echo "Expected app bundle not found at $APP" >&2
  exit 1
fi

VERSION="$(awk -F'"' '/^version *=/{print $2; exit}' "$ROOT/Cargo.toml")"
ARCH="$(uname -m)"
OUT="$ROOT/target/bundle/Slackless-${VERSION}-${ARCH}.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP" "$STAGE/Slackless.app"
ln -s /Applications "$STAGE/Applications"

rm -f "$OUT" "${OUT}.sha256"
hdiutil create \
  -volname "Slackless ${VERSION}" \
  -srcfolder "$STAGE" \
  -ov \
  -format UDZO \
  "$OUT" >/dev/null

hdiutil verify "$OUT" >/dev/null
shasum -a 256 "$OUT" | tee "${OUT}.sha256"
ls -lh "$OUT"
