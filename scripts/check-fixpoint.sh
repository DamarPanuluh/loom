#!/usr/bin/env bash
# Verify the Journey-root dogfood fixture can be rebuilt from a fresh v12
# store.  v1-v11 exports are an intentional hard boundary, never a migration
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
inventory = report["source_inventory"]
assert report["schema"] == "loom.release-snapshot/v1"
assert report["status"] == "passed"
assert report["candidate_hash"] == inventory["inventory_hash"]
assert inventory["schema"] == "loom.release-source-inventory-attestation/v1"
assert inventory["manifest_hash"] == "1e7f61e0ec084423"
assert inventory["git_influenced_plan"] is False
assert inventory["materialized_matches"] is True
assert inventory["provenance"] == "source_controlled_manifest_git_verified"
assert inventory["git_verification"] == "verified"
assert inventory["entry_count"] == 261
assert inventory["file_count"] == 257
assert inventory["tombstone_count"] == 4
' <<<"$snapshot"

echo "== v12 fresh-graph fixpoint =="
"$WORK/scripts/dogfood.sh" --fresh-in-place --check
echo "fixpoint: OK — a fresh v12 graph completed the Journey-root dogfood gate"
