#!/usr/bin/env bash
# Drive Loom over its own committed, living graph.
#
# This script deliberately does not seed intents, replay verdicts, waive gaps,
# or delete `.loom/`. Asserted truth is durable memory and must be earned by an
# operator through `loom next`; a shell script may only recompute derived facts
# and run machine-observable proofs.
#
# Usage:
#   scripts/dogfood.sh          # update local derived facts/proofs, then gate
#   scripts/dogfood.sh --check  # same, but also fail if loom.graph.json changed
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
B="$ROOT/target/debug/loom"
CHECK=false
[ "${1:-}" = "--check" ] && CHECK=true
export_before=""
if $CHECK; then
  export_before="missing"
  [ ! -f loom.graph.json ] || export_before="$(cksum <loom.graph.json)"
fi

echo "== code gates =="
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --quiet
cargo build --quiet
export PATH="$(dirname "$B"):$PATH"

# A clean checkout carries the portable export, not the local SQLite store.
# Restore it once, then continue incrementally from that durable memory.
if [ ! -f .loom/graph.sqlite ]; then
  [ -f loom.graph.json ] || {
    echo "dogfood: neither .loom/graph.sqlite nor loom.graph.json exists" >&2
    exit 1
  }
  "$B" init . --name loom >/dev/null
  "$B" import loom.graph.json >/dev/null
fi

echo "== structural sync =="
"$B" sync

echo "== pending command proofs =="
"$B" validation run --all

echo "== repository journeys =="
for spec in journeys/*.yaml; do
  "$B" journey run "$spec"
done

echo "== integrity =="
"$B" doctor
"$B" coverage

# Sync and proof writes refresh an already tracked export. Export explicitly at
# closeout as well so the projection gate is understandable in isolation.
"$B" export
"$B" export --check

status_json=$("$B" status --json)
python3 - "$status_json" <<'PY'
import json
import sys

status = json.loads(sys.argv[1])
unmet = [
    f"{r['name']}: {r['detail']}"
    for r in status["maturity"]["rungs"]
    if r["state"] != "met"
]
queued = {name: count for name, count in status["queues"].items() if count}
if unmet or queued:
    if unmet:
        print("dogfood: maturity gaps remain:", file=sys.stderr)
        for gap in unmet:
            print(f"  - {gap}", file=sys.stderr)
    if queued:
        print(f"dogfood: routed work remains: {queued}", file=sys.stderr)
    print("dogfood: inspect the next packet with `loom next --json`", file=sys.stderr)
    raise SystemExit(1)
print("dogfood: complete — every maturity rung is met and every queue is empty")
PY

if $CHECK; then
  export_after="missing"
  [ ! -f loom.graph.json ] || export_after="$(cksum <loom.graph.json)"
  if [ "$export_before" != "$export_after" ]; then
    echo "dogfood: loom.graph.json changed during restore/sync/proof; commit the fresh export" >&2
    exit 1
  fi
fi
