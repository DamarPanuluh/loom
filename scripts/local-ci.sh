#!/usr/bin/env bash
# The developer-machine CI gate. Loom's opt-in pre-push hook executes this
# script, so repository checks run before code reaches the remote and
# GitHub-hosted minutes are not required.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== build trusted local Loom adapter =="
cargo build --quiet

echo "== Loom local CI =="
scripts/dogfood.sh --check
