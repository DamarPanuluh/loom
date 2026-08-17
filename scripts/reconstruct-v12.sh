#!/usr/bin/env bash
# Reconstruct a fresh schema-v12 graph in an isolated destination.
#
# This is mechanical reconstruction only. It never touches a pre-existing
# .loom/ directory, never ratifies, never derive-accepts, never answers
# questions, and never runs Journey proofs. Compile is blocked until a human
# re-establishes local authority.
#
# Usage:
#   scripts/reconstruct-v12.sh --destination DIR
#
# DIR must be empty (filled from a git archive of HEAD) or an existing source
# checkout that does not contain .loom/. The live repository is refused.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST=""

usage() {
  cat <<'EOF'
usage: scripts/reconstruct-v12.sh --destination DIR

Reconstruct a fresh v12 graph in DIR without touching the caller's live .loom/.

Mechanical steps: init, import loom.graph.json, register code, sync, locally
reauthorize CLI surfaces, emit hash-bound review manifests, write an honest
ladder/report.

Does not: ratify, derive-accept, reject, answer questions, compile, or run
proofs. A completed reconstruction is not graph-maturity green.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --destination)
      [ $# -ge 2 ] || { echo "reconstruct-v12: --destination requires a path" >&2; exit 2; }
      DEST="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
done

[ -n "$DEST" ] || { usage >&2; exit 2; }

mkdir -p "$DEST"
DEST="$(cd "$DEST" && pwd)"
ROOT="$(cd "$ROOT" && pwd)"

if [ "$DEST" = "$ROOT" ]; then
  echo "reconstruct-v12: refusing the live repository; pass an isolated --destination so the existing .loom/ stays untouched" >&2
  exit 2
fi

if [ -e "$DEST/.loom" ]; then
  echo "reconstruct-v12: refusing existing .loom/ at $DEST/.loom" >&2
  exit 1
fi

if [ -e "$ROOT/.loom" ] && [ "$(cd "$ROOT/.loom" && pwd)" = "$DEST" ]; then
  echo "reconstruct-v12: refusing the live .loom/ directory as a destination" >&2
  exit 1
fi

B="${LOOM_BIN:-}"
if [ -z "$B" ]; then
  if [ -x "$ROOT/target/debug/loom" ] && [ ! -L "$ROOT/target/debug/loom" ]; then
    B="$ROOT/target/debug/loom"
  elif command -v loom >/dev/null; then
    B="$(command -v loom)"
  else
    echo "reconstruct-v12: no loom binary; build target/debug/loom or set LOOM_BIN" >&2
    exit 2
  fi
fi

populated=false
if [ -n "$(find "$DEST" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
  populated=true
fi

if $populated; then
  [ -f "$DEST/loom.graph.json" ] && [ -d "$DEST/journeys" ] || {
    echo "reconstruct-v12: destination already has files but is not a Loom source checkout (need loom.graph.json and journeys/)" >&2
    exit 2
  }
else
  command -v git >/dev/null || {
    echo "reconstruct-v12: git is required to fill an empty destination from HEAD" >&2
    exit 2
  }
  git -C "$ROOT" archive HEAD | tar -x -C "$DEST"
fi

cd "$DEST"
[ ! -e .loom ] || {
  echo "reconstruct-v12: destination unexpectedly contains .loom/ after copy" >&2
  exit 1
}

echo "== reconstruct-v12: fresh store =="
"$B" --graph "$DEST" init . --name loom-v12-reconstruct >/dev/null
"$B" --graph "$DEST" import loom.graph.json --json

TEST_IGNORE_REASON="Tests are Validation/proof artifacts, not implementation ownership; literal test paths may be re-registered when an Exercises edge needs source-drift tracking."
test_ignore_state="$(
  "$B" --graph "$DEST" ignore list --json | python3 -c '
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
    "$B" --graph "$DEST" ignore add 'tests/**' --reason "$TEST_IGNORE_REASON" >/dev/null
    ;;
  *)
    echo "reconstruct-v12: refusing conflicting or duplicate tests/** coverage policy" >&2
    exit 1
    ;;
esac

for glob in 'src/**/*.rs' 'scripts/*.sh'; do
  "$B" --graph "$DEST" codefile add "$glob" >/dev/null
done
"$B" --graph "$DEST" sync --json

while IFS= read -r -d '' spec; do
  "$B" --graph "$DEST" journey add "$spec" --json >/dev/null
done < <(find journeys -type f \( -name '*.yaml' -o -name '*.yml' -o -name '*.json' \) ! -name '*.surface.json' -print0)

while IFS= read -r -d '' manifest; do
  journey_id="$(basename "$manifest" .surface.json)"
  "$B" --graph "$DEST" journey surface-accept "$journey_id" --manifest "$manifest" --json >/dev/null
done < <(find journeys/surfaces -type f -name '*.surface.json' -print0)

echo "== reconstruct-v12: review manifests =="
python3 - "$B" "$DEST" <<'PY'
import json, subprocess, sys, collections
from pathlib import Path

binary, dest = sys.argv[1], Path(sys.argv[2])

def loom(*args):
    proc = subprocess.run([binary, "--graph", str(dest), *args], capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"reconstruct-v12: command failed: {' '.join(args)}\n{proc.stderr or proc.stdout}")
    return proc.stdout

def loom_allow_fail(*args):
    proc = subprocess.run([binary, "--graph", str(dest), *args], capture_output=True, text=True)
    return proc.returncode, proc.stdout, proc.stderr

doctor_code, doctor_out, _ = loom_allow_fail("doctor", "--json")
doctor = json.loads(doctor_out) if doctor_out.strip() else []
status = json.loads(loom("status", "--json"))
proposals = json.loads(loom("proposal", "list", "--limit", "200", "--json"))["items"]
journeys = json.loads(loom("journey", "list", "--json"))["items"]
ratify = json.loads(loom("next", "--mode", "ratify", "--all", "--json"))
rectify = json.loads(loom("next", "--mode", "rectify", "--all", "--json"))
coverage = json.loads(loom("next", "--mode", "coverage", "--all", "--json"))
elaborate = json.loads(loom("next", "--mode", "elaborate", "--all", "--json"))

derived = []
readiness_rows = []
for journey in journeys:
    show = json.loads(loom("journey", "show", journey["name"], "--json"))
    ready = show["readiness"]
    readiness_rows.append({
        "journey": journey["name"],
        "derived": ready.get("derived"),
        "derivations_ratified": ready.get("derivations_ratified"),
        "implemented": ready.get("implemented"),
        "surfaced": ready.get("surfaced"),
        "compiled": ready.get("compiled"),
        "proven": ready.get("proven"),
        "derived_intent_ids": ready.get("derived_intent_ids") or [],
    })
    for intent_id in ready.get("derived_intent_ids") or []:
        derived.append((journey["name"], intent_id))

manifest_dir = dest / "review-manifests"
manifest_dir.mkdir(exist_ok=True)
emitted = []
for proposal in proposals:
    body = proposal.get("body") or {}
    if body.get("source") != "journey_derivation" or not body.get("raw"):
        continue
    manifest = json.loads(body["raw"]) if isinstance(body["raw"], str) else body["raw"]
    journey_id = manifest["journey_id"]
    path = manifest_dir / f"{journey_id}.json"
    path.write_text(json.dumps(manifest, indent=2) + "\n")
    code, out, err = loom_allow_fail(
        "journey", "derive", journey_id, "--candidate-json", str(path), "--json"
    )
    candidate = {}
    if code == 0 and out.strip():
        packet = json.loads(out)
        candidate = packet.get("candidate_state") or {}
    emitted.append({
        "journey_id": journey_id,
        "proposal_id": manifest.get("proposal_id"),
        "journey_hash": manifest.get("journey_hash"),
        "manifest_hash": body.get("manifest_hash"),
        "path": str(path.relative_to(dest)),
        "intent_count": len(manifest.get("intents") or []),
        "relationship_count": len(manifest.get("relationships") or []),
        "unresolved_question": manifest.get("unresolved_question"),
        "candidate_matches": len(candidate.get("candidate_intent_matches") or []),
        "matching_adopted_proposals": len(candidate.get("matching_adopted_proposals") or []),
        "derive_inspect_exit": code,
        "derive_inspect_error": err.strip() if code != 0 else None,
    })

derived_ids = {intent_id for _, intent_id in derived}
ratify_items = ratify.get("items") or []
domain_intents = [
    item["target"]["name"]
    for item in ratify_items
    if item.get("target", {}).get("id") not in derived_ids
]
kinds = collections.Counter(issue.get("kind") for issue in doctor)
rungs = [
    {
        "name": row["name"],
        "state": row["state"],
        "depth": row["depth"],
        "blocked": row.get("blocked"),
        "blocked_by": row.get("blocked_by"),
        "detail": row["detail"],
    }
    for row in status["maturity"]["rungs"]
]
ownership = status.get("code_ownership") or {}
index = {
    "schema": "loom.v12-review-index/v1",
    "kind": "import_authority_reconstruction",
    "destination": str(dest),
    "persistent_v12_graph": True,
    "ladder_climbed": False,
    "code_ci_vs_graph_maturity": {
        "code_ci": "not assessed by this script",
        "graph_maturity": "not green — human authority still required",
    },
    "doctor": {
        "exit_code": doctor_code,
        "count": len(doctor),
        "kinds": dict(kinds),
    },
    "queues": status.get("queues"),
    "validation_summary": status.get("validation_summary"),
    "code_ownership": {
        "registered": ownership.get("registered"),
        "owned": ownership.get("owned"),
        "unowned": ownership.get("unowned"),
        "unowned_files": ownership.get("unowned_files"),
    },
    "maturity": {
        "rung": status["maturity"]["rung"],
        "phase": status["maturity"]["phase"],
        "next_command": status["maturity"]["next_command"],
        "rungs": rungs,
    },
    "journey_readiness": readiness_rows,
    "batches": [
        {
            "id": 1,
            "title": "Re-establish local authority voided by import",
            "kind": "human_authority",
            "distinct_decisions": 1,
            "not": "75 independent product-design questions",
            "journeys": sorted({row["journey_id"] for row in emitted}),
            "derived_intent_edges": len(derived),
            "unique_derived_intents": len(derived_ids),
            "domain_intents_also_voided": domain_intents,
            "ratify_queue": len(ratify_items),
            "decision_requested": "Re-establish local standing for the imported wantedness. Meaning did not change. This is not semantic approval of new behavior.",
            "recommended_answer": "Re-establish local authority for the imported v12 graph; meaning is unchanged.",
            "write_back": "loom intent ratify --all --evidence 're-establish local standing for imported wantedness; meaning is unchanged' --human-decision '<exact human answer>' --json",
            "after_write": "Then compile and run Journey proof profiles. Compile/run are not product decisions.",
            "supporting_files": [row["path"] for row in emitted],
            "manifests": emitted,
        },
        {
            "id": 2,
            "title": "Own newly unowned files",
            "kind": "builder_coverage",
            "distinct_decisions": len(coverage.get("items") or []),
            "items": coverage.get("items") or [],
            "decision_requested": "Ground each unowned file to the intent that realizes its behavior, or unregister it. This is implementation ownership, not wantedness.",
        },
        {
            "id": 3,
            "title": "Clear false-duplicate rectify friction",
            "kind": "rectify_judgment",
            "distinct_decisions": len(rectify.get("items") or []),
            "items": rectify.get("items") or [],
            "decision_requested": "These are structural duplicate heuristics, not wantedness. Clear, relate, or escalate. Do not ratify from the rectify lane.",
        },
    ],
    "deferred": {
        "elaborate": {
            "count": len(elaborate.get("items") or []),
            "note": "All current elaborate items open scenarios/proof/journey axes. Journey ancestry and proof are blocked on batch 1 plus local proof runs; do not treat them as 23 independent product questions yet.",
            "items": elaborate.get("items") or [],
        },
        "analyze": status.get("queues", {}).get("analyze"),
        "triage": status.get("queues", {}).get("triage"),
        "note": "Do not grind uninspected relationships or structural findings before local authority and proof runs exist.",
    },
    "forbidden_next": [
        "loom journey derive-accept without the human's exact answer",
        "loom intent ratify without the human's exact answer",
        "loom intent reject",
        "loom question answer",
        "bulk finding verdicts",
        "bulk relationship inspect",
    ],
}
(manifest_dir / "INDEX.json").write_text(json.dumps(index, indent=2) + "\n")
print(json.dumps({
    "review_manifests": str(manifest_dir),
    "index": str(manifest_dir / "INDEX.json"),
    "doctor_kinds": dict(kinds),
    "doctor_count": len(doctor),
    "ratify_queue": len(ratify_items),
    "manifests_emitted": len(emitted),
    "unowned_files": ownership.get("unowned_files"),
    "maturity_rung": status["maturity"]["rung"],
    "maturity_phase": status["maturity"]["phase"],
}, indent=2))
PY

echo "reconstruct-v12: OK — persistent v12 graph reconstructed at $DEST"
echo "reconstruct-v12: human authority is still required; this is not graph-maturity green"
echo "reconstruct-v12: review index at $DEST/review-manifests/INDEX.json"
echo "reconstruct-v12: live repository .loom/ was not modified"
