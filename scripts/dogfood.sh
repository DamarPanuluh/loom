#!/usr/bin/env bash
# Exercise Loom against an isolated, journey-root graph.
#
# Schema v12 intentionally does not translate the repository's v11 graph:
# executable legacy journeys cannot supply the human-authored meaning required
# by Journey roots.  The normal entry point therefore copies the worktree,
# excluding .loom/, and only ever initializes/imports inside that copy.
#
# Usage:
#   scripts/dogfood.sh          # safe fresh-v12 dogfood gate
#   scripts/dogfood.sh --check  # same gate, suitable for CI/release
#
# `--fresh-in-place` is internal: check-fixpoint.sh calls it only after making
# an isolated worktree.  It refuses a pre-existing .loom/ directory so it
# cannot accidentally be used to migrate or modify the live v11 graph.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK=false
IN_PLACE=false

for arg in "$@"; do
  case "$arg" in
    --check) CHECK=true ;;
    --fresh-in-place) IN_PLACE=true ;;
    -h|--help)
      cat <<'EOF'
usage: scripts/dogfood.sh [--check]

Runs the Journey-root dogfood gate in an isolated fresh v12 worktree.
`--check` is the CI/release form of the same gate.
EOF
      exit 0
      ;;
    *)
      echo "usage: scripts/dogfood.sh [--check]" >&2
      exit 2
      ;;
  esac
done

if ! $IN_PLACE; then
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/loom-dogfood.XXXXXX")"
  trap 'rm -rf "$WORK"' EXIT
  SNAPSHOTTER="$ROOT/target/debug/loom"
  [ -f "$SNAPSHOTTER" ] && [ ! -L "$SNAPSHOTTER" ] && [ -x "$SNAPSHOTTER" ] || {
    echo "dogfood: trusted snapshot adapter is missing at $SNAPSHOTTER; build it explicitly before starting the gate" >&2
    exit 2
  }
  command -v shasum >/dev/null || {
    echo "dogfood: cannot attest the trusted snapshot adapter without shasum" >&2
    exit 2
  }
  snapshotter_sha256="$(shasum -a 256 "$SNAPSHOTTER" | awk '{print $1}')"
  echo "dogfood: snapshot_adapter_provenance=existing_target_binary snapshot_adapter_sha256=$snapshotter_sha256" >&2
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
  args=(--fresh-in-place)
  $CHECK && args+=(--check)
  "$WORK/scripts/dogfood.sh" "${args[@]}"
  exit $?
fi

cd "$ROOT"
[ ! -e .loom ] || {
  echo "dogfood: refusing existing .loom/; run the public command so it creates an isolated fresh v12 graph" >&2
  exit 1
}

echo "== code gates =="
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --quiet
cargo build --quiet
B="$ROOT/target/debug/loom"

echo "== fresh v12 graph =="
# A v12 export is a reviewed dogfood fixture and may be restored into the
# fresh store.  v1-v11 exports are deliberately ignored rather than imported:
# the normal root export is currently v11 and has no valid Journey-root
# translation.  An absent/old export starts the same honest cold graph.
if [ -f loom.graph.json ]; then
  export_schema="$(python3 - loom.graph.json <<'PY'
import json
import sys
try:
    print(json.load(open(sys.argv[1])).get("schema_version", "missing"))
except (OSError, ValueError) as error:
    print(f"invalid:{error}")
PY
)"
else
  export_schema="missing"
fi

"$B" init . --name loom-dogfood >/dev/null
if [ "$export_schema" = "12" ]; then
  "$B" import loom.graph.json >/dev/null
  echo "dogfood: restored the committed v12 dogfood export into a fresh store"
else
  case "$export_schema" in
    1|2|3|4|5|6|7|8|9|10|11)
      echo "dogfood: not importing loom.graph.json (schema v$export_schema is legacy and remains untouched)" >&2
      ;;
    missing)
      echo "dogfood: no v12 export found; starting a clean Journey-root graph" >&2
      ;;
    *)
      echo "dogfood: loom.graph.json is not a usable v12 export ($export_schema); starting a clean Journey-root graph" >&2
      ;;
  esac
fi

schema="$($B status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["graph"]["schema_version"])')"
[ "$schema" = "12" ] || {
  echo "dogfood: expected a fresh schema-v12 graph, got v$schema" >&2
  exit 1
}

# Cold graphs need real repository code before a human can derive technical
# Intents. Tests already ran in the code gate above; they are proof artifacts,
# not implementation ownership. Record that boundary before discovery so a
# future broad glob cannot silently reintroduce tests as coverage work. An
# exact imported v12 rule is idempotent; a conflicting or duplicated rule
# fails closed instead of rewriting reviewed graph policy.
TEST_IGNORE_REASON="Tests are Validation/proof artifacts, not implementation ownership; literal test paths may be re-registered when an Exercises edge needs source-drift tracking."
test_ignore_state="$(
  "$B" ignore list --json | python3 -c '
import json, sys
matches = [row for row in json.load(sys.stdin) if row.get("glob") == "tests/**"]
if not matches:
    print("missing")
elif len(matches) == 1 and matches[0].get("reason") == sys.argv[1]:
    print("current")
else:
    print("conflict")
' "$TEST_IGNORE_REASON"
)"
case "$test_ignore_state" in
  current) ;;
  missing)
    "$B" ignore add 'tests/**' --reason "$TEST_IGNORE_REASON" >/dev/null
    ;;
  *)
    echo "dogfood: refusing conflicting or duplicate tests/** coverage policy" >&2
    exit 1
    ;;
esac

# This is discovery only; the gate does not infer or approve technical Intents.
for glob in 'src/**/*.rs' 'scripts/*.sh'; do
  "$B" codefile add "$glob" >/dev/null
done
"$B" sync >/dev/null

echo "== authored Journey roots =="
journey_count=0
while IFS= read -r -d '' spec; do
  registration="$("$B" journey add "$spec" --json)"
  journey_id="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["journey"]["name"])' <<<"$registration")"
  # `show` resolves the registered stable id and is the source of truth for
  # its profiles; file names and the old executable-spec convention are never
  # used as run keys.
  profiles="$("$B" journey show "$journey_id" --json | python3 -c '
import json, sys
for profile in sorted(json.load(sys.stdin)["spec"]["profiles"]):
    print(profile)
')"
  [ -n "$profiles" ] || {
    echo "dogfood: Journey '$journey_id' has no authored profiles" >&2
    exit 1
  }
  while IFS= read -r profile; do
    [ -n "$profile" ] || continue
    echo "dogfood: running Journey '$journey_id' profile '$profile'"
    if ! "$B" journey run "$journey_id" --profile "$profile"; then
      cat >&2 <<EOF
dogfood: Journey '$journey_id' profile '$profile' is not runnable.
dogfood: release remains closed until a human reviews and accepts the exact
dogfood: derivation manifest, the derived Intents are implemented/grounded,
dogfood: and the resulting surface manifest is accepted.  Do not fabricate
dogfood: --human-decision or import/migrate the legacy v11 graph.
EOF
      exit 1
    fi
  done <<<"$profiles"
  journey_count=$((journey_count + 1))
done < <(find journeys -type f \( -name '*.yaml' -o -name '*.yml' -o -name '*.json' \) -print0)

[ "$journey_count" -gt 0 ] || {
  echo "dogfood: no authored Journey artifacts found" >&2
  exit 1
}

echo "== integrity =="
"$B" doctor
"$B" coverage
"$B" export
"$B" export --check

if $CHECK; then
  "$B" journey drift --json | python3 -c '
import json, sys
rows = json.load(sys.stdin)
stale = [row for row in rows if not row.get("current", False)]
if stale:
    print("dogfood: Journey projection drift remains:", stale, file=sys.stderr)
    raise SystemExit(1)
'
fi

echo "dogfood: complete — fresh v12 Journey graph is runnable and current"
