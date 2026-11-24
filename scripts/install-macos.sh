#!/usr/bin/env bash
# Install or update Slackless on macOS (Apple Silicon arm64) directly from GitHub Releases.
# Downloaded via curl (never sets com.apple.quarantine) and verified against SHA256SUMS.txt.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ankarhem/slackless/main/scripts/install-macos.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/ankarhem/slackless/main/scripts/install-macos.sh | bash -s -- --no-cli
#   scripts/install-macos.sh --version 2026.11.24-01 --dir ~/Applications

set -euo pipefail

REPO="ankarhem/slackless"
VERSION="latest"
INSTALL_DIR=""
INSTALL_CLI=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:?--version requires a tag (e.g. 2026.11.24-01)}"
      shift 2
      ;;
    --dir)
      INSTALL_DIR="${2:?--dir requires a directory path}"
      shift 2
      ;;
    --cli)
      INSTALL_CLI=1
      shift
      ;;
    --no-cli)
      INSTALL_CLI=0
      shift
      ;;
    -h|--help)
      sed -n '2,10p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: Slackless prebuilt releases currently support macOS only." >&2
  exit 1
fi

if [[ "$(uname -m)" != "arm64" ]]; then
  echo "error: Slackless prebuilt releases currently target Apple Silicon (arm64)." >&2
  exit 1
fi

if [[ -z "$INSTALL_DIR" ]]; then
  if [[ -w "/Applications" ]]; then
    INSTALL_DIR="/Applications"
  else
    INSTALL_DIR="$HOME/Applications"
  fi
fi

API_URL="https://api.github.com/repos/${REPO}/releases/latest"
if [[ "$VERSION" != "latest" ]]; then
  API_URL="https://api.github.com/repos/${REPO}/releases/tags/${VERSION}"
fi

echo "Fetching release metadata from ${REPO} (${VERSION})..."
RELEASE_JSON="$(curl -fsSL -H "Accept: application/vnd.github+json" -H "User-Agent: slackless-installer" "$API_URL")"

PARSED="$(/usr/bin/python3 - "$RELEASE_JSON" <<'PY'
import json, sys
rel = json.loads(sys.argv[1])
tag = rel.get("tag_name", "")
assets = {a["name"]: a["browser_download_url"] for a in rel.get("assets", [])}
app_name = f"Slackless-{tag}-macos-arm64.app.tar.gz"
cli_name = f"slackless-cli-{tag}-macos-arm64.tar.gz"
print(tag)
print(app_name)
print(assets.get(app_name, ""))
print(cli_name)
print(assets.get(cli_name, ""))
print(assets.get("SHA256SUMS.txt", ""))
PY
)"

TAG="$(printf '%s\n' "$PARSED" | sed -n '1p')"
APP_ASSET="$(printf '%s\n' "$PARSED" | sed -n '2p')"
APP_URL="$(printf '%s\n' "$PARSED" | sed -n '3p')"
CLI_ASSET="$(printf '%s\n' "$PARSED" | sed -n '4p')"
CLI_URL="$(printf '%s\n' "$PARSED" | sed -n '5p')"
SUMS_URL="$(printf '%s\n' "$PARSED" | sed -n '6p')"

if [[ -z "$TAG" || -z "$APP_URL" ]]; then
  echo "error: could not find ${APP_ASSET:-Slackless app archive} in release ${VERSION}" >&2
  exit 1
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading Slackless ${TAG}..."
curl -fL --progress-bar -o "${TMPDIR}/${APP_ASSET}" "$APP_URL"

if [[ -n "$SUMS_URL" ]]; then
  curl -fsSL -o "${TMPDIR}/SHA256SUMS.txt" "$SUMS_URL"
  EXPECTED="$(awk -v f="$APP_ASSET" '$2 == f { print $1; exit }' "${TMPDIR}/SHA256SUMS.txt")"
  if [[ -n "$EXPECTED" ]]; then
    ACTUAL="$(shasum -a 256 "${TMPDIR}/${APP_ASSET}" | awk '{print $1}')"
    if [[ "$ACTUAL" != "$EXPECTED" ]]; then
      echo "error: SHA-256 mismatch for ${APP_ASSET} (expected ${EXPECTED}, got ${ACTUAL})" >&2
      exit 1
    fi
    echo "Verified SHA-256 checksum."
  fi
fi

mkdir -p "$INSTALL_DIR"
STAGE_DIR="$(mktemp -d "${INSTALL_DIR}/.slackless-install.XXXXXX")"
trap 'rm -rf "$TMPDIR" "$STAGE_DIR"' EXIT

tar -xzf "${TMPDIR}/${APP_ASSET}" -C "$STAGE_DIR"
if [[ ! -x "${STAGE_DIR}/Slackless.app/Contents/MacOS/slackless-gui" ]]; then
  echo "error: extracted archive does not contain Slackless.app" >&2
  exit 1
fi

# Ensure no quarantine attribute is set and stop any currently running instance cleanly.
xattr -dr com.apple.quarantine "${STAGE_DIR}/Slackless.app" 2>/dev/null || true
if pgrep -x slackless-gui >/dev/null 2>&1; then
  echo "Stopping running Slackless..."
  pkill -x slackless-gui 2>/dev/null || true
  for _ in $(seq 1 40); do
    pgrep -x slackless-gui >/dev/null 2>&1 || break
    sleep 0.1
  done
fi

DEST="${INSTALL_DIR}/Slackless.app"
rm -rf "${DEST}.prev"
[[ -d "$DEST" ]] && mv "$DEST" "${DEST}.prev"
if mv "${STAGE_DIR}/Slackless.app" "$DEST"; then
  rm -rf "${DEST}.prev"
else
  [[ -d "${DEST}.prev" ]] && mv "${DEST}.prev" "$DEST"
  echo "error: failed to install Slackless.app into ${INSTALL_DIR}" >&2
  exit 1
fi

LSREG="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [[ -x "$LSREG" ]]; then
  "$LSREG" -f "$DEST" >/dev/null 2>&1 || true
fi

echo "Installed Slackless ${TAG} -> ${DEST}"

if [[ "$INSTALL_CLI" -eq 1 ]]; then
  mkdir -p "$HOME/.local/bin"
  if [[ -x "${DEST}/Contents/MacOS/slackless" ]]; then
    ln -sf "${DEST}/Contents/MacOS/slackless" "$HOME/.local/bin/slackless"
    echo "Linked slackless CLI -> $HOME/.local/bin/slackless (matches ${TAG})"
  elif [[ -n "$CLI_URL" ]]; then
    echo "Downloading slackless CLI ${TAG}..."
    curl -fsSL -o "${TMPDIR}/${CLI_ASSET}" "$CLI_URL"
    if [[ -f "${TMPDIR}/SHA256SUMS.txt" ]]; then
      EXPECTED_CLI="$(awk -v f="$CLI_ASSET" '$2 == f { print $1; exit }' "${TMPDIR}/SHA256SUMS.txt")"
      if [[ -n "$EXPECTED_CLI" ]]; then
        ACTUAL_CLI="$(shasum -a 256 "${TMPDIR}/${CLI_ASSET}" | awk '{print $1}')"
        if [[ "$ACTUAL_CLI" != "$EXPECTED_CLI" ]]; then
          echo "error: SHA-256 mismatch for ${CLI_ASSET}" >&2
          exit 1
        fi
      fi
    fi
    tar -xzf "${TMPDIR}/${CLI_ASSET}" -C "$TMPDIR"
    install -m 755 "${TMPDIR}/slackless" "$HOME/.local/bin/slackless"
    echo "Installed slackless CLI -> $HOME/.local/bin/slackless"
  fi
  case ":${PATH:-}:" in
    *":$HOME/.local/bin:"*) ;;
    *)
      echo "Note: add $HOME/.local/bin to your PATH to run 'slackless' from the terminal:"
      echo '  export PATH="$HOME/.local/bin:$PATH"'
      ;;
  esac
fi

echo "Launch with: open \"${DEST}\""
