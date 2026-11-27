#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then
  echo "Usage: bash formal/run-tlc.sh /path/to/tla2tools.jar" >&2
  exit 2
fi

jar="$(realpath "$1")"
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
printf '%s  %s\n' 936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88 "$jar" | sha256sum --check --status
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
java -cp "$jar" tlc2.TLC -workers 1 -metadir states -config "$dir/tla/Ledger.cfg" "$dir/tla/Ledger.tla"
