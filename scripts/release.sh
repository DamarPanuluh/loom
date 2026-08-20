#!/usr/bin/env bash
# The release rule: the ONE way to cut a new loom version. Never hand-edit the
# version in Cargo.toml — run this. It bumps per semver, refuses to ship unless
# fmt/clippy/tests are clean, records a CHANGELOG entry, installs to global, and
# verifies. Safe to re-run after fixing a failing gate (nothing changes until
# the gates pass).
#
#   scripts/release.sh <patch|minor|major> "<one-line summary>" [--dry-run]
#
# semver: patch = compatible bug fix. Before 1.0, minor may deliberately break
# an unstable surface (for example 0.29.x -> 0.30.0); at/after 1.0, minor is
# backward-compatible and major is breaking. SCHEMA_VERSION in src/lib.rs is
# separate and versions the on-disk graph.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

level="${1:-}"
summary="${2:-}"
dry_run=false
for a in "$@"; do [ "$a" = "--dry-run" ] && dry_run=true; done

case "$level" in
  patch|minor|major) ;;
  *) echo "usage: scripts/release.sh <patch|minor|major> \"<summary>\" [--dry-run]" >&2; exit 2 ;;
esac
if [ -z "$summary" ] || [ "$summary" = "--dry-run" ]; then
  echo "error: a one-line summary is required (it goes in the CHANGELOG)" >&2; exit 2
fi

# current version = the first top-level `version = "..."` in Cargo.toml
cur="$(grep -m1 '^version = "' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')"
IFS='.' read -r vmaj vmin vpat <<<"$cur"
case "$level" in
  patch) vpat=$((vpat+1)) ;;
  minor) vmin=$((vmin+1)); vpat=0 ;;
  major) vmaj=$((vmaj+1)); vmin=0; vpat=0 ;;
esac
new="${vmaj}.${vmin}.${vpat}"

echo "loom release: $cur -> $new ($level)"
echo "  summary: $summary"
if $dry_run; then echo "  (dry-run: gates run in isolated copies; no files changed or installed)"; fi

# Gate: nothing ships unless code is green and the Journey-root dogfood graph
# can be rebuilt without ever opening/migrating the repository's legacy v11
# .loom.  Both scripts copy the worktree and fail closed at an outstanding
# human derivation/surface approval; a release must not bypass that authority.
echo "== release gates =="
scripts/dogfood.sh --check
scripts/check-fixpoint.sh

if $dry_run; then
  echo "dry-run: all release gates passed; no files changed or installed"
  exit 0
fi

# Bump the package version (first `^version = "..."`; dependency versions are
# `dep = { version = ... }`, never at line start, so they are untouched).
awk -v v="$new" '!done && /^version = "/ { sub(/"[^"]+"/, "\"" v "\""); done=1 } { print }' \
  Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
cargo build --quiet   # refresh Cargo.lock

# Prepend the CHANGELOG entry above the most recent one.
awk -v ver="$new" -v day="$(date +%Y-%m-%d)" -v sum="$summary" '
  !done && /^## \[/ { printf "## [%s] - %s\n- %s\n\n", ver, day, sum; done=1 }
  { print }' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md

echo "== install =="
cargo install --path . --force
echo "installed: $("$HOME/.cargo/bin/loom" --version)"
echo "done: loom $new — remember to review CHANGELOG.md"
echo "to publish GitHub Release binaries, tag v$new and push it (the workflow publishes on v* tags)"
