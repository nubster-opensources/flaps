#!/usr/bin/env bash
#
# scripts/cargo-publish-idempotent.sh
#
# Wrapper around `cargo publish` that treats "already exists" as success.
# This makes the release workflow re-runnable without manual intervention
# when a previous run published some crates before failing.
#
# Usage:
#   cargo-publish-idempotent.sh <crate-name> [cargo publish args...]
#
set -euo pipefail

CRATE="${1:?usage: $0 <crate-name> [cargo publish args...]}"
shift

output=$(cargo publish -p "$CRATE" "$@" 2>&1) && { printf '%s\n' "$output"; exit 0; }
exit_code=$?

printf '%s\n' "$output"

if grep -q "already exists" <<< "$output"; then
  echo "[$CRATE] already published at this version, skipping."
  exit 0
fi

exit $exit_code
