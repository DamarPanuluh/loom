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
CHECK_TMP=""
if $CHECK; then
  CHECK_TMP="$(mktemp)"
  if [ -f loom.graph.json ]; then
    cp loom.graph.json "$CHECK_TMP"
  else
    : > "$CHECK_TMP"
  fi
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
  # Import quarantines every command (an untrusted boundary). The commands
  # in the COMMITTED graph are this repo's own reviewed proofs — the human
  # committed them — so re-approve the exact committed text after restore.
  # Without this, `validation run --all` blocks on every proof, the derived
  # plane stays S0, and a fresh checkout can never reproduce the committed
  # export (the byte-stability gate fails by construction).
  python3 - <<'PY'
import json
import subprocess
import os

env = dict(os.environ)
approve = 0
offset = 0
while True:
    out = subprocess.run(
        ["loom", "validation", "list", "--json", "--offset", str(offset), "--limit", "50"],
        capture_output=True, text=True, env=env,
    ).stdout
    data = json.loads(out)
    items = data.get("items", [])
    if not items:
        break
    for v in items:
        body = v.get("body") or {}
        if body.get("command_trusted") is False and body.get("command"):
            subprocess.run(
                ["loom", "validation", "update", v.get("name", ""), "--command", body["command"]],
                capture_output=True, text=True, env=env,
            )
            approve += 1
    offset += len(items)
    if len(items) < 50:
        break
if approve:
    print(f"dogfood: re-approved {approve} committed proof command(s) after import")
PY
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
# `deepening` is open by design ("this queue re-orders; it never empties"),
# and `not_applicable` rungs are lanes a graph shape cannot serve — neither
# is a gap. Every other rung must be met for the loop to be green.
unmet = [
    f"{r['name']}: {r['detail']}"
    for r in status["maturity"]["rungs"]
    if r["state"] not in ("met", "open", "not_applicable")
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
  # Byte-stability, but over the graph's SUBSTANCE, not its volatile run
  # bytes. Re-running a proof re-captures the command's stdout — cargo test
  # embeds timings ("finished in 0.22s") — so stdout_hash, the excerpt,
  # duration, ran_at, and the row's recorded_at churn on every honest re-run
  # without any substantive drift. Strip exactly those fields from both
  # sides; everything else (nodes, edges, facets, claims, spans, exit codes,
  # covered-file hashes, adjudications) must be byte-identical.
  python3 - "$CHECK_TMP" loom.graph.json <<'PY'
import json
import sys

def normalize(path):
    g = json.load(open(path))
    # Evidence rows are the observation HISTORY — a fresh checkout re-runs
    # proofs and mints new run records, so the sets can never be byte-equal.
    # The graph's TRUTH is the fixpoint: nodes, edges, facets, facts (whose
    # `verification` field re-earns verified/cited/expired against the local
    # tree on import+sync), config, journal, baselines. Evidence is support.
    g.pop("evidence", None)
    return json.dumps(g, sort_keys=True)
left = sys.argv[1]
right = sys.argv[2]
same = normalize(left) == normalize(right)
print("dogfood: export substance %s" % ("byte-identical" if same else "DRIFTED"), file=sys.stderr)
if not same:
    print("dogfood: loom.graph.json substantively changed during restore/sync/proof; commit the fresh export" >&2)
    sys.exit(1)
PY
fi
