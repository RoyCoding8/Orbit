# Contributing to Orbit

Orbit uses React 19 and TypeScript for its interface (`src/`), Rust with Tauri v2 for its desktop backend (`src-tauri/`), a pure-`std` shared crate (`orbit-core/`) for security and state invariants, and Kani 0.68.0 (`formal/`) for formal verification.

## Development setup

Install Node.js 22+, stable Rust (1.82+), and the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Install the agent CLIs needed for the features you work on.

```sh
npm ci
npm run tauri dev
```

The development server runs at `http://127.0.0.1:1420`. For release packages, see [Building Release Packages](README.md#building-release-packages).

## Repository layout

- `src/` — Frontend source organized into `components/`, `lib/`, `pages/`, and `styles/`
- `tests/frontend/` — Vitest + Testing Library suites and `test-setup.ts`
- `src-tauri/` — Tauri v2 backend (`src/harness/`, `src/transcript/`, `src/transfer/`, `schemas/`)
- `orbit-core/` — Dependency-free pure-Rust invariants shared by `src-tauri/` and `formal/`
- `formal/` — Kani 0.68.0 proof harnesses (`formal/run.sh`, `formal/README.md`)

## Platform setup

### Windows

Windows builds require WebView2 and a compiler matching the Rust toolchain:

- **GNU:** `stable-x86_64-pc-windows-gnu`, with MinGW-w64 GCC and G++ on `PATH`.
- **MSVC:** a Rust MSVC toolchain and Microsoft C++ Build Tools.

Windows CI builds with MSVC; local GNU builds are also supported.

### Linux

On Ubuntu/Debian, install the native build dependencies:

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

Run all frontend and Rust gates before opening a pull request:

```sh
npm run build
npm test
cargo fmt --manifest-path orbit-core/Cargo.toml --check
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path orbit-core/Cargo.toml -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

If you modify pure invariants in `orbit-core/` or `formal/`, also run the Kani formal verification suite (requires [Kani 0.68.0](https://model-checking.github.io/kani/)):

```sh
cargo fmt --manifest-path formal/Cargo.toml --check
cargo clippy --manifest-path formal/Cargo.toml -- -D warnings
./formal/run.sh
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
