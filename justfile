export MACOSX_DEPLOYMENT_TARGET := env("MACOSX_DEPLOYMENT_TARGET", "13.0")

# List available recipes
default:
    @just --list

# Run all checks: formatting, clippy, workspace tests, and workspace build
validate:
    cargo fmt --all -- --check --config style_edition=2024
    cargo clippy --workspace -- -D warnings
    cargo clippy --workspace --tests -- -D warnings
    cargo test --workspace
    cargo build --workspace

# Run the workspace test suite (extra args forwarded to `cargo test`)
test *args:
    cargo test --workspace {{args}}

# Build and run the GUI in release mode
run *args:
    cargo run --release -p slackless-gui -- {{args}}

# Install release binaries (`slackless-gui` and `slackless`) via cargo
install:
    cargo install --path apps/slackless-gui --locked
    cargo install --path apps/slackless-cli --locked
