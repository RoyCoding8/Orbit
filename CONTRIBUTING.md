# Contributing to Orbit

Orbit uses React 19 and TypeScript for its interface (`src/`), Rust with Tauri v2 for its desktop backend (`src-tauri/`), and a standard-library-only crate (`orbit-core/`) for deterministic production rules.

## Development setup

Install Node.js 22+, stable Rust (1.82+), and the [Tauri v2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Install the agent CLIs needed for the features you work on.

```sh
npm ci
npm run tauri dev
```

The development server runs at `http://127.0.0.1:1420`. For release packages, see [Building Release Packages](README.md#building-release-packages).

## Repository layout

- `src/`: Frontend source organized into `components/`, `lib/`, `pages/`, and `styles/`
- `tests/frontend/`: Vitest + Testing Library suites and `test-setup.ts`
- `src-tauri/`: Tauri v2 backend (`src/harness/`, `src/transcript/`, `src/transfer/`, `schemas/`)
- `orbit-core/`: Dependency-free pure-Rust invariants shared by `src-tauri/` and `formal/`
- `formal/`: Kani safety harnesses and a TLA+ journal model ([verification guide](formal/README.md))

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
cargo test --manifest-path orbit-core/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

Changes to core rules or journal lifecycle behavior should also pass the formal checks. Use the tool versions and TLC download command in the [verification guide](formal/README.md).

```sh
cargo fmt --manifest-path formal/Cargo.toml --check
cargo clippy --manifest-path formal/Cargo.toml -- -D warnings
bash formal/run.sh
bash formal/run-tlc.sh /path/to/tla2tools.jar
```

GitHub Actions (`.github/workflows/ci.yml`) runs frontend and Rust checks on Ubuntu, macOS, and Windows, plus Kani and TLC on Ubuntu.

The manually dispatched Release workflow (`.github/workflows/release.yml`) builds packages for Windows (x64, arm64), Linux (x64, arm64), and macOS (arm64, x64) and creates a draft GitHub Release using the version in `src-tauri/tauri.conf.json`, with an optional `prerelease` input (defaults to `false`). When `TAURI_SIGNING_PRIVATE_KEY` (and optional `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) repository secrets are configured, it signs updater artifacts and generates `latest.json` for in-app updates alongside `SHA256SUMS.txt`.

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
