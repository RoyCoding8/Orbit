#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export PATH="/usr/local/cargo/bin:${HOME}/.cargo/bin:${PATH}"

# Fallback TMPDIR when /tmp is mounted noexec or unavailable.
if [[ -z "${TMPDIR:-}" ]]; then
  if [[ -d "${HOME}/tmp" ]] || mkdir -p "${HOME}/tmp" 2>/dev/null; then
    export TMPDIR="${HOME}/tmp"
  fi
else
  mkdir -p "${TMPDIR}"
fi

if ! command -v cargo-kani >/dev/null 2>&1; then
  echo "ERROR: cargo-kani not found in PATH (${PATH})." >&2
  exit 1
fi

EXTRA_ARGS=()
HAS_FORMAT=0
for arg in "$@"; do
  if [[ "${arg}" == "--output-format" || "${arg}" == --output-format=* ]]; then
    HAS_FORMAT=1
  fi
done
if [[ ${HAS_FORMAT} -eq 0 ]]; then
  EXTRA_ARGS+=(--output-format terse)
fi

echo "==> Running Kani formal verification suite (${SCRIPT_DIR}/Cargo.toml)"
echo "    $(cargo kani --version)"
echo "    TMPDIR=${TMPDIR:-<default>}"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  echo "    CARGO_TARGET_DIR=${CARGO_TARGET_DIR}"
fi

set +e
OUTPUT="$(cargo kani --manifest-path "${SCRIPT_DIR}/Cargo.toml" "${EXTRA_ARGS[@]}" "$@" 2>&1)"
STATUS=$?
set -e

printf '%s\n' "${OUTPUT}"

SUMMARY="$(printf '%s\n' "${OUTPUT}" | grep -E '^Complete - [0-9]+ successfully verified harnesses, [0-9]+ failures' || true)"
if [[ -n "${SUMMARY}" ]]; then
  echo "==> Summary: ${SUMMARY}"
fi

if [[ ${STATUS} -ne 0 ]]; then
  echo "==> FAILED: Kani verification exited with status ${STATUS}" >&2
  exit "${STATUS}"
fi

echo "==> PASSED: All Kani proof harnesses verified successfully."
