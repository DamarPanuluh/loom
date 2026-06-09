#!/usr/bin/env bash
# Full-lifecycle smoke for the loom binary: builds it, then drives one complete
# brownfield session in a throwaway repo — map → ground → sync ripple → quality
# verdict → smells → proof → export/import round trip — asserting graph state
# at each step. Exits non-zero on the first broken link.
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
"$L" init . >/dev/null && "$L" init . >/dev/null   # idempotent
ok "detect + idempotent init"

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

echo "── sync ripple (mtime + imports + locators) ──"
sleep 1; printf '\n# touched\n' >> app.py
R="$("$L" sync --json)"
[ "$(echo "$R" | jget "['files_changed']")" = 1 ] || fail "sync mtime"
[ "$(echo "$R" | jget "['relates_to_edges_flagged']")" = 1 ] || fail "sync ripple"
"$L" smells --limit 50 --json | jget "['total']" >/dev/null
LOOM_AGENT=llm:fixer "$L" edge explore "add item" "reject empty" ground \
  --criterion "the empty-guard runs before save, so both intents live in def add without conflict" >/dev/null
ok "code change → edge stale → re-grounded"

echo "── quality: stick → verdict → green re-earned after change ──"
export LOOM_AGENT=llm:quality
"$L" rule seed iso5055 >/dev/null
"$L" rule apply iso5055-rel-boundary-validation "add item" >/dev/null
"$L" rule verdict iso5055-rel-boundary-validation "add item" --status passing \
  --criterion "blank input raises before any write happens" \
  --evidence "app.py def add: strip-check raises ValueError before save() is reached" >/dev/null
sleep 1; printf '# touched again\n' >> app.py
[ "$("$L" sync --json | jget "['governs_edges_flagged']")" = 1 ] || fail "governs ripple"
ok "passing GOVERNS goes stale when its code changes"

echo "── validator: proof that invokes loom (lock regression) ──"
export LOOM_AGENT=llm:validator
"$L" validation add --name "loom self-read under validate" --type assertion \
  --command "\"$L\" status --json > /dev/null" --intent "add item" >/dev/null
OUT="$("$L" validate "add item")"
echo "$OUT" | grep -q "1/1 passed" || fail "validate (DB lock regression?): $OUT"
ok "validation command may read the graph (session released during exec)"

echo "── export → import round trip ──"
"$L" export --out graph.json >/dev/null
W2="$(mktemp -d)"; cp graph.json "$W2/"; cd "$W2"
"$L" init . >/dev/null
"$L" import graph.json >/dev/null
[ "$("$L" intent list --json | jget ".__len__()")" = 3 ] || fail "import intents"
"$L" doctor >/dev/null || fail "doctor after import"
ok "graph travels: export → fresh init → import → doctor green"
cd /; rm -rf "$W2"

echo
echo "PASS — $PASS checks, full lifecycle verified."
