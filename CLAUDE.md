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

## Node types (6)

### Intent
The core node. Everything orbits this.
```
id                  STRING (uuid)
name                STRING
description         STRING
abstraction_level   STRING  -- "feature" | "component" | "system" | "cross_cutting"
domain              STRING
source_refs         LIST    -- file paths (native list since schema v5)
status              STRING  -- "proposed" | "confirmed" | "deprecated"
created_at          STRING
updated_at          STRING
tags                LIST    -- registered VocabTerm names (≤3, sorted; empty/absent
                               = untagged). Native list since schema v5; validated
                               against the registry at write time.
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
imports         LIST    -- statically-imported repo paths (loom sync; native
                           list since schema v5)
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
validation_type  STRING  -- "test" | "assertion" | "benchmark" | "manual_check" |
                            "saga" (consumer-plane chain run by the built-in
                             engine — see `loom saga`; the spec path lives in a
                             `spec:<path>` line of the description)
command          STRING  -- e.g. "cargo test --test foo"
last_run         STRING  -- timestamp
last_result      STRING  -- "passed" | "failed" | "not_run" | "blocked"
                            (blocked = recorded "can't run yet" + reason; out of
                             the validator queue/compass, visible in report,
                             sticky across sync — code changes don't unblock)
```

### Hypothesis
The PRE-DECISION plane: an improvement proposal that must be proven before it
becomes work. Invisible to coverage/completeness/queues until adopted —
speculation never counts as state of the world.
```
id                 STRING (uuid)
name               STRING
claim              STRING  -- what's wrong NOW (falsifiable, provable against code)
proposal           STRING  -- the proposed change
predicted_outcome  STRING  -- measurable result if adopted (the post-implementation
                              acceptance contract)
status             STRING  -- "proposed" | "supported" | "refuted" | "adopted" |
                              "confirmed" | "rejected"
author             STRING  -- proposer provenance
evidence           STRING  -- what the prover found ("" until proven)
inspected_by       STRING  -- prover provenance (must differ from author when
                              both declare roles — proposer ≠ prover)
last_inspected     STRING
created_at / updated_at
```
State machine: `proposed → supported | refuted → adopted → confirmed | rejected`.
Anyone proposes (evidence-gated); the ANALYZER lane proves (the verdict also
stamps every TARGETS edge: supported→passing, refuted→independent); the
BUILDER lane decides. Adoption is pure conversion: link spawned `planned`
intents — lineage decision notes travel both ways, and the predicted outcome
is written as a not_run manual_check Validation (description carries a
`hypothesis:<id>` line) VALIDATES-linked to each spawned intent — then the
ordinary build/validate machinery owns the work. When the validator marks that
outcome validation passed, the hypothesis derives `confirmed`: every adopted
improvement gets checked for whether it actually delivered. Additive in
schema v3 (older graphs/exports keep working); a PORT (`--as-planned`) resets
supported/refuted→proposed and confirmed→adopted (earned evidence stays
behind; decisions travel as lineage).

### VocabTerm
The bounded tag vocabulary — a registry of KEYS, not a knowledge plane: no
edges, no lifecycle, no inspection state. Its value is forced collision: two
agents describing the same responsibility in open prose rarely share words,
but picking from a small inlined registry they collide — and collisions are
what the `duplicated_responsibility` smell and discovery ranking consume.
Collision strength is rarity-weighted (Σ 1/freq over shared terms), so broad
spammed terms decay toward zero on their own. Drift is DETECTED (`vocab_drift`
smell) and converged (`loom vocab merge` — one sweep, nothing to re-inspect),
never prevented by a closed list. Tags stay OPTIONAL: an untagged intent is
honest, a wrong tag lies.
```
id           STRING (uuid)
name         STRING  -- the term: lowercase [a-z0-9_-]+, the key intents carry in `tags`
description  STRING  -- contrastive: what it covers AND what it does not (names the neighbour)
author       STRING
created_at   STRING
```

## Edge types (6)

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

### TARGETS (Hypothesis → Intent)
Which intents an improvement hypothesis would touch. Carries the full
inspectable-edge meta (criterion/confidence/evidence/…), defaults `uninspected`
— per-target grounding and sync staleness are the v3 slice of the plane.

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
                 (degree counts REAL relationships only: `independent` edges
                  give the grid closure but add NOTHING to blast radius, and
                  edges touching retired intents are excluded entirely)
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
→ Passing TARGETS edges on hypotheses aimed at those intents → needs_reverification
  (hypothesis support was earned against the old target code — the triage
   queue serves the supported hypothesis as a RE-PROVE item; re-proving
   re-stamps the edges)
(IMPLEMENTS edges are structural assertions, used as the index — not flipped.
 Every flipped edge gets a transition note naming the changed file, so a stale
 edge explains itself: "passing → needs_reverification (sync: src/foo.rs changed)".)

Files registered in the graph but MISSING on disk (deleted/renamed) are
reported by sync, never skipped silently — drop phantoms with
`loom codefile remove <path-or-id>` (kills its IMPLEMENTS edges; intents
grounded only there become unrealized again and the compass routes to ground).
```

The graph structure IS the impact analysis. No custom algorithm — just edge traversal with state transitions.

## The LLM-driver output contract

loom's only user is an LLM agent on a long horizon: every output is the prompt
for the agent's next decision, and after context compaction loom's output is
the only memory that survives. Four invariants, enforced by shared helpers in
`src/output.rs` (`print_anchor`, `with_anchor`, `pulse_json`, `more_marker`,
`apply_limit`, `SECTION_CAP = 10`, `LIST_LIMIT = 50`) — every new command MUST
follow them:

1. **Anchor after mutation.** Phase-moving verdicts (edge ground/issue/
   independent/fix, validation mark, rule verdict, sync, import, saga run,
   validate, intent confirm/retire) end with `→ Next: <runnable command>` +
   the two-line pulse in human mode, and `next_step` + `graph_state` fields in
   json. Construction steps called in rapid mapping loops (implement,
   hierarchy, tag, codefile add, vocab add, …) get a LIGHT anchor — the
   `next_step` line/field without the pulse, so loops don't drown in repeated
   state. An agent never needs a separate `loom status` to know where it
   stands.
2. **Human/json parity.** Whatever guidance human mode prints, json carries
   (orchestrated agents run `--json`; a hint that lives only in `println!` is
   invisible to them). List commands always emit
   `{"<noun>s": [...], "total": N, "truncated": bool}` — never a bare array,
   never a data-dependent shape.
3. **Bounded output.** Anything that scales with graph size is capped:
   inventory lists honor `--limit` (default 50, `0` = all), sub-sections
   inside work items and show views cap at `SECTION_CAP` keeping the NEWEST
   notes (addressed-to-role notes always survive the cap), and every
   truncation prints `… +N more — <runnable fetch command>` (an affordance,
   never an apology). Errors teach: a failure names the corrective command or
   inlines the valid choices — never a bare "not found". SYNTAX failures are
   covered systematically (cli.rs `parse_or_teach` + commands/mod.rs
   `teach_unknown`):
   - every clap parse error (missing flag, bad value, stray positional)
     reprints the failing command's EXAMPLE after_help under the error — an
     EXAMPLE block IS the command's error message;
   - a ratchet test (`every_flag_requiring_command_ships_an_example`) fails
     the build when a command with required flags lacks an EXAMPLE, so new
     commands can't ship friction-shaped;
   - unknown top-level tokens land in an `external_subcommand` catch-all:
     noun-less verbs and synonyms (`update`/`rename`/`retire`/`add`/`ground`/
     `prove`/…) answer with the real invocation and the agent's own argument
     spliced in; typos get a real edit-distance suggestion (clap's stock tip
     once mapped `update` → 'guide');
   - `intent update` additionally catches positional wording and a missing
     --reason with the full shape (evolved / --reword / rename).
4. **Surface, then dig.** Payloads embed PROJECTIONS — the fields the next
   decision needs — never full records: work items carry the `*Surface` types
   from `src/types.rs` (intent without timestamps, grounding as
   path/locator/status, notes deduplicated with a `×times` count), the json
   `graph_state` field is `pulse_json` (the two human pulse lines,
   structured), and `--json` prints COMPACT (pretty-printing is token spend).
   Every elision names its runnable dig command: `loom intent show`,
   `loom edge show`, `loom note list`, `loom status --json` (the one place
   the FULL GraphState travels). Same reason `loom next --take` template
   lines omit `criterion`: `loom batch` reuses the recorded one, so neither
   loom nor the agent re-transmits text the graph already holds.

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
  `uninspected_outside_queues` names the uninspected edges NO queue serves
  (structural IMPLEMENTS, blocked proofs), so the raw histogram always
  reconciles with `unresolved_edges`. Coverage math is an identity:
  explored_pairs.total = covered + pending(uninspected/stale pairs) +
  unexplored_pairs (all over ACTIVE intents).

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

loom next [--mode discovery|fix|build|validate|quality|review|triage|align] [--take N] [--compact]
  One queue per agent role:
  discovery = inspect relationships (analyzer) · fix = resolve failing/stale
  RELATES_TO (fixer) · build = realize planned/needs_change intents (builder) ·
  validate = failing/unrun/missing proofs (validator) · quality = uninspected/
  failing GOVERNS edges PLUS never-measured rule×intent pairs (synthetic
  `unmeasured` items, surfaced at the highest unmeasured altitude only — one
  `loom rule verdict` resolves each, creating the edge with the verdict) ·
  review = verdicts recorded with confidence < 0.7, ranked by
  (1−confidence)×centrality — THE TIERED DOUBLE-CHECK: a low-capability scout
  records honest uncertainty and the graph itself routes exactly those claims
  to a stronger reviewer (independent re-inspection: form your own hypothesis
  BEFORE reading the recorded evidence; re-record to confirm ≥0.7 or overturn).
  Optional like discovery — review hardens closure, it never blocks complete.
  align = the validator's user↔intent drift queue: intents ranked by
  churn-since-confirm × centrality × staleness — code moved under a meaning the
  user never re-affirmed. Intents ruled visibility=internal are NEVER served
  (machinery isn't interview material) until a redefinition clears the ruling.
  The item is a CONCEPT-alignment move, not a wording check: it carries
  `visibility` (user_visible | internal | untriaged), `where_it_sits` (the
  parent chain — why it matters), and `not_to_confuse_with` (siblings +
  verified-independent neighbours), and the scaffold says to present what the
  product can DO, why it matters, and the audience UP FRONT — machinery
  presented as a product capability is how interviews go wrong. Vocabulary
  enters only when the user asks, stumbles, or collides with the graph. The
  description stays graph-speak source material — never read it aloud; on
  "evolved" translate the user's answer back into a falsifiable description.
  The user rules on BEHAVIOR, never on wording. Exactly one outcome lands:
  `loom intent confirm` (concept still right — resets the suspicion clock) /
  `confirm --visibility internal` (machinery — stop asking until redefined) /
  `update --reword` (words confusing, concept right — no ripple, clock resets) /
  `update` (concept evolved) / `retire --replaced-by` (superseded) /
  `add --lifecycle planned` (missing concept revealed).
  Optional like discovery — the graph can't read heads; this is the human gate.
  triage = the pre-decision plane's queue (analyzer, effort high), ranked by
  combined target-intent centrality (blast radius). Two item kinds, told apart
  by status: proposed = never proven (prove it) · supported with stale TARGETS
  = its support was earned against since-changed target code (re-prove or
  refute; re-proving re-stamps the edges). The work item carries the claim,
  targets, their groundings, and the prove command. Optional like
  discovery/review — speculation never blocks complete.
  --take N (discovery/fix/quality, capped 50) = the bulk READ half of the batch
  loop: N COMPACT items in ONE call instead of one rich item + anchor per call.
  discovery/fix group by the file that staled them (parsed from sync transition
  notes, indexed in one scan — never per-item) with prefilled `ground` template
  lines; quality groups by INTENT (one neighborhood read pays for every rule
  held against it) with prefilled `rule_verdict` lines and per-item effort from
  the rule's annotation. The token-bounded post-sync drain: read each hot
  neighborhood once, verdict its whole group via `loom batch`. Sync suggests
  `--take 20` when it flags >10 edges.
  --compact (discovery/fix) = the single-item PROJECTION: intent ids/names,
  edge id, top grounded paths, a ONE-LINE suggested command, owner_role/effort,
  and a `dig` pointer — no validations/notes/descriptions/pulse. For agents
  that already know the loop and only need the verdict coordinates ("intent
  alignment" runs live here: `loom next --compact --json`, verdict, repeat).
  EVERY work item carries `owner_role` AND `effort: low|mid|high` — effort
  names how much capability the WORK needs (computed from structure; quality
  items inherit the rule's inspection_effort). Loom never names models — the
  harness maps effort tiers to whatever models exist. The fix queue dispatches
  by item state: needs_reverification → analyzer/mid (re-inspection of an
  existing criterion), failing → fixer/high (repair).
  Returns single highest-priority work item with FULL context:
  - Edge (type, inspection_status, criterion, evidence, priority_score)
  - Both intent nodes (name, description, abstraction_level, source_refs)
  - Related CodeFiles (paths, last_modified)
  - VALIDATES edges on those intents (validation name, last_result)
  - Suggested action
  No second lookup needed. LLM can act immediately.

loom intent add --name --description --level [--domain] [--source ...]
loom intent add ... [--aspect happy|sad|fallback|…] [--lifecycle planned|implemented|needs_change] [--tag <term> ...] [--visibility user_visible|internal]
loom intent confirm <id> [--visibility user_visible|internal]
  Ratify the meaning (status → confirmed) AND stamp a freshness note (kind=
  confirm, append-only — alignment history travels in the export). Re-confirming
  is the align loop's cheap outcome: it resets the drift-suspicion clock
  `loom next --mode align` ranks by. `--visibility internal` records the
  audience ruling atomically with the confirm — the "this is machinery, stop
  asking the user about it" interview outcome (out of the align queue until
  the meaning is redefined). Validator lane.
loom intent update <id> [--name "<new>"] [--description "<new>"] [--reword] --reason "<why>"
  EVOLUTION in place — same node, same id, full history — distinct from retire
  (supersession by a different intent). A --description change is a REDEFINITION
  and ripples ONE HOP, the semantic twin of `loom sync`: passing/independent
  RELATES_TO + GOVERNS → needs_reverification, passing IMPLEMENTS →
  needs_reverification ("does the code still do what this NOW says?"), passing
  TARGETS → needs_reverification, linked proofs → not_run (blocked keeps its
  reason). Every flip is noted with cause "intent '<name>' redefined"; the old
  wording is preserved in a decision note. --name alone is cosmetic (no ripple).
  A redefinition also CLEARS the visibility ruling (the new meaning's audience
  is unknown — the align interview re-triages it). --reword (requires
  --description) = same concept, clearer words: no ripple, visibility kept,
  but the align clock still resets ("terminology confusing, keep concept").
  Lifecycle is NOT auto-flipped — the staled IMPLEMENTS routes the honest
  question through the fix queue instead of faking a needs_change verdict.
  Builder lane.
loom intent mark <id> --lifecycle planned|implemented|needs_change [--reason "<why>"]
  Set the prescriptive lifecycle. needs_change = a known issue/refactor (honest,
  no faked verdict); --reason is recorded as a note. Feeds `loom next --mode build`.
loom intent delete <id>          (remove a mistake: node + its edges + notes)
loom intent retire <id> --reason "<why>" [--replaced-by <intent>]
  Design that was REAL and got superseded (delete is for mistakes). Status →
  deprecated; node/edges/notes stay as history, but the intent becomes
  INVISIBLE TO COMPUTATION: queues, coverage axes, centrality, the N×N grid,
  completeness, and sync ripple stop counting it. Reports the TRIGGERED WORK:
  orphaned children (re-parent or retire), files that lost their only owner
  (they surface as vertical gaps), proofs left dangling. The successor is
  recorded in a decision note — lineage stays traceable.
loom intent source add <id> <path>     (append to source_refs — docs AND code:
                                        contracts, ADRs, design notes; idempotent)
loom intent source remove <id> <path>
loom intent tag add <id> <term>        (tag from the registered vocabulary, max 3;
                                        an unknown term errors with the registry
                                        inlined — the menu at the decision point)
loom intent tag remove <id> <term>
loom intent list [--status] [--level] [--limit N]
loom intent show <id>            (intent + edges + hierarchy + implements + notes)

loom edge explore <a-id> <b-id>
  Prints both intents + source_refs. Creates edge if not exists.
  Subcommands:
    ground --criterion --confidence [--evidence "<found>"]
           [--evidence-locator path:lines]... [--inspected-by]
      evidence is optional on ground (the criterion may say it all) and ALWAYS
      replaces the previous verdict's evidence (a re-ground never leaves stale
      failure evidence behind the new green).
    issue --criterion --evidence [--evidence-locator path:lines]... [--inspected-by]
    --evidence-locator (repeatable) = file/line anchor(s), e.g.
      `src/db/queries/stats.rs:299-340`, folded into the stored evidence as
      `@<locator>` — a later review lands on the exact lines, not prose.
    independent --notes
    fix --description

loom edge list [--status] [--limit N]
loom edge show <edge-id>

loom cluster <intent-id>
  All unresolved edges touching this intent. For batching neighborhood work.

loom codefile add <path>          (or a glob: 'src/**/*.rs')
loom codefile list [--limit N]
loom codefile show <path-or-id>
  The per-file OWNERSHIP view: which intents claim it (level + locator +
  status), which quality rules reach it through them, its imports, and a
  tangled flag (≥3 intents). The answer hotspots only hint at.
loom codefile remove <path-or-id> (drop a phantom after delete/rename on disk;
                                   removes its IMPLEMENTS edges too)

loom validation add --name --type [--command] [--description] [--intent <id>]...
  --intent (repeatable) links the new Validation to intent(s) (one VALIDATES
  edge each) in one step; omit to link later with `loom edge validates`.
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
loom validation list [--intent <id>] [--limit N]

loom validate <intent-id> | --all
  Runs command on all VALIDATES edges for this intent. (manual_check without a
  command is skipped — use `loom validation mark` for those.)
  Updates Validation.last_result and VALIDATES edge inspection_status.
  --all = every PENDING proof in the graph (last_result == not_run: never run
  or sync-invalidated) in one verb — the drain after a sync flood resets N
  proofs at once. Settled verdicts (passed/failed) are not re-run; blocked
  proofs keep their recorded reason and stay out.

loom saga add <spec.yaml>
loom saga run <name|spec.yaml>
loom saga list
  THE CONSUMER PLANE: an external-consumer proof — an ordered chain of endpoint
  invocations that consumes the system the way a real consumer will (values
  captured from one response thread into the next request). Runtime complement
  to read-evidence: RELATES_TO edges are normally grounded by READING code; a
  saga stamps the edges along its intent path with EXECUTION evidence.
  Engine is built in and pure Rust (reqwest/rustls + RFC 9535 JSONPath — no
  libcurl); deliberately a saga executor, NOT a general HTTP test tool
  (anything fancier = an ordinary command-based Validation).
  Saga specs are trusted repo artifacts: `run` executes the declared HTTP
  calls, allows any `http(s)` target (including localhost), and follows
  reqwest's default redirect policy (up to 10 redirects). Guardrails are size
  ceilings, not sandboxing: response bodies are capped at 8 MiB and spec files
  are capped at 512 KiB before YAML parsing.
  Spec (YAML, the graph binding is first-class — every step names the intent
  it proves):
    saga: checkout-flow
    base: "{{ env.BASE_URL }}"        # {{ var }} / {{ env.X }} interpolation
    steps:
      - name: create cart
        intent: cart-creation          # id, exact name, or unique fragment
        request: { method: POST, url: /carts, json: { items: [] } }
        expect:  { status: 201, body: { "$.id": { exists: true } } }
        capture: { cart_id: "$.id" }   # JSONPath → var for later steps
      - name: capture payment
        intent: payment-capture
        request: { method: POST, url: "/carts/{{ cart_id }}/payment" }
        expect:  { status: 200, body: { "$.state": paid } }
  expect.body values: bare value = equals · {exists: bool} · {contains: "…"};
  expect.status omitted = any 2xx; expect.headers = substring match.
  `add` declares the proof: Validation node (type=saga, command =
  `loom saga run <spec>`) + VALIDATES edges to every step intent + the
  RELATES_TO path edges between consecutive step intents (uninspected — green
  is earned by running) + the spec registered as a CodeFile (it travels in the
  export, counts in coverage). Idempotent; re-add after editing reconciles.
  `run` executes (DB closed while HTTP runs, same lock discipline as
  `loom validate`) and translates outcomes into graph verdicts — the failure
  semantics: consecutive steps that BOTH passed → their RELATES_TO edge goes
  passing with runtime evidence; the boundary into the failing step → failing
  with the exact broken expectation ("expected 200, got 502"); steps after the
  failure are UNTOUCHED (never reached ≠ failing); the Validation + all its
  VALIDATES edges carry the run verdict. Existing non-empty edge criteria are
  preserved (execution refines the analyzer's contract, never overwrites it).
  Exits non-zero on failure, so the stored command also works under
  `loom validate` and in CI. Validator lane (`add` is builder|validator).
  Sync ripple already covers re-validation: code behind a step intent changes
  → its VALIDATES edges → not_run → the saga resurfaces in the validate queue.
  ENV VALUES: `{{ env.X }}` = passed AT INVOCATION (`BASE_URL=… loom saga run
  <name>`), never stored in the graph — they point at a LIVE target (start the
  system under test first). `saga add`/`list` report what's required (`run
  with: BASE_URL=<value> …`); a missing value REFUSES to run with the exact
  invocation in the error and nothing stamped — and `loom validate` records it
  as `blocked` (environment-not-ready), never as a failed proof.

loom rule add --name --description --severity [--effort low|mid|high]
  --effort = how much capability INSPECTING this rule needs (pack rules ship
  annotated: secrets-scan low, atomicity high, default mid). Travels into
  quality work items as `effort`.
loom rule list [--limit N]
loom rule apply <rule-id> <intent-id>   (positional; creates GOVERNS edge, uninspected)
loom rule check <intent-id>             (read-only: show GOVERNS edges by status)
loom rule verdict <rule-id> <intent-id> --status passing|failing|independent \
    --criterion "<what compliance looks like>" --evidence "<what was found>" \
    [--evidence-locator path:lines]... [--confidence 0.9] [--inspected-by llm:quality]
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
  invocations (pair with the bulk read: `loom next --mode fix --take 20`).
  Ops per line: ground / issue / independent (RELATES_TO) and
  rule_verdict (GOVERNS, creates the edge if absent). ground also takes an
  optional "evidence"; ground/issue/rule_verdict take an optional
  "evidence_locator" (string or array of `path:lines` anchors). EVERY gate applies per
  line — lanes, substantive criterion/evidence/notes, confidence — and each
  edge still gets its transition note. Continues past failed lines, reports
  per-line results, exits non-zero if any failed. Bulk changes the ceremony,
  never the honesty.

loom hypothesis add --name <n> --claim <c> --proposal <p> --predicted-outcome <o> \
    [--target <intent>]... [--author <agent>]
  Propose an improvement (status=proposed) — THE PRE-DECISION PLANE, the
  structured upgrade of `note --kind idea`. Any lane proposes; evidence gates
  reject vacuous claim/proposal/outcome. --target creates TARGETS edges.
loom hypothesis target <hypothesis> <intent>   (link another affected intent)
loom hypothesis prove <id> --verdict supported|refuted --evidence "<found>" \
    [--inspected-by llm:analyzer]
  The proof step: did the claimed problem turn out to be real in the code as it
  is NOW? Analyzer lane; the prover must differ from the proposer (when both
  declare roles — solo mode passes, as everywhere). The verdict also stamps
  every TARGETS edge (supported→passing, refuted→independent) — which is also
  how stale support clears after a re-prove. Decided (adopted/confirmed/
  rejected) hypotheses cannot be re-proven.
loom hypothesis adopt <id> [--spawned <intent>]... [--reason "<how it converts>"]
  THE CONVERSION POINT (builder lane, owned custody, requires status=supported):
  link the planned intents spawned from it — lineage decision notes both ways,
  and predicted_outcome becomes a not_run manual_check Validation (its
  description carries a `hypothesis:<id>` line) VALIDATES-linked to each
  spawned intent: the acceptance contract enters the proof plane. Requires
  --spawned or --reason; from here `loom next --mode build` owns the work.
  When `loom validation mark <outcome-validation> --result passed` lands, the
  hypothesis derives `confirmed` — the improvement provably delivered.
loom hypothesis reject <id> --reason "<why>"   (any state except adopted/confirmed)
loom hypothesis list [--status proposed|supported|refuted|adopted|confirmed|rejected] [--limit N]
loom hypothesis show <id>                      (fields + TARGETS + notes)

loom note add --text <text> [--kind <kind>] [--intent <id> | --edge <id> | --file <path|id>] [--author human|llm] [--for <role>]
  Append free-text memory. kind: justification | commentary | idea | question | decision | todo
  (transition + confirm are auto-recorded by loom: verdict history and
  `loom intent confirm` freshness stamps — listable, never written by hand).
  Attach to an intent, an edge, or a code file (id or registered path), or leave
  free-floating. Append-only (never overwritten). A kind=decision note is the
  adjudication record smells honor (scatter/tangle/happy-path/recurrence).
  --for builder|analyzer|fixer|validator|quality ADDRESSES the note to a lane —
  the directed-handoff channel: an out-of-lane finding becomes a message the
  owning lane sees FIRST (`loom next` sorts addressed notes to the top of the
  item's notes). Notes surface in `loom next`, `loom intent show`, `loom edge show`.
loom note prune
  Remove notes whose target no longer exists (deleted intent/hypothesis/edge)
  — the remedy `loom doctor` names for dangling note targets. Only
  unreachable notes are removed; history on live or retired nodes is never
  touched. (The hard-delete commands now prune their edges' notes themselves;
  this cleans up damage from older versions.)
loom note list [--intent <id>] [--edge <id>] [--file <path|id>] [--kind <kind>] [--for <role>] [--limit N]
  --for <role> = the lane's inbox (only notes addressed to it). --limit keeps
  the NEWEST rows (append-only memory; the tail is the live context).

loom vocab add <term> --why "<contrastive definition>"
  Register a tag term (builder lane). The --why must be CONTRASTIVE: what it
  covers AND what it does not, naming the neighbouring term ("authz —
  permission checks, NOT login/session (that's authn)"). A term that reads
  like an existing one (same stem / containment / tiny edit distance) is
  REJECTED at the door — synonym terms split the keyspace and intents stop
  colliding. Keep the registry small (warn past ~75): its value is that an
  agent can hold the whole menu in context at the moment of choice.
loom vocab list
  The registry: every term with usage count + definition — the menu agents
  pick from when tagging.
loom vocab merge <from> <to>
  Converge drift: every intent carrying <from> is retagged to <to> (deduped),
  <from> is deleted. One sweep, nothing to re-inspect — terms are keys, not
  inspectable claims. The `vocab_drift` smell emits this command.

loom doctor
  Verify graph integrity against the declared schema (src/db/schema.rs):
  schema version, required-property presence, valid field values, dangling
  references, and the evidence audit behind every verdict — vacuous criterion,
  confidence outside [0,1], confidence still 0.0 behind passing/failing, empty
  last_inspected behind a verdict, out-of-lane provenance. Also emits advisory
  HINTS (never fail the exit code): all-solo provenance (declare roles for real
  separation of duties), and a stale committed loom.graph.json.
  Exits non-zero if any issue is found. Run after upgrades or if results look wrong.
  A version mismatch points at `loom migrate`.

loom migrate
  Upgrade a LIVE graph to the current schema version IN PLACE — a version
  CHAIN, each step idempotent, the meta version stamped LAST (crash-safe by
  re-run, not by transaction: bulk read-modify loops inside one transaction
  go quadratic on grafeo 0.5.x — see commands/migrate.rs).
  v3 → v4: edge identity became DERIVED (`<prefix>:<from>:<to>`, e.g.
  `rt:<intent-a>:<intent-b>`) instead of a stored uuid — every note that
  referenced a stored edge uuid is remapped (legacy id props on old edges are
  inert and left alone). v3/v4 → v5: source_refs/tags/imports convert from
  JSON-encoded strings to NATIVE LISTS. Also backfills the property indexes.
  Idempotent: a current graph reports "nothing to do". Re-export after
  migrating. Repos with only a committed loom.graph.json don't need this —
  `loom import` upgrades v3/v4 exports in flight.

loom guide [--mode greenfield|brownfield|refactor|port|seed]
  Self-contained driving protocol for an LLM new to loom: mental model, the loop,
  the done-condition, and a MODE-SPECIFIC population checklist (auto-detected via
  `loom detect` if --mode omitted): greenfield = design-as-planned-intents then
  build; brownfield = map & verify existing; refactor = flag needs_change & change;
  port = adopt a source graph's design (`import --as-planned`) and re-realize
  it in a new language/repo; seed = the USER interview (explicit-only, never
  auto-detected — the binary can't detect "the user wants to talk"): elicit a
  head into planned intents (altitude-calibrated, one question per landing,
  terminate on enumerable gaps, not exhaustion) or re-align a populated graph
  via `loom next --mode align`. An empty graph's compass routes phase=seed here.

loom schema
  The data model — node/edge types + properties, the inspection state machine,
  and the valid value vocabularies. Generated from the schema vocabulary (drift-proof).

loom find <query> [--limit N]
  ASK THE MAP — codebase intelligence entry point: BM25 keyword search over
  active intent names + descriptions (+domain), ranked. Each hit carries its
  hierarchy chain, IMPLEMENTS groundings with locators, and a stale-edge count
  (the freshness warning: claims about since-changed code). Scoring runs in
  Rust, NOT grafeo's text index — `CALL grafeo.search.text` returns internal
  node ids that can't be joined back to properties through GQL (probed; the
  trailing MATCH parses and is silently dropped). Deterministic; no fuzzy/
  stemming by design — the calling LLM reformulates. A miss distinguishes
  "not mapped" (points at `loom coverage`) from "doesn't exist".

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
  names the children; a kind=decision note on the intent NEWER than its newest
  grounding records "the spread is deliberate" and resolves the finding, a
  later grounding re-flags it), tangled files (≥3 intents — per-file detail via
  `loom codefile show`; a kind=decision note on the FILE — `loom note add
  --file <path> --kind decision` — newer than its newest claim resolves it,
  a later claim re-flags), undeclared coupling (file A imports file B but
  their intents have no edge — physical evidence vs semantic graph), recurrent
  trouble (a target whose transition history keeps returning to failing/
  needs_change — redesign, don't re-patch; a kind=decision note NEWER than the
  last regression resolves the finding without erasing history, and a later
  regression re-flags it), unmeasured intents (a QualityRule
  was never held against a coded intent — HIERARCHY-AWARE: a verdict on a
  component covers its descendants, so measure at the highest honest altitude
  instead of grinding per-leaf busywork; a leaf can still get its own, more
  specific verdict), unused rules, happy-path-only groups (children declare an
  `--aspect happy` but no sad/fallback sibling — failure behavior undeclared;
  a kind=decision note on the parent newer than its newest aspect-tagged child
  records "N/A here" and resolves it, a new aspect child re-flags),
  duplicated responsibility (two same-level intents whose REGISTERED tags
  collide rarity-weighted, grounded in DISJOINT files with no import between
  them — the case every physical detector misses: same responsibility
  implemented twice in unrelated code; untagged intents never fire it), and
  vocab drift (two registered terms that read like the same word — remedy is
  the exact `loom vocab merge`).
  OPEN FINDINGS GATE GREEN: once every queue is dry, `graph_state` routes
  phase=audit until `loom smells` returns zero OPEN — green means every
  suspicion was ANSWERED (structurally fixed, or refuted via its adjudication
  path above), never that the heuristics went quiet on their own.
  ADJUDICATIONS STAY VISIBLE: a finding suppressed by a decision note is not
  gone — `loom smells` prints it under `adjudicated` with the ruling (who,
  when, why) and the exact structural change that re-opens it. "No findings"
  and "N findings ruled deliberate" never look alike (dogfood lesson: five
  godfile rulings batch-stamped in one second were invisible in every output).
  Disagreeing with a ruling is overruled through the work, not the ledger:
  `loom hypothesis add … --target <intent>` routes the redesign through triage.
  Each finding carries the exact remedy command — and the redesign-shaped ones
  (recurrent trouble, tangled files, twin merges, code-level scatter) emit
  `loom hypothesis add` so a redesign gets PROVEN before it becomes work,
  instead of dying in a note. The same suspicion signals
  (import links, shared files, description overlap, shared tags
  rarity-weighted, same domain) rank
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
  run `loom sync` after to reconcile with the machine's files. TWO-PHASE: every
  node and edge is validated before anything is written — a corrupted/hostile
  export is rejected loudly (field-naming error) and leaves NO partial graph.
loom import <file> --as-planned
  PORTING: the semantic plane travels, the physical plane is rebuilt. Intents/
  hierarchy/criteria/rules/notes are adopted; CodeFiles, IMPLEMENTS groundings,
  verdict meta, and proof results are dropped (they were earned against the OLD
  code). Every intent arrives lifecycle=planned, every proof not_run with its
  command kept as the spec to re-express; the target keeps its own graph
  identity. `loom guide --mode port` teaches the re-realization loop; the
  criteria written for the old code are the acceptance contract for the new.

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
 Edge ids are DERIVED from the endpoints — `rt:<from>:<to>` (hy/imp/gov/val/tgt
 for the other types), never stored — stable across export/import; `loom edge
 show` takes them and notes reference them.
 `loom edge implement <intent> 'src/db/**'` bulk-grounds over REGISTERED paths;
 `loom edge unimplement <intent> <path|glob>` is the ungrounding half — used to
 move groundings down to children when decomposing a scattered intent.)
```

All commands support `--json` for machine-readable output. LLM driving mode uses `--json` everywhere.

GRAPH TARGETING: every command resolves its graph via `--graph <path>` >
`$LOOM_GRAPH` > cwd (one shared `resolve_root()` in db/mod.rs). Pin a session
with `export LOOM_GRAPH=<repo>` and every loom call hits that graph no matter
what `cd` does — kills the cd-fallback incident class (a failed cd + a
mutating script silently hitting whatever graph cwd landed in). Interactive
driving keeps the zero-ceremony cwd default; an unpinned command in a bare
directory still errors rather than guessing.

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
  → surfaced as `horizontally_explored`; phase=complete additionally requires
    the AUDIT gate: zero open `loom smells` findings (phase=audit until every
    suspicion is resolved or refuted via its remedy).
```
Compass routing (queries/stats.rs `graph_state`) prioritises vertical gaps
(`ground`/`incomplete` phases) ahead of optional horizontal `discovery`, then
gates `phase=complete` behind `audit` (open findings). The old "all edges passing/independent"
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
├── saga/                 the consumer plane's engine (pure Rust: reqwest/rustls)
│   ├── mod.rs            module wiring + design rationale
│   ├── spec.rs           YAML spec format, load-time validation, {{ }} interpolation
│   └── runner.rs         sequential executor: captures, asserts, halt-on-failure outcomes
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
│       ├── hypothesis.rs   Hypothesis node queries (the pre-decision plane)
│       ├── note.rs         Note annotation queries (append-only memory)
│       ├── ignore.rs       Ignore patterns (coverage escape hatch, with reasons)
│       ├── vocab.rs        VocabTerm registry + intent tags (bounded vocabulary:
│       │                   normalize/merge/rarity-weighted collision + look-alike)
│       ├── meta.rs         LoomMeta sentinel: version + last_synced freshness
│       ├── completeness.rs vertical-completeness spine (tree + realization join)
│       ├── relates_to.rs   RELATES_TO edge (the intent grid)
│       ├── hierarchy.rs    HIERARCHY edge (structural tree, enforced at insert)
│       ├── implements.rs   IMPLEMENTS edge (carries `locator`)
│       ├── governs.rs      GOVERNS edge
│       ├── targets.rs      TARGETS edge (hypothesis → affected intents)
│       ├── validates.rs    VALIDATES edge
│       ├── scoring.rs      priority scoring + discovery candidates (loom next)
│       ├── find.rs         BM25 keyword search over intents (loom find)
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
    ├── hypothesis.rs     loom hypothesis * (propose/prove/adopt/reject)
    ├── vocab.rs          loom vocab * (registry + the in-band tag nudge `validate_tags`)
    ├── note.rs           loom note *
    ├── rule.rs           loom rule *
    ├── saga.rs           loom saga * (consumer-plane proofs: declare, run, stamp the path)
    ├── report.rs         loom report (+ completeness gaps)
    ├── doctor.rs         loom doctor
    ├── guide.rs          loom guide
    ├── schema.rs         loom schema
    ├── find.rs           loom find (ask the map)
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

**Why edge identity is DERIVED, not stored (schema v4):** edges are unique per endpoint pair, so an edge's id is computed at read time — `schema::edge_key(type, from, to)` → `rt:<a>:<b>` / `imp:` / `gov:` / `val:` / `tgt:` / `hy:` — never written to the store. The uuid it replaced was redundant identity that (a) sat on the one property name grafeo can't filter by, (b) regenerated on every import so note targets broke in transit, and (c) forced scan-to-find-by-id reads. Derived keys are stable across machines and exports by construction. `loom migrate` upgrades live v3 graphs (remaps note targets); `loom import` upgrades v3 exports in flight.

**Why edges are matched by endpoints, never by their own id:** in grafeo 0.5.42, the property NAME `id` is shadowed on relationships in *filter* position: `WHERE r.id = X` and inline `{id: X}` resolve `id` to grafeo's INTERNAL edge id (an integer) instead of the user property — they match nothing, ever (deterministically; in *RETURN* position `r.id` correctly yields the stored property). Every OTHER edge-property filter (`WHERE r.inspection_status = X`, `<>`, inline forms) is deterministic — verified under stress in `tests/grafeo_probe.rs` (50/50 set-then-filter cycles, in-memory and persistent), which also pins MERGE+RETURN, transactions, `$param` binding, and index idempotency; run it after any grafeo upgrade. So: edge-ID lookups take endpoint ids and match `MATCH (a {id})-[r]->(b {id}) SET ...` (RELATES_TO/IMPLEMENTS/GOVERNS/VALIDATES are unique per endpoint pair) or scan-and-filter in Rust; status filters live in the query (with a zero-cost Rust retain as regression guard); get-or-create is one `MERGE … ON CREATE SET … RETURN`. The long-lived single `Session` in `db/mod.rs` stays (read-your-writes within a session). Multi-statement mutations (import, the sync ripple, retire, one batch line) run inside `with_transaction` (START TRANSACTION/COMMIT/ROLLBACK) — a failure midway rolls back instead of leaving a half-flipped graph. `loom init` creates property indexes on every inline-matched key (idempotent; re-running init backfills older graphs). CALL procedures run but a trailing MATCH after `CALL … YIELD` is silently dropped — joining algorithm output back to properties requires a Rust-side join via `id(n)` (works; unused so far). Concurrent ReadOnly access alongside a writer is refused by the file lock — don't design for parallel readers on 0.5.42.
