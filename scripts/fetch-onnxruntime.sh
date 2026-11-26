#!/usr/bin/env bash
# Fetch the pinned ONNX Runtime shared library that `ort` (`load-dynamic`) loads on demand, and
# print its path. It is cached in the app data dir, which is also where unbundled binaries
# (`cargo run`, a standalone `slackless` CLI) look for it; `bundle-macos.sh` copies it into
# `Slackless.app/Contents/Frameworks/`. Lookup order: crates/slackless-huddle/src/onnx_runtime.rs.
#
# Version: ort 2.0.0-rc.12 with `api-24` needs ONNX Runtime >= 1.24; 1.24.2 is the build that ort
# release was made against. Source: Microsoft's official PyPI wheel, which ships the standalone C
# library next to the Python bindings (the GitHub release archives are unavailable since 2026-11-17).
#
# Usage: scripts/fetch-onnxruntime.sh

set -euo pipefail

ORT_VERSION="1.24.2"
ORT_WHEEL_URL="https://files.pythonhosted.org/packages/9c/98/8f5b9ae63f7f6dd5fb2d192454b915ec966a421fdd0effeeef5be7f7221f/onnxruntime-${ORT_VERSION}-cp313-cp313-macosx_14_0_arm64.whl"
# Digest PyPI publishes for the wheel, and the digest of the library inside it.
ORT_WHEEL_SHA256="038ebcd8363c3835ea83eed66129e1d11d8219438892dfb7dc7656c4d4dfa1f9"
ORT_DYLIB_SHA256="0f235b39934a9c8279acfa9f206c3865276bf7db95b146f4867c6a239b9d6ded"

if [[ "$(uname -s)-$(uname -m)" != "Darwin-arm64" ]]; then
  echo "error: only the macOS arm64 ONNX Runtime build is pinned; set ORT_DYLIB_PATH to your own libonnxruntime instead" >&2
  exit 1
fi

DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/slackless/onnxruntime"
DEST="$DEST_DIR/libonnxruntime.dylib"

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

verify() {
  local actual
  actual="$(sha256_of "$1")"
  if [[ "$actual" != "$2" ]]; then
    echo "error: SHA-256 mismatch for $1 (got $actual, expected $2)" >&2
    exit 1
  fi
}

if [[ ! -f "$DEST" || "$(sha256_of "$DEST")" != "$ORT_DYLIB_SHA256" ]]; then
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  echo "Downloading ONNX Runtime $ORT_VERSION..." >&2
  curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
    -o "$TMP/onnxruntime.whl" "$ORT_WHEEL_URL"
  verify "$TMP/onnxruntime.whl" "$ORT_WHEEL_SHA256"
  unzip -p "$TMP/onnxruntime.whl" "onnxruntime/capi/libonnxruntime.${ORT_VERSION}.dylib" \
    >"$TMP/libonnxruntime.dylib"
  verify "$TMP/libonnxruntime.dylib" "$ORT_DYLIB_SHA256"
  mkdir -p "$DEST_DIR"
  mv "$TMP/libonnxruntime.dylib" "$DEST"
fi

printf '%s\n' "$DEST"
