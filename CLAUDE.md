# loom — LLM Handoff Document

You are working on **loom**, a CLI tool that helps LLMs systematically understand and clean up codebases. Read this entire document before touching any code.

## What loom is

loom builds a **living intent graph** of a codebase. Nodes are "intents" — what pieces of code are supposed to do. Edges are the relationships between intents, each with a verification status. Any LLM drives loom via structured CLI commands to discover, understand, and fix codebases autonomously.

The first dogfood target is loom's own codebase.

## The three planes

The graph spans three planes:

- **Semantic plane** — `Intent` nodes. What the system is supposed to do.
- **Physical plane** — `CodeFile` nodes. What actually exists on disk.
- **Normative plane** — `QualityRule` nodes. What good looks like.

Edges connect planes. The graph is only useful when all three planes are populated and connected.

## Node types (4)

### Intent
The core node. Everything orbits this.
```
id                  STRING (uuid)
name                STRING
description         STRING
abstraction_level   STRING  -- "feature" | "component" | "system" | "cross_cutting"
domain              STRING
source_refs         STRING  -- JSON array of file paths
status              STRING  -- "proposed" | "confirmed" | "deprecated"
created_at          STRING
updated_at          STRING
```

Abstraction levels:
- `system` — 1–3 per codebase. The whole product's purpose.
- `component` — 5–15. Cohesive subsystems (DB layer, CLI layer, etc.)
- `feature` — many. Atomic, independently verifiable ("loom init is idempotent").
- `cross_cutting` — spans everything (error handling, JSON output mode, atomicity).

Right granularity test: can you write a falsifiable criterion for an edge involving this intent? If yes, the granularity is right.

### CodeFile
Bridges the semantic plane to the physical.
```
id              STRING (uuid)
path            STRING  -- absolute path
language        STRING
last_modified   STRING  -- mtime from filesystem, updated by loom sync
imports         STRING  -- JSON array of statically-imported repo paths (loom sync)
content_hash    STRING  -- FNV-1a 64 hex of the bytes; sync's change detector
                           (mtime churn from checkout/rebase never false-flags)
```

### QualityRule
Named, reusable anti-pattern rules. Not generic linter rules — specific to this codebase.
```
id                  STRING (uuid)
name                STRING  -- e.g. "no_redundant_abstraction"
description         STRING
detection_logic     STRING  -- how to detect a violation
severity            STRING  -- "warning" | "error"
```

### Validation
Explicit proof object that an intent is fulfilled. Intents without validations are risky.
```
id               STRING (uuid)
name             STRING
description      STRING
validation_type  STRING  -- "test" | "assertion" | "benchmark" | "manual_check"
command          STRING  -- e.g. "cargo test --test foo"
last_run         STRING  -- timestamp
last_result      STRING  -- "passed" | "failed" | "not_run" | "blocked"
                            (blocked = recorded "can't run yet" + reason; out of
                             the validator queue/compass, visible in report,
                             sticky across sync — code changes don't unblock)
```

## Edge types (5)

Design principle: edge **type** describes the nature of the relationship (structural, stable). Edge **state** describes what we know about it (epistemological, mutable).

### RELATES_TO (Intent ↔ Intent)
The main workhorse. Represents any relationship worth tracking between two intents. This is the "cell" in the N×N intent grid.

### HIERARCHY (Intent → Intent)
Parent/child. Feature rolls up to component, component to system. Enables zoom in/out during traversal.

### IMPLEMENTS (Intent → CodeFile)
Traces an intent to its actual code location. The bridge from semantic to physical.

### GOVERNS (QualityRule → Intent)
A quality rule applies to this intent — the **green gate**. Replaces both the old MUST_COMPLY_WITH and VIOLATES — same relationship, different states. Carries the full inspectable-edge meta (criterion/confidence/evidence/last_inspected/inspected_by); defaults `uninspected` (compliance is earned, not assumed).

### VALIDATES (Validation → Intent)
A validation proves that this intent is fulfilled. Intents with no VALIDATES edges have no proof of correctness.

## State machine (on the INSPECTABLE edges)

`inspection_status` is the heartbeat — on the edges that represent a *claim
verified against code*: RELATES_TO (analyzer), IMPLEMENTS (analyzer), GOVERNS
(quality), VALIDATES (validator). As of schema **v3**, HIERARCHY does NOT carry
one: it's a structural tree edge, enforced at insert (unique parent, no cycles),
never "inspected".

```
uninspected          declared but never verified against actual code
passing              inspected, criterion met
failing              inspected, criterion violated
independent          inspected, confirmed no relationship (RELATES_TO: intents
                     unrelated; GOVERNS: rule does not apply to this intent)
needs_reverification was passing/failing, but adjacent code changed — stale
```

Flow: `uninspected → passing | failing | independent ↔ needs_reverification`

GOVERNS defaults to **uninspected** (v3): applying a rule asserts it *applies*,
not that the intent *complies* — green is earned via `loom rule check`. An
uninspected/failing GOVERNS drives the compass `quality` phase.

## Agent roles & field ownership (v3)

Every schema field declares its owning **agent role** — the primary writer —
encoded in `required_node_props`/`required_edge_props` and surfaced by
`loom schema` (`name [owner]`). Roles: `builder` (constructs the graph),
`analyzer` (grounds edges), `fixer` (resolves failing/needs_change), `validator`
(runs validations, confirms intents), `quality` (the green gate), plus `loom`
(computed) and `any` (the shared `notes` channel). Roles map onto work modes
(builder→build, analyzer→discovery, fixer→fix, validator→validate,
quality→refactor-to-green) so `loom next` routes by lifecycle stage.

Provenance carries the role: `inspected_by`/`author` resolve from an explicit
flag → `$LOOM_AGENT` env (e.g. `LOOM_AGENT=llm:analyzer`) → `"llm"` (see
`src/agent.rs`). The schema's *owner* says who SHOULD fill a field; provenance
says who DID — a mismatch flags self-inspection (weaker claim).

**Lanes are ENFORCED (`src/gate.rs`).** An agent that declares a role is held
to that role's lane at the command boundary: builder constructs (intent add,
hierarchy, codefile, implement), analyzer grounds (explore ground/issue/
independent), fixer resolves (edge fix, lifecycle needs_change→implemented),
validator proves (validate, intent confirm), quality gates (rule add/apply/
verdict). Out-of-lane → hard error pointing at the offender's own queue. Bare
`llm`/`human` = solo mode (all lanes pass — single-agent driving stays
supported). Unknown roles are rejected, not bypassed. Evidence gates reject
vacuous criterion/evidence/notes (≥10 chars, no placeholders) and confidence
outside [0,1]; `loom doctor` re-audits all of it after the fact, including
out-of-lane provenance on recorded verdicts.

**Key design decision:** `independent` is a state on RELATES_TO, not a separate edge type. A RELATES_TO edge with `inspection_status = independent` means "we looked, there is no meaningful relationship." This is a verified claim, not an absence. It is as important as `passing`.

## State vs metadata separation

Two namespaces on every edge, both as flat properties:

**State** (workflow-critical, drives loom next and ripple propagation):
```
inspection_status
```

**Meta** (evidence layer, accumulative, explains how we arrived at the state):
```
criterion       what "passing" looks like for this specific edge
confidence      DOUBLE 0.0–1.0
evidence        what was found during inspection
last_inspected  STRING timestamp
inspected_by    "human" | "llm"
priority_score  DOUBLE computed
notes           STRING
```

The workflow engine reads `inspection_status`. The Socratic loop reads and writes the meta properties.

## Priority scoring (loom next)

```
priority_score = degree(intent_a) + degree(intent_b)   -- centrality/impact
              + urgency(inspection_status)               -- failing > needs_reverification > uninspected
              - staleness_penalty(last_inspected)        -- older = lower priority (stale but not urgent)
```

High-centrality intents surface first. Failing edges before ungrounded ones. The agent never manually decides what to work on.

## Ripple propagation

When code changes, the graph propagates the impact:

```
loom sync detects content change (content_hash differs; mtime is only the
                                  never-hashed fallback — checkout churn is quiet upkeep)
→ CodeFile.last_modified + content_hash updated
→ Those intents' RELATES_TO neighbors → needs_reverification (one hop)
→ Those intents' passing GOVERNS edges → needs_reverification
  (quality green is a claim about the old code — it must be re-earned
   via `loom next --mode quality` + `loom rule verdict`)
→ VALIDATES edges on those intents → Validation.last_result = not_run
  (blocked validations are NOT flipped — a code change doesn't unblock them)
(IMPLEMENTS edges are structural assertions, used as the index — not flipped.
 Every flipped edge gets a transition note naming the changed file, so a stale
 edge explains itself: "passing → needs_reverification (sync: src/foo.rs changed)".)

Files registered in the graph but MISSING on disk (deleted/renamed) are
reported by sync, never skipped silently — drop phantoms with
`loom codefile remove <path-or-id>` (kills its IMPLEMENTS edges; intents
grounded only there become unrealized again and the compass routes to ground).
```

The graph structure IS the impact analysis. No custom algorithm — just edge traversal with state transitions.

## Commands reference

```
loom init [path] [--name <graph-name>] [--observed]
  Creates .loom/ directory, initializes Grafeo DB with full schema, and stamps
  the graph's IDENTITY (graph_id uuid + human name, default = dir name) — what
  other looms reference in a federation; it travels in the export.
  --observed = this graph maps code its drivers DON'T own (vendor SDK, another
  team's service): discovery/quality/validation all work, but build/fix lanes
  are disabled (custody gate) — findings, not fixes. Idempotent — re-running is
  safe and backfills identity on older graphs (also the way to set --name/
  --observed later).

loom status
  Graph stats: intent count, edge counts by inspection_status, open issues.

loom sync [path]
  THE PROGRAMMATIC FLAG ENGINE.
  Walks CodeFiles and detects CONTENT changes (content_hash; mtime is only the
  first-run fallback — checkout/rebase timestamp churn never false-flags),
  then propagates needs_reverification. Every flipped edge gets an append-only
  transition note naming the changed file ("passing → needs_reverification
  (sync: src/foo.rs changed)") — staleness explains itself in `loom edge show`
  and `loom next`. Output: N files changed, M edges flagged, K validations
  invalidated. LLM calls this after any code change, then calls loom next.

loom next --all
  THE CLOSEOUT VIEW: every role queue at once — counts + top item per queue
  (build/fix/ground/validate/quality/discovery, in handoff order), vertical-
  completeness gaps, doctor health, and top smells, as ONE prioritized list.
  The single operational answer to "what's left?" — no reconciling five
  commands by hand. Discovery is flagged optional (horizontal axis).

loom next [--mode discovery|fix|build|validate|quality]
  One queue per agent role:
  discovery = inspect relationships (analyzer) · fix = resolve failing/stale
  RELATES_TO (fixer) · build = realize planned/needs_change intents (builder) ·
  validate = failing/unrun/missing proofs (validator) · quality = uninspected/
  failing GOVERNS edges PLUS never-measured rule×intent pairs (synthetic
  `unmeasured` items, surfaced at the highest unmeasured altitude only — one
  `loom rule verdict` resolves each, creating the edge with the verdict).
  Returns single highest-priority work item with FULL context:
  - Edge (type, inspection_status, criterion, evidence, priority_score)
  - Both intent nodes (name, description, abstraction_level, source_refs)
  - Related CodeFiles (paths, last_modified)
  - VALIDATES edges on those intents (validation name, last_result)
  - Suggested action
  No second lookup needed. LLM can act immediately.

loom intent add --name --description --level [--domain] [--source ...]
loom intent add ... [--aspect happy|sad|fallback|…] [--lifecycle planned|implemented|needs_change]
loom intent confirm <id>
loom intent mark <id> --lifecycle planned|implemented|needs_change [--reason "<why>"]
  Set the prescriptive lifecycle. needs_change = a known issue/refactor (honest,
  no faked verdict); --reason is recorded as a note. Feeds `loom next --mode build`.
loom intent delete <id>          (remove a mistake: node + its edges + notes)
loom intent source add <id> <path>     (append to source_refs — docs AND code:
                                        contracts, ADRs, design notes; idempotent)
loom intent source remove <id> <path>
loom intent list [--status] [--level]
loom intent show <id>            (intent + edges + hierarchy + implements + notes)

loom edge explore <a-id> <b-id>
  Prints both intents + source_refs. Creates edge if not exists.
  Subcommands:
    ground --criterion --confidence [--inspected-by]
    issue --criterion --evidence [--inspected-by]
    independent --notes
    fix --description

loom edge list [--status]
loom edge show <edge-id>

loom cluster <intent-id>
  All unresolved edges touching this intent. For batching neighborhood work.

loom codefile add <path>          (or a glob: 'src/**/*.rs')
loom codefile list
loom codefile show <path-or-id>
  The per-file OWNERSHIP view: which intents claim it (level + locator +
  status), which quality rules reach it through them, its imports, and a
  tangled flag (≥3 intents). The answer hotspots only hint at.
loom codefile remove <path-or-id> (drop a phantom after delete/rename on disk;
                                   removes its IMPLEMENTS edges too)

loom validation add --name --type [--command] [--description] [--intent <id>]
  --intent links the new Validation to an intent (VALIDATES) in one step;
  omit it to link later with `loom edge validates <validation-id> <intent-id>`.
loom validation mark <id|name> --result passed|failed --evidence "<what you checked>"
loom validation mark <id|name> --result blocked --reason "<what it is waiting on>"
  Record a verdict BY HAND for a manual_check / async proof that has no runnable
  --command (which `loom validate` would otherwise skip). Validator-lane; evidence/
  reason must be substantive. Updates last_result + the per-intent VALIDATES verdict.
  `blocked` = honest "can't run yet" (live target down, missing credential): leaves
  the validator queue + compass, stays visible in `loom report`, survives sync
  (a code change doesn't unblock it). Re-mark passed/failed to unblock.
loom validation update <id|name> [--command "<cmd>"] [--description "<text>"]
  Fix a wrong definition (e.g. a bad cargo package in --command). A changed
  command RESETS the proof — last_result → not_run, VALIDATES edges →
  uninspected — because the old result proved a different command.
loom validation delete <id|name>
  Remove a mistake (the validation analogue of `intent delete`): node +
  VALIDATES edges + their notes. Intents that lose their only proof resurface
  in `loom next --mode validate`.
loom validation list [--intent <id>]

loom validate <intent-id>
  Runs command on all VALIDATES edges for this intent. (manual_check without a
  command is skipped — use `loom validation mark` for those.)
  Updates Validation.last_result and VALIDATES edge inspection_status.

loom rule add --name --description --severity
loom rule list
loom rule apply <rule-id> <intent-id>   (positional; creates GOVERNS edge, uninspected)
loom rule check <intent-id>             (read-only: show GOVERNS edges by status)
loom rule verdict <rule-id> <intent-id> --status passing|failing|independent \
    --criterion "<what compliance looks like>" --evidence "<what was found>" \
    [--confidence 0.9] [--inspected-by llm:quality]
  THE quality write path — how GOVERNS green is earned. The verdict IS the
  measurement: if no GOVERNS edge exists yet, it is CREATED with the verdict
  (no separate `apply` needed — `apply` remains for pre-declaring "this rule
  applies" without a verdict). independent = measured, rule doesn't apply.
  Quality lane; criterion/evidence must be substantive.

loom report [--format json|text]
  Full coverage: edge counts by status across all types, intents without validations,
  failing GOVERNS, validation pass rate, recent passing edges.

loom batch [file|-]
  Bulk verdicts from JSON Lines (default stdin) — THE post-sync re-verification
  surface: a sync that stales 30 claims is one `loom batch` call, not 30
  invocations. Ops per line: ground / issue / independent (RELATES_TO) and
  rule_verdict (GOVERNS, creates the edge if absent). EVERY gate applies per
  line — lanes, substantive criterion/evidence/notes, confidence — and each
  edge still gets its transition note. Continues past failed lines, reports
  per-line results, exits non-zero if any failed. Bulk changes the ceremony,
  never the honesty.

loom note add --text <text> [--kind <kind>] [--intent <id> | --edge <id>] [--author human|llm]
  Append free-text memory. kind: justification | commentary | idea | question | decision | todo.
  Attach to an intent, an edge, or leave free-floating. Append-only (never overwritten).
  Notes surface in `loom next`, `loom intent show`, and `loom edge show`.
loom note list [--intent <id>] [--edge <id>] [--kind <kind>]

loom doctor
  Verify graph integrity against the declared schema (src/db/schema.rs):
  schema version, required-property presence, valid field values, dangling
  references, and the evidence audit behind every verdict — vacuous criterion,
  confidence outside [0,1], confidence still 0.0 behind passing/failing, empty
  last_inspected behind a verdict, out-of-lane provenance. Also emits advisory
  HINTS (never fail the exit code): all-solo provenance (declare roles for real
  separation of duties), and a stale committed loom.graph.json.
  Exits non-zero if any issue is found. Run after upgrades or if results look wrong.

loom guide [--mode greenfield|brownfield|refactor]
  Self-contained driving protocol for an LLM new to loom: mental model, the loop,
  the done-condition, and a MODE-SPECIFIC population checklist (auto-detected via
  `loom detect` if --mode omitted): greenfield = design-as-planned-intents then
  build; brownfield = map & verify existing; refactor = flag needs_change & change.

loom schema
  The data model — node/edge types + properties, the inspection state machine,
  and the valid value vocabularies. Generated from the schema vocabulary (drift-proof).

loom hotspots [--limit N]
  Structural importance (graph centrality, NOT runtime profiling): most-central
  intents (blast radius) and most-tangled files (most intents in one file).

loom smells [--limit N]
  Derived problem signals — the graph as instrument, not ledger. Computed from
  structure alone (no LLM judgment in the flagging): twin intents (split-brain:
  same level, similar wording, no edge), overlapping ownership (two intents
  claim the same file, no edge), scattered intents (level-aware thresholds;
  the evidence GROUPS the grounded files BY DIRECTORY — the mechanical
  clustering for a decompose: loom shows where the files cluster, the LLM
  names the children), tangled files (≥3 intents — per-file detail via
  `loom codefile show`), undeclared coupling (file A imports file B but
  their intents have no edge — physical evidence vs semantic graph), recurrent
  trouble (a target whose transition history keeps returning to failing/
  needs_change — redesign, don't re-patch; a kind=decision note NEWER than the
  last regression resolves the finding without erasing history, and a later
  regression re-flags it), unmeasured intents (a QualityRule
  was never held against a coded intent — HIERARCHY-AWARE: a verdict on a
  component covers its descendants, so measure at the highest honest altitude
  instead of grinding per-leaf busywork; a leaf can still get its own, more
  specific verdict), unused rules, happy-path-only groups (children declare an
  `--aspect happy` but no sad/fallback sibling — failure behavior undeclared).
  Each finding carries the exact remedy command. The same suspicion signals
  (import links, shared files, description overlap, same domain) rank
  unexplored pairs in `loom next` discovery, with the why in the work item's
  notes. `loom rule verdict --status independent` records "measured — rule
  does not apply" so unmeasured findings resolve honestly.

loom rule seed iso5055|mobile|web-ui|service|data|concurrency
  Seed a built-in measuring-stick pack — the repo-kind VANTAGE POINTS for 360°
  normative coverage, each rule written for LLM inspection (detection_logic
  says exactly what to look for). Idempotent (existing names skipped).
  iso5055 = baseline, applies to any code (10 CWE-grounded rules across
  Reliability/Security/Performance/Maintainability) · mobile = lifecycle,
  offline, permissions, main thread, battery, platform divergence, deep links ·
  web-ui = view states, a11y, XSS, client-side trust, feedback, responsive,
  URL state · service = contract artifacts, idempotency, timeouts/retries,
  saga compensation, boundary auth, observability, degradation, compatible
  evolution · data = migrations, ingest validation, loss accounting, PII,
  rerun idempotency, lineage · concurrency = sync discipline, no lock across
  I/O/await, atomic multi-step, deadlock ordering, cancellation safety,
  bounded concurrency, plus perf-budget-proven (hot-path intents must state a
  budget in their criterion AND carry a passing benchmark validation — the
  normative plane demanding proof in the validation plane).
  `loom detect` recommends which packs fit this repo. After seeding,
  `loom next --mode quality` serves every never-measured rule×intent pair.

loom export [path]                    (default loom.graph.json; "-" = stdout;
                                       positional, mirroring `loom import <file>`;
                                       --out <path> still accepted)
loom export --check
  THE COMMIT GUARD: verify the existing export matches the live graph
  byte-for-byte (determinism makes freshness a byte comparison). Exits
  non-zero on drift or a missing file — hook it into pre-commit/CI so a graph
  change can never silently ship without its travel format.
loom import <file>
  The graph's travel format: deterministic JSON (same graph → identical bytes)
  meant to be committed so the graph travels with the repo and graph changes
  are diffable in PRs. Import restores into a fresh `loom init` (never merges);
  run `loom sync` after to reconcile with the machine's files.

Every verdict transition (ground/issue/independent/fix/rule verdict/lifecycle
mark) is auto-recorded as an append-only note (kind=transition) — the graph's
recurrence memory, read by the recurrent_trouble smell.

loom coverage
  Reconcile files on disk (respecting .gitignore) against the graph. Buckets each
  file: grounded (≥1 IMPLEMENTS) / delegated (owned by a child graph — federation) /
  excluded (matches an ignore pattern) / registered-but-ungrounded (unexplained
  code) / unaccounted (gap). Ensures nothing is silently missed. Done = no
  unaccounted (missing delegation targets are flagged).

loom detect
  Programmable repo introspection: stack (from manifests), source presence, top
  languages, suggested mode (greenfield vs brownfield), and RECOMMENDED QUALITY
  PACKS for this repo kind (each with its disk evidence) — the binary suggests
  the 360° vantage points so the agent doesn't have to remember them. Runs even
  before `loom init`.

loom ignore add <glob> --reason <why> [--author human|llm]
  The coverage escape hatch, stored IN the graph (not a .loomignore file) as a
  recorded, doctor-checkable decision. `.gitignore` is honored separately.
loom ignore list

loom delegate add <glob> --to <child-export-path>
loom delegate list
  FEDERATION (monorepo): a subtree owned by ANOTHER loom graph. `loom coverage`
  buckets matching files as `delegated` — covered by the child, verified against
  its committed export (a missing target is reported, never silently trusted).
  The root grounds seam intents in the children's exports; content-hash sync
  then ripples cross-service automatically. Data flows UP only (children
  export, parent observes) — a parent never writes into a child's graph.

(Discoverability extras: bare `loom` prints an orientation; `loom intent add` takes
 `--aspect happy|sad|fallback|…`; `loom edge implement … --locator "fn run"` grounds
 to a symbol; `loom codefile add 'src/**/*.rs'` bulk-registers via glob. `loom status`
 ends with a phase-aware "→ Next" compass, and status/next carry a `graph_state` pulse.
 Intents and rules are addressable by id, exact name, or unique name fragment.
 `loom edge implement <intent> 'src/db/**'` bulk-grounds over REGISTERED paths;
 `loom edge unimplement <intent> <path|glob>` is the ungrounding half — used to
 move groundings down to children when decomposing a scattered intent.)
```

All commands support `--json` for machine-readable output. LLM driving mode uses `--json` everywhere.

## Traversal strategy

Two distinct modes — do not mix:

**Discovery mode** (`loom next --mode discovery`)
Map reality against the intent graph. Visit ungrounded edges, inspect code, ground them or confirm independence. No fixes. Output: every edge has a status and criterion.

**Fix mode** (`loom next --mode fix`)
Target `failing` edges. Apply Socratic method, make minimal changes, verify. Output: violations resolved.

**The Socratic loop per edge:**
1. Read both intent descriptions + source_refs
2. Form hypothesis: "I expect the code to show X"
3. Inspect actual code
4. Confirm or deny
5a. Confirmed → mark passing, write criterion
5b. Denied + code wrong → mark failing, write evidence
5c. Denied + hypothesis wrong → revise, re-inspect
6. Zoom out check: traverse HIERARCHY upward — does this make sense at system level?
7. Fix mode: minimal change → verify → mark passing

**Clustering for efficiency:**
When you select an edge, pull all other unresolved edges sharing either node. Work the neighborhood while you have the context loaded. Context window is expensive; locality is free.

**Done condition (two axes — vertical is binding, horizontal is optional):**

The completeness model is split. The **vertical spine** is mechanically verifiable and is what "done" means:
```
VERTICAL (binding):
  - HIERARCHY is a well-formed TREE: each intent has ≤1 parent, no cycles
    (enforced at insert time in queries/hierarchy.rs; doctor re-checks).
  - Every implemented LEAF intent is realized: ≥1 IMPLEMENTS edge.
  - Every CodeFile is reached by ≥1 IMPLEMENTS.
  - On disk: `loom coverage` reports nothing unaccounted.
  → surfaced as `vertically_complete` in graph_state; details in
    queries/completeness.rs (vertical_completeness), `loom report`, `loom doctor`.

HORIZONTAL (optional closure — understanding/cleanup, NOT required for done):
  - Every intent pair has an inspected RELATES_TO edge
    (none uninspected / stale / unexplored).
  → surfaced as `horizontally_explored`; phase=complete means both axes done.
```
Compass routing (queries/stats.rs `graph_state`) prioritises vertical gaps
(`ground`/`incomplete` phases) ahead of optional horizontal `discovery`, and only
emits `phase=complete` when both axes hold. The old "all edges passing/independent"
rule was a horizontal-only check that could hide unrealized intents and orphan
files — the vertical spine is the airtight part.

COHERENCE BY CONSTRUCTION: the compass routes each phase on the SAME selection
the corresponding queue serves (validate routes on `validate_selection`, shared
verbatim with `loom next --mode validate`; quality covers failing + stale +
uninspected + unmeasured exactly as `quality_candidates` does) — `loom status`
can never send an agent to a `loom next` that answers "nothing to do". The
invariant is tested per phase (compass_phase_always_has_a_nonempty_queue).
Consequences: implemented leaves WITHOUT proof route to `validate` (proven is
part of done), and stale GOVERNS green routes to `quality`.

**360° coverage vector** (`graph_state.coverage`, second line of the pulse
footer on every orientation command): five counted axes so the driving LLM
always sees which vantage point is weakest — `grounded` (CodeFiles reached) ·
`realized` (implemented leaves with code) · `explored` (the horizontal grid) ·
`measured` (rule×coded-intent pairs with a verdict, hierarchy-inherited) ·
`proven` (implemented leaves with a passed validation). An axis with no surface
shows `—` (never a vacuous 100%). The `measured` axis is queue-mandatory, not
write-mandatory: an EMPTY normative plane (no rules, coded intents) routes the
compass to quality ("seed a pack"), and never-measured pairs feed
`loom next --mode quality` as `unmeasured` items at the highest unmeasured
altitude — resolved in one `loom rule verdict` (which creates the edge);
`phase=complete` requires the measuring grid closed.

## Dogfooding setup

loom reviews itself. After building:
```bash
cd /Users/laptopdp/Developer/damarpanuluh/loom
./target/debug/loom init .
```

First three intents to add (loom's own codebase):
1. "CLI parsing and dispatch" (component) — src/cli.rs, src/commands/mod.rs
2. "Graph persistence via Grafeo" (component) — src/db/mod.rs, src/db/queries.rs
3. "Priority-scored work queue" (feature) — src/db/queries.rs, src/commands/next.rs

Then: `loom next` — it will return the first ungrounded edge to inspect.

## Codebase structure

```
src/
├── main.rs               entry point, command dispatch
├── cli.rs                clap CLI definitions (derive)
├── types.rs              all structs: Intent, CodeFile, QualityRule, Validation, Note,
│                         InspectionStatus, NoteKind, EdgeType, WorkItem, SyncReport, etc.
├── output.rs             dual-mode rendering (human / --json) + graph pulse
├── repo.rs               filesystem introspection: gitignore-aware walk + stack detection
├── agent.rs              acting-agent resolution for provenance (--by / $LOOM_AGENT / "llm")
├── gate.rs               the enforcement layer: role lanes (declared role held to its
│                         lane; solo mode passes) + evidence gates (substantive
│                         criterion/evidence/notes, confidence ∈ [0,1])
├── db/
│   ├── mod.rs            Grafeo DB connection (single long-lived Session), LoomDb trait
│   ├── schema.rs         THE schema: vocabulary (labels/edges/props + per-field owner
│   │                     role), version, required-property tables, GQL escaping (`esc`)
│   └── queries/          query layer, split by concern (flat-re-exported from mod.rs)
│       ├── mod.rs          module wiring + flat re-export + #[cfg(test)] suite
│       ├── row.rs          shared value/row extraction helpers
│       ├── intent.rs       Intent node queries
│       ├── codefile.rs     CodeFile node queries
│       ├── rule.rs         QualityRule node queries
│       ├── validation.rs   Validation node queries
│       ├── note.rs         Note annotation queries (append-only memory)
│       ├── ignore.rs       Ignore patterns (coverage escape hatch, with reasons)
│       ├── meta.rs         LoomMeta sentinel: version + last_synced freshness
│       ├── completeness.rs vertical-completeness spine (tree + realization join)
│       ├── relates_to.rs   RELATES_TO edge (the intent grid)
│       ├── hierarchy.rs    HIERARCHY edge (structural tree, enforced at insert)
│       ├── implements.rs   IMPLEMENTS edge (carries `locator`)
│       ├── governs.rs      GOVERNS edge
│       ├── validates.rs    VALIDATES edge
│       ├── scoring.rs      priority scoring + discovery candidates (loom next)
│       ├── stats.rs        counts / centrality / graph_state pulse / completeness gaps
│       └── integrity.rs    graph integrity checks (loom doctor)
└── commands/
    ├── init.rs           loom init
    ├── status.rs         loom status (+ compass + pulse)
    ├── intent.rs         loom intent *
    ├── edge.rs           loom edge *
    ├── next.rs           loom next
    ├── cluster.rs        loom cluster
    ├── sync.rs           loom sync (flag engine; stamps last_synced)
    ├── validate.rs       loom validate
    ├── codefile.rs       loom codefile * (glob-aware add)
    ├── validation.rs     loom validation *
    ├── note.rs           loom note *
    ├── rule.rs           loom rule *
    ├── report.rs         loom report (+ completeness gaps)
    ├── doctor.rs         loom doctor
    ├── guide.rs          loom guide
    ├── schema.rs         loom schema
    ├── hotspots.rs       loom hotspots
    ├── coverage.rs       loom coverage
    ├── detect.rs         loom detect
    └── ignore.rs         loom ignore *
```

## Build

```bash
cd /Users/laptopdp/Developer/damarpanuluh/loom
cargo build
```

The `grafeo-engine` crate statically links at build time. First build is slow.

Run `cargo test` for the query-layer regression suite (in `db/queries/mod.rs`),
which covers the relationship reliability rule below.

## Key design decisions (why, not just what)

**Why graph DB (Grafeo) not SQLite:** Blast radius, centrality, and ripple propagation are native graph traversals. SQLite fights you on this with JOIN chains. Grafeo is pure Rust, embedded, zero server.

**Why `independent` is a state not an edge type:** Independence is a verified claim about a relationship, not a different kind of relationship. Encoding it as state keeps the schema clean and queries uniform.

**Why Validation is a node not a property:** Validations are reusable, runnable, and have their own lifecycle (last_run, last_result). Nodes are the right abstraction for entities with identity and state.

**Why separate state from meta:** State drives the workflow engine. Meta provides evidence. Keeping them conceptually distinct prevents the agent from confusing "what do we know" with "how do we know it."

**Why loom sync propagates one hop:** Full graph propagation from a file change would be too aggressive — everything would reset. One hop (IMPLEMENTS → RELATES_TO neighbors) is the right blast radius for a file-level change. System-level changes require explicit re-initialization.

**Why edges are matched by endpoints, never by their own id:** grafeo 0.5.x does not reliably match/filter a relationship by its own property (`r.id`, `r.inspection_status`) — inline `{id: X}`, `WHERE r.id = X`, and `WHERE r.inspection_status = X` all return results nondeterministically, and a read right after writing the same relationship is especially flaky. A full traversal (`MATCH (a)-[r]->(b) RETURN r.*`) and node-property matching are reliable. So: edge updates take endpoint ids and match `MATCH (a {id})-[r]->(b {id}) SET ...` (RELATES_TO/IMPLEMENTS/GOVERNS/VALIDATES are unique per endpoint pair); id/status lookups scan all and filter in Rust; and after a write we construct the result struct in Rust instead of re-reading. This is also why `db/mod.rs` holds one long-lived `Session` (read-your-writes within a session) rather than one per statement.
