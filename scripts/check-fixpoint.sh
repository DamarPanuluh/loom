#!/usr/bin/env bash
# Verify the Journey-root dogfood fixture can be rebuilt from a fresh v13
# store. Older exports are an intentional hard boundary for this gate, never a migration
# source.  Runs in a copy of the worktree so neither the live .loom/ nor the
# source tree's target/ directory is touched.
#
# Usage: scripts/check-fixpoint.sh [workdir]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
  -h|--help)
    echo "usage: scripts/check-fixpoint.sh [empty-workdir]"
    exit 0
    ;;
esac
WORK="${1:-$(mktemp -d "${TMPDIR:-/tmp}/loom-fixpoint.XXXXXX")}"
CREATED=false
[ -d "$WORK" ] || mkdir -p "$WORK"
if [ -z "${1:-}" ]; then
  CREATED=true
fi
trap '[ "$CREATED" = true ] && rm -rf "$WORK"' EXIT

if [ -e "$WORK/.loom" ] || [ -n "$(find "$WORK" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
  echo "fixpoint: workdir must be empty and must not contain .loom/: $WORK" >&2
  exit 2
fi

SNAPSHOTTER="$ROOT/target/debug/loom"
[ -f "$SNAPSHOTTER" ] && [ ! -L "$SNAPSHOTTER" ] && [ -x "$SNAPSHOTTER" ] || {
  echo "fixpoint: trusted snapshot adapter is missing at $SNAPSHOTTER; build it explicitly before starting the gate" >&2
  exit 2
}
command -v shasum >/dev/null || {
  echo "fixpoint: cannot attest the trusted snapshot adapter without shasum" >&2
  exit 2
}
snapshotter_sha256="$(shasum -a 256 "$SNAPSHOTTER" | awk '{print $1}')"
echo "fixpoint: snapshot_adapter_provenance=existing_target_binary snapshot_adapter_sha256=$snapshotter_sha256" >&2
SNAPSHOT_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
snapshot="$(env -i PATH="$SNAPSHOT_PATH" TMPDIR="${TMPDIR:-/tmp}" "$SNAPSHOTTER" --graph "$ROOT" --json release snapshot --destination "$WORK")"
python3 -c '
import json, sys
report = json.load(sys.stdin)
expected = json.load(open(sys.argv[1]))
inventory = report["source_inventory"]
assert report["schema"] == expected["schema"]
assert report["status"] == expected["status"]
assert expected["candidate_hash_relation"] == "source_inventory.inventory_hash"
assert report["candidate_hash"] == inventory["inventory_hash"]
for field, value in expected["source_inventory"].items():
    assert inventory[field] == value, (field, inventory[field], value)
' "$ROOT/release/snapshot-expectation.json" <<<"$snapshot"

echo "== v13 fresh-graph fixpoint =="
"$WORK/scripts/dogfood.sh" --fresh-in-place --check
echo "fixpoint: OK — a fresh v13 graph completed the structural Journey-root dogfood gate (cold-import integrity only; profiles not executed; not graph-maturity green)"
