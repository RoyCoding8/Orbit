# Contributing to Orbit

Orbit uses React and TypeScript for its interface and Rust with Tauri v2 for its desktop backend.

## Development setup

Install Node.js 22, stable Rust, and the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Install the agent CLIs needed for the features you work on.

```sh
npm ci
npm run tauri dev
```

The development server uses `http://127.0.0.1:1420`. For release packages, see [Build](README.md#build).

## Platform setup

### Windows

Windows builds require WebView2 and a compiler matching the Rust toolchain:

- **GNU:** `stable-x86_64-pc-windows-gnu`, with MinGW-w64 GCC and G++ on `PATH`.
- **MSVC:** a Rust MSVC toolchain and Microsoft C++ Build Tools.

Windows CI builds with MSVC; local GNU builds are also supported.

### Linux

On Ubuntu, install the native build dependencies:

```sh
sudo apt-get update
sudo apt-get install --yes build-essential curl wget file \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev libxdo-dev \
  libssl-dev librsvg2-dev patchelf
```

### macOS

Install Xcode Command Line Tools:

```sh
xcode-select --install
```

## Checks

```sh
npm run build
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

GitHub Actions runs the frontend and Rust checks on Ubuntu, macOS, and Windows for pushes and pull requests. The manually dispatched Release workflow builds platform packages and creates a draft prerelease using the version in `src-tauri/tauri.conf.json`.

## Native integration tests

These optional tests require the corresponding agent CLI on `PATH`. They use temporary homes and session stores and do not send model prompts.

OpenCode v2 archive import and export:

```sh
cargo test --manifest-path src-tauri/Cargo.toml native_v2_cli_accepts_isolated_api_fixture_and_roundtrips_archive_collision --lib -- --ignored --nocapture
```

Codex, Pi, and OpenCode conversion:

```sh
cargo test --manifest-path src-tauri/Cargo.toml transcript::conversion::tests::native_ --lib -- --ignored --nocapture
```

Antigravity native import, deletion, restoration, and conversion require `agy` 1.5.0. Close the Antigravity App and CLI sessions before running:

```sh
cargo test --manifest-path src-tauri/Cargo.toml live_isolated_native_import_replay_delete_restore_and_incoming_conversion --lib -- --ignored --nocapture
```

Agent format support and feature limits are listed in the [reference](docs/reference.md#agent-support).
