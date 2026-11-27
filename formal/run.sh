#!/usr/bin/env bash
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export LC_ALL=C

if (( $# )); then
  exec cargo kani --manifest-path "$dir/Cargo.toml" "$@"
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
cargo kani --manifest-path "$dir/Cargo.toml" --output-format terse 2>&1 | tee "$log"

expected="$dir/kani-harnesses.txt"
diff -u "$expected" <(sed -nE 's/^(Thread [0-9]+: )?Checking harness ([^ ]+)\.\.\.$/\2/p' "$log" | sort)
count="$(wc -l < "$expected" | tr -d '[:space:]')"
if (( count == 0 )) || ! grep -Fxq "Complete - $count successfully verified harnesses, 0 failures, $count total." "$log"; then
  echo "Kani did not verify all $count expected harnesses." >&2
  exit 1
fi
