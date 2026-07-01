#!/usr/bin/env bash
# The release rule: the ONE way to cut a new loom version. Never hand-edit the
# version in Cargo.toml — run this. It bumps per semver, refuses to ship unless
# fmt/clippy/tests are clean, records a CHANGELOG entry, installs to global, and
# verifies. Safe to re-run after fixing a failing gate (nothing changes until
# the gates pass).
#
#   scripts/release.sh <patch|minor|major> "<one-line summary>" [--dry-run]
#
# semver: patch = bug fix, minor = backward-compatible feature, major = breaking.
# (SCHEMA_VERSION in src/lib.rs is a SEPARATE thing — the on-disk graph schema.)
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
if $dry_run; then echo "  (dry-run: no gates run, no files changed, nothing installed)"; exit 0; fi

# Gate: nothing ships unless the tree is clean and green.
echo "== gates =="
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --quiet

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
