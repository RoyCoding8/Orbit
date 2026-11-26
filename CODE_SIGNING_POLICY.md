# Code Signing Policy

## Scope and Status

Orbit (`https://github.com/RoyCoding8/Orbit`) is an open-source desktop application licensed under the Apache License 2.0.

Orbit is applying to the [SignPath Foundation](https://signpath.org/) open-source code signing program for Windows release installers. Until that application is approved and integrated into `.github/workflows/release.yml`, Windows installers are distributed unsigned. Once approved, covered Windows release artifacts will use free code signing provided by SignPath.io with a certificate by SignPath Foundation.

## Privacy and Network Behavior

Orbit operates locally on the user's machine to read and edit configuration, skills, memories, hooks, MCP server definitions, and chat archives for supported AI coding agents.

Orbit does not collect telemetry or transmit user data to third-party servers. Outbound network connections occur only when:

- The user or app checks for updates against GitHub Releases (`https://github.com/RoyCoding8/Orbit/releases/latest/download/latest.json`) via `tauri-plugin-updater`.
- The user explicitly runs an MCP server connection test against a user-configured endpoint.

## Roles and Security Controls

- **Committers / Reviewers:** `@RoyCoding8` (Shashwata Roy)
- **Approvers:** `@RoyCoding8` (Shashwata Roy)

All accounts with write access to the repository, release workflow, or SignPath dashboard use multi-factor authentication (MFA). External pull requests require maintainer review before merge.

## Build Provenance and Signing Process

- Only release artifacts built from this repository by the GitHub Actions workflow (`.github/workflows/release.yml`) are eligible for code signing.
- Local builds, pull request builds, and third-party binaries are never submitted for publisher code signing.
- Every production signing request requires explicit maintainer approval before signing completes.
- Published GitHub Releases include `SHA256SUMS.txt` checksums for all release assets.
- Tauri in-app updater archives use a separate Ed25519 signature (`.sig` and `latest.json`) to verify update integrity. When Windows Authenticode signing is active, Authenticode signing runs before updater `.sig` generation so the updater signature always matches the final signed installer bytes.
