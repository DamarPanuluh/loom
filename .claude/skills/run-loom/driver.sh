#!/usr/bin/env bash
# Full-lifecycle smoke for the loom binary: builds it, then drives one complete
# brownfield session in a throwaway repo — map → ground → content-hash sync
# ripple (touch-only flags nothing; real change flags + notes the cause) →
# quality verdict (incl. 360° unmeasured queue + one-command verdict) → smells
# → proofs (incl. blocked) → closeout → export/import round trip + commit
# guard — asserting graph state at each step. Exits non-zero on the first
# broken link.
#
# Usage:  .claude/skills/run-loom/driver.sh          (from the repo root)
#         LOOM_BIN=/path/to/loom driver.sh           (skip the build)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
if [ -z "${LOOM_BIN:-}" ]; then
  echo "── build ──"
  (cd "$ROOT" && cargo build 2>&1 | tail -1)
  LOOM_BIN="$ROOT/target/debug/loom"
fi
L="$LOOM_BIN"
PASS=0
ok()   { PASS=$((PASS+1)); echo "  ok  $1"; }
fail() { echo "  FAIL $1" >&2; exit 1; }
jget() { python3 -c "import sys,json;d=json.load(sys.stdin);print(eval(\"d$1\"))"; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"
cat > app.py <<'EOF'
from .store import save
def add(text):
    if not text.strip():
        raise ValueError("empty")
    save(text)
EOF
cat > store.py <<'EOF'
def save(item):
    open("db.txt", "a").write(item + "\n")
EOF

echo "── detect / init ──"
[ "$("$L" detect --json | jget "['suggested_mode']")" = brownfield ] || fail detect
[ "$("$L" detect --json | jget "['recommended_packs'][0]['pack']")" = iso5055 ] \
  || fail "detect should recommend the iso5055 baseline pack"
"$L" init . >/dev/null && "$L" init . >/dev/null   # idempotent
ok "detect (incl. pack recommendation) + idempotent init"

echo "── builder maps (lanes enforced, names resolve) ──"
export LOOM_AGENT=llm:builder
"$L" intent add --name "todo app" --description "tiny todo list exercise app" --level system >/dev/null
"$L" intent add --name "add item" --description "add appends a non-empty item and persists it" --level feature --aspect happy >/dev/null
"$L" intent add --name "reject empty" --description "add with blank text raises without writing" --level feature --aspect sad >/dev/null
"$L" edge hierarchy "todo app" "add item" >/dev/null
"$L" edge hierarchy "todo app" "reject empty" >/dev/null
"$L" codefile add '*.py' >/dev/null
"$L" edge implement "add item" app.py --locator "def add" >/dev/null
"$L" edge implement "reject empty" app.py >/dev/null
"$L" edge implement "add item" store.py >/dev/null
[ "$("$L" status --json | jget "['graph_state']['vertically_complete']")" = True ] || fail vertical
ok "map by name → vertically complete"

echo "── lane + evidence gates reject bad input ──"
"$L" edge explore "add item" "reject empty" ground --criterion "they share the validation boundary in def add" 2>/dev/null \
  && fail "builder grounded an edge (lane gate missed)" || ok "lane violation blocked (builder cannot ground)"
LOOM_AGENT=llm:analyzer "$L" edge explore "add item" "reject empty" ground --criterion "todo" 2>/dev/null \
  && fail "vacuous criterion accepted" || ok "vacuous criterion rejected"
LOOM_AGENT=llm:analyzer "$L" edge explore "add item" "reject empty" ground \
  --criterion "the empty-guard runs before save, so both intents live in def add without conflict" >/dev/null
ok "analyzer grounds with substantive criterion"

echo "── sync ripple (content-hash + imports + locators) ──"
touch app.py store.py
[ "$("$L" sync --json | jget "['files_changed']")" = 0 ] \
  || fail "touch-only mtime churn flagged a change (content-hash regression)"
ok "touch-only churn flags nothing (detection is content-based)"
printf '\n# touched\n' >> app.py
R="$("$L" sync --json)"
[ "$(echo "$R" | jget "['files_changed']")" = 1 ] || fail "sync content change"
[ "$(echo "$R" | jget "['relates_to_edges_flagged']")" = 1 ] || fail "sync ripple"
"$L" note list --kind transition | grep -q "(sync: app.py changed)" \
  || fail "stale-cause transition note missing"
"$L" smells --limit 50 --json | jget "['total']" >/dev/null
LOOM_AGENT=llm:fixer "$L" edge explore "add item" "reject empty" ground \
  --criterion "the empty-guard runs before save, so both intents live in def add without conflict" >/dev/null
ok "code change → edge stale (cause noted on the edge) → re-grounded"

echo "── quality: stick → verdict → green re-earned after change ──"
export LOOM_AGENT=llm:quality
"$L" rule seed iso5055 >/dev/null
"$L" rule seed bogus 2>/dev/null && fail "unknown pack accepted" || true
Q="$("$L" next --mode quality --json)"
[ "$(echo "$Q" | jget "['governs']['inspection_status']")" = unmeasured ] \
  || fail "never-measured rule×intent pair should top the quality queue"
echo "$Q" | jget "['governs']['notes']" | grep -q "detection:" \
  || fail "detection logic should travel with the unmeasured item"
ok "360°: quality queue serves never-measured pairs (detection logic attached)"
"$L" rule verdict iso5055-perf-bounded-work "reject empty" --status independent \
  --criterion "no iteration over external-sized data exists anywhere in this path" \
  --evidence "def add validates a single string and persists once; there is no loop or recursion" >/dev/null
[ "$("$L" status --json | jget "['graph_state']['coverage']['measured_pairs']['covered']")" -ge 1 ] \
  || fail "one-command verdict did not count as measured"
"$L" status | grep -q "360°:" || fail "360° coverage line missing from the pulse"
ok "360°: one-command verdict (no apply) creates the edge; coverage counts it"
"$L" rule apply iso5055-rel-boundary-validation "add item" >/dev/null
"$L" rule verdict iso5055-rel-boundary-validation "add item" --status passing \
  --criterion "blank input raises before any write happens" \
  --evidence "app.py def add: strip-check raises ValueError before save() is reached" >/dev/null
printf '# touched again\n' >> app.py
[ "$("$L" sync --json | jget "['governs_edges_flagged']")" = 1 ] || fail "governs ripple"
ok "passing GOVERNS goes stale when its code changes"

echo "── batch: bulk verdicts, per-line gates, partial failure → non-zero ──"
BOUT="$(printf '%s\n%s\n' \
  '{"op":"rule_verdict","rule":"iso5055-sec-no-hardcoded-secrets","intent":"reject empty","status":"independent","criterion":"no secret material can exist on this validation-only code path","evidence":"def add handles a text string and a file append; no credentials, tokens, or keys anywhere","confidence":0.9}' \
  '{"op":"ground","a":"add item","b":"reject empty","criterion":"todo","confidence":0.9}' \
  | "$L" batch - 2>&1)" && fail "batch with a bad line exited 0" || true
echo "$BOUT" | grep -q "1 applied, 1 failed" || fail "batch per-line accounting wrong: $BOUT"
ok "batch: bulk verdicts, gates per line (bad line fails, good line lands), non-zero exit"

echo "── validator: proof that invokes loom (lock regression) ──"
export LOOM_AGENT=llm:validator
"$L" validation add --name "loom self-read under validate" --type assertion \
  --command "\"$L\" status --json > /dev/null" --intent "add item" >/dev/null
OUT="$("$L" validate "add item")"
echo "$OUT" | grep -q "1/1 passed" || fail "validate (DB lock regression?): $OUT"
ok "validation command may read the graph (session released during exec)"
# One validation can prove SEVERAL intents — a single run's result must mirror
# to ALL its VALIDATES edges, or the compass (edge states) disagrees with the
# validator queue (last_result) forever: phase=validate with an empty queue.
"$L" edge validates "loom self-read under validate" "reject empty" >/dev/null
"$L" validate "add item" >/dev/null
[ "$("$L" status --json | jget "['graph_state']['phase']")" != validate ] \
  || fail "compass stuck on validate: passed run left a sibling VALIDATES edge uninspected"
ok "a validation run proves every intent it validates (compass agrees with queue)"

echo "── blocked proof: reason required, out of the queue, visible ──"
"$L" validation add --name "external smoke" --type manual_check --intent "reject empty" >/dev/null
"$L" validation mark "external smoke" --result blocked 2>/dev/null \
  && fail "blocked accepted without --reason" || true
"$L" validation mark "external smoke" --result blocked \
  --reason "needs a live staging URL which does not exist in this sandbox" >/dev/null
[ "$("$L" next --mode validate --json | jget "['status']")" = empty ] \
  || fail "blocked proof still nags the validator queue"
"$L" validation list --json | grep -q '"blocked"' || fail "blocked not visible in list"
ok "blocked: reason gated, queue silent, state visible"

echo "── graph pin: LOOM_GRAPH beats cwd (the cd-fallback incident class) ──"
PINHOME="$PWD"
FOREIGN="$(mktemp -d)"
cd "$FOREIGN"   # a foreign cwd with no .loom
LOOM_GRAPH="$PINHOME" "$L" note add --text "pinned write landed in the pinned graph despite a foreign cwd" --kind commentary >/dev/null \
  || fail "pinned command failed from foreign cwd"
"$L" status >/dev/null 2>&1 && fail "unpinned command in a bare dir found a graph" || true
cd "$PINHOME"; rm -rf "$FOREIGN"
"$L" note list --kind commentary | grep -q "pinned write landed" || fail "pin write missing from pinned graph"
ok "graph pin: foreign-cwd mutation hit the pinned graph; unpinned stays strict"

echo "── closeout ──"
[ "$("$L" next --all --json | jget "['mode']")" = all ] || fail "next --all"
ok "closeout view answers across every lane"

echo "── export (positional) → commit guard → import round trip ──"
"$L" export graph.json >/dev/null
"$L" export graph.json --check >/dev/null || fail "fresh export reads stale"
LOOM_AGENT=llm:builder "$L" intent add --name "drift" \
  --description "deliberate graph drift to trip the commit guard" --level feature >/dev/null
"$L" export graph.json --check >/dev/null 2>&1 \
  && fail "stale export passed --check" || ok "commit guard: fresh passes, drift fails non-zero"
W2="$(mktemp -d)"; cp graph.json "$W2/"; cd "$W2"
"$L" init . >/dev/null
"$L" import graph.json >/dev/null
[ "$("$L" intent list --json | jget ".__len__()")" = 3 ] || fail "import intents"
"$L" doctor >/dev/null || fail "doctor after import"
ok "graph travels: export → fresh init → import → doctor green"
cd /; rm -rf "$W2"

echo
echo "PASS — $PASS checks, full lifecycle verified."
