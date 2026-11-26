#!/usr/bin/env bash
# Measure real LLVM line/function coverage for one or more crates using the active rustc sysroot's
# `llvm-profdata` and `llvm-cov` (no external cargo-llvm-cov installation required).
#
# Usage:
#   ./scripts/coverage.sh slackless-huddle [file-filter-regex]
#   ./scripts/coverage.sh slackless-core [file-filter-regex]
#   ./scripts/coverage.sh slackless-store [file-filter-regex]
#   ./scripts/coverage.sh slackless-gui [file-filter-regex]

set -euo pipefail

CRATE="${1:-slackless-huddle}"
FILTER="${2:-}"

SYSROOT="$(rustc --print sysroot)"
LLVM_BIN="$(find "$SYSROOT/lib/rustlib" -maxdepth 2 -type d -name bin | head -n 1)"
LLVM_PROFDATA="$LLVM_BIN/llvm-profdata"
LLVM_COV="$LLVM_BIN/llvm-cov"

if [[ ! -x "$LLVM_COV" || ! -x "$LLVM_PROFDATA" ]]; then
  echo "error: llvm-cov / llvm-profdata not found in rustc sysroot ($SYSROOT)" >&2
  exit 1
fi

COV_DIR="target/cov/$CRATE"
rm -rf "$COV_DIR"
mkdir -p "$COV_DIR"

export RUSTFLAGS="-C instrument-coverage"
export LLVM_PROFILE_FILE="$PWD/$COV_DIR/prof-%p-%m.profraw"

echo "==> Running instrumented tests for $CRATE..."
BINARIES=()
while IFS= read -r line; do
  if [[ -n "$line" ]]; then
    BINARIES+=("$line")
  fi
done < <(
  cargo test --profile ci-test --target-dir target/cov-build -p "$CRATE" --no-run --message-format=json \
    | python3 -c '
import json, sys
for raw in sys.stdin:
    try:
        msg = json.loads(raw)
    except Exception:
        continue
    if msg.get("reason") == "compiler-artifact" and msg.get("profile", {}).get("test") and msg.get("executable"):
        print(msg["executable"])
'
)

cargo test --profile ci-test --target-dir target/cov-build -p "$CRATE" --quiet

"$LLVM_PROFDATA" merge -sparse "$COV_DIR"/*.profraw -o "$COV_DIR/merged.profdata"

OBJECTS=()
for bin in "${BINARIES[@]}"; do
  OBJECTS+=(--object "$bin")
done

echo "==> Coverage report for $CRATE${FILTER:+ (filter: $FILTER)}:"
REPORT="$("$LLVM_COV" report \
  --use-color=false \
  --ignore-filename-regex='(/\.cargo/|/nix/store/|/rustc/|/tests/|/tests\.rs$)' \
  --instr-profile="$COV_DIR/merged.profdata" \
  "${OBJECTS[@]}")"

if [[ -n "$FILTER" ]]; then
  printf '%s\n' "$REPORT" | awk -v pat="$FILTER" 'NR<=2 || $0 ~ pat || /^---/ || /^TOTAL/'
else
  printf '%s\n' "$REPORT"
fi
