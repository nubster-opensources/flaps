#!/usr/bin/env bash
#
# Verify that the project identity is consistent across its three surfaces:
#   - the GitHub About description (repository metadata),
#   - the README one-liner (the blockquote line starting with "> "),
#   - the description of the crate that shares the repository name.
#
# This is the single largest source of drift between a repository and the
# OSS convention, so it is enforced in CI rather than left to review.
#
# Requires: gh (with GH_TOKEN), cargo, jq. Runs on GitHub-hosted runners.

set -euo pipefail

readme="README.md"

if [ ! -f "$readme" ]; then
  echo "::error::$readme not found."
  exit 1
fi

# README one-liner: first line starting with "> ", prefix and trailing space stripped.
oneliner="$(grep -m1 '^> ' "$readme" | sed 's/^> *//' | sed 's/[[:space:]]*$//' || true)"
if [ -z "$oneliner" ]; then
  echo "::error::No README one-liner found (expected a line starting with '> ')."
  exit 1
fi

# On an ungenerated template the one-liner still holds a placeholder; skip.
case "$oneliner" in
  *'{{'*)
    echo "Template placeholder detected in the README one-liner; skipping identity coherence check."
    exit 0
    ;;
esac

repo="${GITHUB_REPOSITORY##*/}"

# About description from the repository metadata.
about="$(gh api "repos/${GITHUB_REPOSITORY}" --jq '.description // ""')"

# Canonical crate description: the crate named like the repository, or the sole
# package when the workspace has exactly one. Empty means "cannot disambiguate".
metadata="$(cargo metadata --format-version 1 --no-deps)"
crate_description="$(printf '%s' "$metadata" | jq -r --arg name "$repo" '
  (.packages[] | select(.name == $name) | .description)
  // (if (.packages | length) == 1 then .packages[0].description else empty end)
  // ""')"

failed=0

if [ "$about" != "$oneliner" ]; then
  echo "::error::About description and README one-liner differ."
  echo "  About:  ${about}"
  echo "  README: ${oneliner}"
  failed=1
fi

if [ -n "$crate_description" ]; then
  if [ "$crate_description" != "$oneliner" ]; then
    echo "::error::Crate description ('${repo}') and README one-liner differ."
    echo "  Cargo:  ${crate_description}"
    echo "  README: ${oneliner}"
    failed=1
  fi
else
  echo "::warning::No crate named '${repo}' and more than one package in the workspace; skipped the crate description comparison."
fi

if [ "$failed" -ne 0 ]; then
  exit 1
fi

if [ -n "$crate_description" ]; then
  echo "Identity coherence OK: About, README one-liner and crate description all match."
else
  echo "Identity coherence OK: About and README one-liner match."
fi
