<p align="center">
  <img src="apps/slackless-gui/icon.png" width="128" alt="Slackless icon">
</p>

<h1 align="center">Slackless</h1>

<p align="center">A fast, lightweight native Slack client for macOS, written in Rust with GPUI.</p>

<p align="center"><sub>Linux and Windows support may come in the future.</sub></p>

## Run

```sh
nix develop   # or: direnv allow
cargo run --release -p slackless-gui
```

The first launch opens Slack's sign-in window; later launches reuse the stored session.
