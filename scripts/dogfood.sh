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

This gate checks cold-import integrity only. It does not climb the maturity
ladder, execute Journey proofs, or persist a v12 graph. A passing run is not
graph-maturity green.
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
expected = json.load(open(sys.argv[1]))
inventory = report["source_inventory"]
assert report["schema"] == expected["schema"]
assert report["status"] == expected["status"]
assert expected["candidate_hash_relation"] == "source_inventory.inventory_hash"
assert report["candidate_hash"] == inventory["inventory_hash"]
for field, value in expected["source_inventory"].items():
    assert inventory[field] == value, (field, inventory[field], value)
' "$ROOT/release/snapshot-expectation.json" <<<"$snapshot"
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
cargo test --all-targets --quiet -- --test-threads=1
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

if $CHECK && [ "$export_schema" != "12" ]; then
  echo "dogfood: --check requires a well-formed committed schema-v12 loom.graph.json (got $export_schema)" >&2
  exit 1
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
profile_count=0
AUTHORITY_ROSTER="$(mktemp "${TMPDIR:-/tmp}/loom-dogfood-authority.XXXXXX")"
DERIVATION_ROSTER="$(mktemp "${TMPDIR:-/tmp}/loom-dogfood-derivations.XXXXXX")"
trap 'rm -f "$AUTHORITY_ROSTER" "$DERIVATION_ROSTER"' EXIT
while IFS= read -r -d '' spec; do
  registration="$("$B" journey add "$spec" --json)"
  journey_id="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["journey"]["name"])' <<<"$registration")"
  cold_show="$("$B" journey show "$journey_id" --json)"
  profiles="$(python3 -c '
import json, sys
for profile in sorted(json.load(sys.stdin)["spec"]["profiles"]):
    print(profile)
' <<<"$cold_show")"
  [ -n "$profiles" ] || {
    echo "dogfood: Journey '$journey_id' has no authored profiles" >&2
    exit 1
  }

  python3 -c '
import json, sys
journey_id = sys.argv[1]
show = json.load(sys.stdin)
ready = show["readiness"]
assert ready["authored"] is True, (journey_id, "authored")
assert ready["derived"] is True, (journey_id, "derived")
assert ready["implemented"] is True, (journey_id, "implemented")
assert ready["derive_gaps"] == [], (journey_id, ready["derive_gaps"])
assert ready["derivations_ratified"] is False, (journey_id, "derivations_ratified")
assert ready["surfaced"] is False, (journey_id, "import unexpectedly surfaced")
surfaces = show["surfaces"]
assert len(surfaces) == 1, (journey_id, "accepted imported surface count", len(surfaces))
assert surfaces[0]["surface"]["status"] == "quarantined", (journey_id, surfaces[0]["surface"])
' "$journey_id" <<<"$cold_show"

  derived_intents="$(python3 -c '
import json, sys
for intent_id in json.load(sys.stdin)["readiness"]["derived_intent_ids"]:
    print(intent_id)
' <<<"$cold_show")"
  [ -n "$derived_intents" ] || {
    echo "dogfood: Journey '$journey_id' has no imported derived Intents" >&2
    exit 1
  }
  while IFS= read -r intent_id; do
    [ -n "$intent_id" ] || continue
    "$B" intent show "$intent_id" --json | python3 -c '
import json, sys
intent = json.load(sys.stdin)
assert intent.get("ratification") == "needs_reconfirmation", (intent.get("id"), intent.get("ratification"))
'
  done <<<"$derived_intents"
  python3 -c '
import json, sys
for derivation in json.load(sys.stdin)["derivations"]:
    print(derivation["edge"]["id"])
' <<<"$cold_show" >>"$DERIVATION_ROSTER"

  manifest="journeys/surfaces/$journey_id.surface.json"
  [ -f "$manifest" ] || {
    echo "dogfood: Journey '$journey_id' is missing canonical surface $manifest" >&2
    exit 1
  }
  "$B" journey surface-accept "$journey_id" --manifest "$manifest" --json >/dev/null
  "$B" journey show "$journey_id" --json | python3 -c '
import json, sys
journey_id = sys.argv[1]
ready = json.load(sys.stdin)["readiness"]
assert ready["surfaced"] is True, (journey_id, "surface was not locally reauthorized")
assert ready["derivations_ratified"] is False, (journey_id, "local surface acceptance fabricated human authority")
' "$journey_id"
  while IFS= read -r profile; do
    [ -n "$profile" ] || continue
    printf '%s\t%s\n' "$journey_id" "$profile" >>"$AUTHORITY_ROSTER"
    profile_count=$((profile_count + 1))
    echo "dogfood: Journey '$journey_id' profile '$profile': not executed — authority_voided_by_import"
  done <<<"$profiles"
  journey_count=$((journey_count + 1))
done < <(find journeys -type f \( -name '*.yaml' -o -name '*.yml' -o -name '*.json' \) ! -name '*.surface.json' -print0)

[ "$journey_count" -gt 0 ] || {
  echo "dogfood: no authored Journey artifacts found" >&2
  exit 1
}

echo "== cold-authority integrity =="
doctor_status=0
doctor_report="$("$B" doctor --json 2>/dev/null)" || doctor_status=$?
python3 -c '
import json, re, sys
roster_path, status = sys.argv[1:]
assert int(status) != 0, "imported authority unexpectedly passed doctor"
issues = json.load(sys.stdin)
assert issues, "doctor failed without structured issues"
assert all(issue.get("kind") == "unratified_journey_derivation" for issue in issues), issues
pattern = re.compile(r"^Derives edge '\''([0-9a-f]{32})'\'' targets an Intent that is not ratified$")
observed = []
for issue in issues:
    match = pattern.fullmatch(issue.get("message", ""))
    assert match, issue
    observed.append(match.group(1))
with open(roster_path) as roster_file:
    expected = [line.strip() for line in roster_file if line.strip()]
assert len(expected) == len(set(expected)), "derivation roster contains duplicates"
assert len(observed) == len(set(observed)), "doctor reported duplicate derivation issues"
assert set(observed) == set(expected), (
    "cold-authority doctor mismatch",
    sorted(set(expected) - set(observed)),
    sorted(set(observed) - set(expected)),
)
' "$DERIVATION_ROSTER" "$doctor_status" <<<"$doctor_report"
"$B" coverage
"$B" export
"$B" export --check

if $CHECK; then
  "$B" journey drift --json | python3 -c '
import json, sys
roster_path = sys.argv[1]
report = json.load(sys.stdin)
with open(roster_path) as roster_file:
    roster = [tuple(line.rstrip("\n").split("\t")) for line in roster_file if line.strip()]
assert len(roster) == len(set(roster)), "authority-pause roster contains duplicates"
rows = []
for index, row in enumerate(report["journeys"]):
    assert isinstance(row.get("current"), bool), f"drift row {index} lacks boolean current"
    pair = (row.get("journey_id"), row.get("profile"))
    assert row["current"] is False, ("authority-pause row unexpectedly current", pair)
    rows.append(pair)
assert len(rows) == len(set(rows)), "drift contains duplicate Journey/profile rows"
assert set(rows) == set(roster), ("authority-pause drift mismatch", sorted(set(roster) - set(rows)), sorted(set(rows) - set(roster)))
assert report["stale"] == len(roster) == len(rows), (report["stale"], len(roster), len(rows))
' "$AUTHORITY_ROSTER"
fi

echo "dogfood: OK — cold-import integrity only"
echo "dogfood: $journey_count Journey(s) re-registered; $profile_count profile(s) not executed — authority_voided_by_import"
echo "dogfood: this is not graph-maturity green — doctor remains fail-closed on unratified imported derivations; proofs were not compiled or run; the temporary graph and export were discarded"
