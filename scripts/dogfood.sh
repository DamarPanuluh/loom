#!/usr/bin/env bash
# Dogfood: drive loom over its own codebase to produce a committed
# loom.graph.json. The point is NOT a green scoreboard — it is a graph whose
# intents are falsifiable BEHAVIORAL claims, each grounded to a specific
# symbol+line and INSPECTED (a recorded verdict with criterion + evidence), so
# `loom find` answers "where does this behavior live, and is it confirmed?"
# better than grep. Tags name each intent's distinct concern; cohesive intents
# that share a file carry a relates edge. Run from the repo root.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
B="$ROOT/target/debug/loom"

cargo build -q

# fresh graph
rm -rf .loom loom.graph.json
"$B" init . --name loom >/dev/null

# register + extract the source plane
"$B" codefile add 'src/**/*.rs' >/dev/null
"$B" sync >/dev/null

# vocabulary: one term per DISTINCT concern (not a coarse plane label), so
# duplicate-detection is armed without falsely flagging unrelated behaviors.
for t in evidence-gate import-order determinism hash-ripple atom-rule \
         debt-projection residue-router leaf-grounding find-surface; do
  "$B" vocab add "$t" --why "concern: $t" >/dev/null
done

# capture the 8-char edge id the create commands print as "... [abcd1234]".
# resolve_edge accepts the prefix, so verdicts can target what the CLI shows.
# The bracketed id is NOT the last character of the output (a `next: …` pulse
# line follows), so extract the last [hex] token instead of suffix-stripping.
last_eid() {
  local eid
  eid="$(printf '%s' "$1" | grep -oE '\[[0-9a-f]{8}\]' | tail -1 | tr -d '[]')"
  [ -n "$eid" ] || { echo "dogfood: no edge id in output: $1" >&2; exit 1; }
  printf '%s' "$eid"
}

# triage each programmatic oversized_file flag with a SPECIFIC, recorded reason
# — the loop's point is judgment, not a rubber stamp. Each known file is a large
# but cohesive surface; the verdict is durable and survives every sync. A NEW
# oversized file we have not judged FAILS the dogfood on purpose: an unjudged
# flag must reach a human, never be auto-laundered "cohesive".
while read -r fid fpath; do
  case "$fpath" in
    src/signal.rs)             why="the computed-signal plane: smells, debt, doctor and finding views are one read-only projection surface" ;;
    src/sync.rs)               why="the structural-plane recompute: hashing, ripple, derived findings and journey resets need a single staleness writer" ;;
    src/packs.rs)              why="pre-authored rule-pack data: declarative rule bodies with guidance fields, not control flow" ;;
    src/journey.rs)            why="the journey spec model + executor: parse, template, execute and judge one flow format in one place" ;;
    src/scan.rs)               why="the diagnostic-adapter plane: config, parse maps and finding convergence are one adapter lifecycle" ;;
    src/cli/subcommands.rs)    why="the clap declaration table: one enum per command family; declarative surface, no logic" ;;
    src/commands/misc_cmd.rs)  why="the orientation surface: door/session/guide/find/detect handlers share the keyword scorer and menu builders" ;;
    src/commands/proof_cmd.rs) why="the proof command family: validation and rule handlers share the verdict/evidence plumbing" ;;
    src/commands/intent.rs)    why="the intent command family: lifecycle/update/retire handlers share resolution, ripple and guard helpers" ;;
    src/workitem/queues.rs)    why="the 'loom next' queue builders: one work-item builder per mode, cohesive by the queue partition" ;;
    *)
      echo "dogfood: untriaged oversized finding '$fpath' has no recorded judgment — triage it (justified|needed|blocked) before dogfood can complete" >&2
      exit 1
      ;;
  esac
  "$B" finding verdict "$fid" justified --reason "$fpath is $why; length is intentional, not tangled" >/dev/null
done < <("$B" finding list --kind oversized_file --state untriaged | awk '$1 ~ /^\[untriaged/ { print $2, $3 }')

# system root — the product purpose (a non-leaf parent, realized via children).
"$B" intent add --name "loom maintains a falsifiable graph for LLM-driven codebase work" \
  --description "durable, falsifiable memory an LLM drives: what the code should do, where it lives, how it is proven" \
  --level system --lifecycle implemented --visibility user_visible >/dev/null
SYS="loom maintains a falsifiable graph"

# behavioral leaf: create it, tag it with its concern, hang it under SYS, and
# verdict the hierarchy edge as holding (the child is independently grounded).
leaf() { # name | desc | tag
  "$B" intent add --name "$1" --description "$2" --level feature \
    --lifecycle implemented --visibility internal >/dev/null
  "$B" intent tag add "$1" "$3" >/dev/null
  local out eid
  out=$("$B" edge relate hierarchy "$SYS" "$1")
  eid=$(last_eid "$out")
  "$B" edge verdict "$eid" ground \
    --criterion "the system intent decomposes into this behavior" \
    --evidence "child '$1' is independently grounded to a symbol and inspected" \
    --confidence 0.9 >/dev/null
}

# ground an intent in a file at a precise locator, then inspect (verdict) it.
gi() { # intent | file | locator | criterion | evidence
  local out eid
  out=$("$B" edge implement "$1" "$2" --locator "$3")
  eid=$(last_eid "$out")
  "$B" edge verdict "$eid" ground --criterion "$4" --evidence "$5" --confidence 0.95 >/dev/null
}

# relate two cohesive intents (kills overlapping_ownership when they share a
# file) and record why the relationship holds.
rel() { # a | b | why
  local out eid
  out=$("$B" edge relate relates "$1" "$2")
  eid=$(last_eid "$out")
  "$B" edge verdict "$eid" ground \
    --criterion "these behaviors are cohesive" --evidence "$3" --confidence 0.9 >/dev/null
}

# ---- the behavioral spine: falsifiable claims, grounded to real symbols ------

leaf "the verdict write boundary enforces the evidence, truth-class and lane gates" \
  "INV-4/5/6/7: record_verdict is the sole writer of asserted edge status; the verdict/show commands resolve the displayed 8-char id first, then it demands criterion+evidence for passing/failing, evidence for independent, refuses derived edges, and checks the agent's lane" evidence-gate
gi "the verdict write boundary enforces the evidence, truth-class and lane gates" \
  src/store/edges.rs "fn record_verdict (evidence/lane/derived gate) + is_placeholder gate; schema CHECK in store/mod.rs" \
  "a passing/failing verdict with empty OR placeholder criterion/evidence is rejected; independent without evidence is rejected; a derived edge is rejected; a wrong-lane agent is rejected; and the column CHECK rejects any truth_class outside {derived,asserted}" \
  "two enforcement layers — store/mod.rs schema CHECK(truth_class) rejects bad values at write (defense in depth), and store/edges.rs record_verdict gates evidence/lane/derived + is_placeholder; covered by inv4/inv5/inv6/inv7 and record_verdict_rejects_placeholder_text_without_partial_write"

leaf "import loads every node before any edge" \
  "two-phase restore: parse-and-validate the whole export, then in one transaction insert all nodes before any edge, so an edge endpoint always resolves" import-order
gi "import loads every node before any edge" \
  src/store/facets.rs "fn restore (validate fully; nodes before edges in one txn)" \
  "importing a graph never inserts an edge before its endpoint nodes exist" \
  "store/facets.rs restore validates fully then inserts nodes before edges in one txn; travel.rs from_json parses before any write"

rel "the verdict write boundary enforces the evidence, truth-class and lane gates" \
    "import loads every node before any edge" \
    "both are store-module persistence-correctness guarantees over the same edge table"

leaf "the derived plane rebuilds byte-identically" \
  "INV-2: export orders nodes/edges/facets canonically and derived rows are content-addressed with a sentinel timestamp, so wipe+sync+export reproduces identical bytes" determinism
gi "the derived plane rebuilds byte-identically" \
  src/travel.rs "export_to_file:75 / Export::from_snapshot" \
  "exporting, wiping derived data, re-syncing and re-exporting yields identical bytes" \
  "travel.rs canonicalizes ordering; store/mod.rs DERIVED_TS empty + FNV-1a derived_id; covered by inv2_derived_plane_rebuildable"

leaf "changing a file re-opens the asserted edges grounded in it" \
  "content-hash ripple: sync stales the asserted edges depending on a changed codefile; an unchanged hash (or a first-ever observation) triggers no ripple, so there is no false churn" hash-ripple
gi "changing a file re-opens the asserted edges grounded in it" \
  src/sync.rs "run:65 (compare 85; ripple only when prior.is_some 90-98)" \
  "editing a grounded file's content moves its settled verdicts to needs_reverification; an unchanged file does not, and a never-seen file does not" \
  "sync.rs recomputes content_hash, skips on equal (86), ripples via stale_edge only when a prior hash existed and differs (90)"

leaf "a symbol-pattern name is refused as an intent" \
  "INV-ATOM: functions/symbols are locators on implements edges, not intents; intent add rejects a symbol-shaped name unless --allow-symbol-name is given with a behavioral description" atom-rule
gi "a symbol-pattern name is refused as an intent" \
  src/commands/intent.rs "intent_add gate:136 -> looks_like_symbol:654" \
  "loom intent add --name capture_payment (no --allow-symbol-name) is rejected" \
  "commands/intent.rs guards the add path on looks_like_symbol; covered by inv_atom_rejects_symbol_named_intent"

leaf "debt clusters are computed on read and never stored" \
  "INV-3: statistical signal (debt) is a projection derived from a snapshot; it is never written back as a node or edge" debt-projection
gi "debt clusters are computed on read and never stored" \
  src/signal.rs "debt:235" \
  "loom debt emits clusters but writes no nodes or edges to the graph" \
  "signal.rs debt builds DebtCluster from a read-only snapshot; no INSERT; the statistical truth-class is computed-only"

leaf "the next router serves the highest-priority asserted residue with a prompt contract" \
  "loom next returns failing, then stale, then uninspected asserted edges as a work item carrying allowed/forbidden actions and the evidence it expects" residue-router
gi "the next router serves the highest-priority asserted residue with a prompt contract" \
  src/workitem/mod.rs "next:171 (priority: fix > validate > build > coverage > quality > analyze > triage > review > prove > elaborate)" \
  "with a failing and an uninspected edge present, next returns the failing one first, with its contract" \
  "workitem/mod.rs next walks the fixed queue priority so failing claims (fix_item) are served before anything else; each item carries a PromptContract"
gi "the next router serves the highest-priority asserted residue with a prompt contract" \
  src/workitem/queues.rs "analyze_item:119 (stale before uninspected; failing routes to fix_item:97)" \
  "an analyze packet serves a stale settled claim before a merely uninspected one" \
  "queues.rs analyze_item takes needs_reverification non-governs/non-validates edges before uninspected ones; failing edges belong to fix_item, so no edge is served by two queues"

leaf "maturity counts only leaf intents as needing grounding" \
  "an intent with hierarchy children is realized via its children; only leaves must ground to code, so a roll-up parent is not reported ungrounded" leaf-grounding
gi "maturity counts only leaf intents as needing grounding" \
  src/maturity.rs "ladder:39 (parents from hierarchy from_id; skip in ungrounded count)" \
  "an implemented parent intent with grounded children is not reported ungrounded" \
  "maturity.rs ladder collects parent ids from hierarchy edges and continues past them when counting ungrounded leaves"

leaf "find surfaces each matched intent's grounding" \
  "loom find prints the file, locator and verdict of every implements edge under a matched intent, not just the node name — the edge a plain text search lacks" find-surface
gi "find surfaces each matched intent's grounding" \
  src/commands/misc_cmd.rs "find_cmd:810 (walks edges_with Implements; prints path @ locator [verdict])" \
  "find on an intent term prints its grounded path + locator + verdict lines beneath the match" \
  "misc_cmd.rs find_cmd follows implements edges and reads the locator facet; verified by the cold-start test that find beats grep"

rel "a symbol-pattern name is refused as an intent" \
    "find surfaces each matched intent's grounding" \
    "both are command-surface behaviors in the src/commands/ family"

# ---- quality + a runnable proof ---------------------------------------------

"$B" rule seed iso5055 >/dev/null
"$B" rule verdict "iso5055-sec-no-hardcoded-secrets" \
  "the verdict write boundary enforces the evidence, truth-class and lane gates" \
  passing --criterion "no literal credentials in the persistence layer" \
  --evidence "store.rs uses parameterized SQL and SQLite functions; no secrets in source" --confidence 0.9 >/dev/null

"$B" validation add --name "cargo test suite" --type test --command "cargo test -q" \
  --intent "the verdict write boundary enforces the evidence, truth-class and lane gates" >/dev/null
# run it for real and record the observed result (honest proof, not an assertion)
"$B" validation run "the verdict write boundary enforces the evidence, truth-class and lane gates" >/dev/null

"$B" export >/dev/null

echo "=== status ==="; "$B" status
echo "=== doctor ==="; "$B" doctor
echo "=== smells ==="; "$B" smells
echo "=== export --check ==="; "$B" export --check
