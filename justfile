export MACOSX_DEPLOYMENT_TARGET := env("MACOSX_DEPLOYMENT_TARGET", "13.0")

# List available recipes
default:
    @just --list

# Run all checks: formatting, clippy, supply-chain audit, workspace tests, and workspace build
validate:
    cargo fmt --all -- --check
    cargo clippy --workspace -- -D warnings
    cargo clippy --workspace --tests -- -D warnings
    cargo deny check
    cargo test --workspace
    cargo build --workspace

# Run the workspace test suite (extra args forwarded to `cargo test`)
test *args:
    cargo test --workspace {{args}}

# Measure real LLVM line/function coverage for one crate (optional file path filter)
coverage crate="slackless-core" filter="":
    ./scripts/coverage.sh {{crate}} {{filter}}

# Run the release micro-benchmarks (store, huddle audio/video, GUI messages)
bench:
    cargo test --release -p slackless-store -p slackless-huddle -p slackless-gui -- bench --ignored --nocapture

# Build and run the GUI in release mode
run *args:
    cargo run --release -p slackless-gui -- {{args}}

# Install release binaries (`slackless-gui` and `slackless`) via cargo
install:
    cargo install --path apps/slackless-gui --locked
    cargo install --path apps/slackless-cli --locked
