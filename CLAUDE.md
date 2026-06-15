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

## Node types (10)

### Intent
The core node. Everything orbits this.
```
id                  STRING (uuid)
name                STRING
description         STRING
abstraction_level   STRING  -- "feature" | "component" | "system" | "cross_cutting"
domain              STRING  -- product/business facet label only (auth, billing);
                               a free grouping key. NO layering effect — v6 split
                               architecture direction out into `layer` below.
layer               STRING  -- architecture-direction grouping (presentation,
                               application, storage). `loom layer order <top> …
                               <bottom>` ranks layers; imports pointing UP that
                               order surface as layering_violation. The smell
                               reads `layer`, not `domain`.
source_refs         LIST    -- file paths (native list since schema v5)
status              STRING  -- "proposed" | "confirmed" | "deprecated"
lifecycle           STRING  -- "planned" | "implemented" | "needs_change" — the
                               axis the build/fix queues route on (default
                               implemented)
aspect              STRING  -- path-coverage facet ("happy" | "sad" | "fallback"
                               | …); a parent with a happy child but no sad/
                               fallback sibling trips the happy_path_only smell
visibility          STRING  -- "user_visible" | "internal" | (unset = untriaged;
                               the align interview triages it)
boundary            STRING  -- "inbound" | "outbound" | (unset = internal) — marks
                               a system-boundary crossing: inbound = a provider
                               contract loom owns; outbound = a consumer dependency
                               loom relies on. The boundary facet + coupling smells
                               read it; no silent inference (the builder rules it).
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
symbols         LIST    -- tree-sitter top-level symbol names (schema v7; additive
                           diagnostic). Feeds symbol-accountability coverage;
                           populated by `loom sync`, [] when unavailable.
symbol_facts    LIST    -- richer per-symbol metadata as JSON objects (schema v8;
                           additive). Powers symbol-level sync narrowing — a file
                           edit ripples only the intents owning the changed symbols.
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

### Persona
A named audience segment — the "as a [X]" of user stories (the consumer plane).
SERVES edges verify which intents actually serve it; JOURNEYS edges bind saga
proofs to its end-to-end path. Personas turn "who is this for?" into an
inspectable claim and connect runtime proofs to the audience they protect.
```
id           STRING (uuid)
name         STRING
description  STRING
author       STRING
created_at   STRING
updated_at   STRING
```

### Note · Ignore · Delegation (infrastructure nodes)
Not planes — bookkeeping the CLI hangs off the graph.
- **Note** — append-only free-text memory (justification | commentary | idea |
  question | decision | todo, plus auto `transition`/`confirm`); targets an
  intent, edge, or codefile. `--for <role>` makes it a lane handoff.
- **Ignore** — a coverage-exclusion glob with a recorded reason (the honest
  escape hatch for generated/vendor/out-of-scope files).
- **Delegation** — a subtree owned by another loom graph (monorepo/federation):
  `loom coverage` treats matching files as covered by the child's committed export.

## Edge types (8)

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

### SERVES (Persona → Intent)
An inspectable claim that this intent actually serves that audience segment.
Carries the full inspectable-edge meta (criterion/confidence/evidence/…),
defaults `uninspected` — "does it serve them?" is earned, not assumed.

### JOURNEYS (Persona → Validation)
A structural binding from an audience segment to a saga proof exercising its
end-to-end path. No inspection_status (like HIERARCHY, it's structural) — it
connects the persona to runtime evidence its journey works.

## State machine (on the INSPECTABLE edges)

`inspection_status` is the heartbeat — on the edges that represent a *claim
verified against code*: RELATES_TO (analyzer), IMPLEMENTS (analyzer), GOVERNS
(quality), VALIDATES (validator), TARGETS (analyzer), SERVES (analyzer). As of
schema **v3**, HIERARCHY does NOT carry one: it's a structural tree edge,
enforced at insert (unique parent, no cycles), never "inspected" — and neither
does JOURNEYS (Persona → Validation, structural).

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
  (hypothesis support was earned against the old target code — the prove
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

Every command supports `--json` (LLM driving mode uses it everywhere). The **full
per-command reference lives in [`docs/COMMANDS.md`](docs/COMMANDS.md)** — extracted from
this handoff doc to stay under the context budget, with the complete semantics and
design rationale for each command. The CLI is also self-documenting at runtime:
`loom guide` (driving protocol), `loom schema` (data model), `loom <command> --help`
(EXAMPLE block + flags). Compact index of the command surface:

```
Setup & sync    init · sync · migrate · import [--as-planned] · export [--check] · detect   (`serve` = retired stub)
Orientation     status · session · door · find · guide · schema · report · coverage · hotspots · smells · doctor
Work queues     next [--mode discovery|fix|build|validate|quality|review|prove|align] [--all] [--take N] [--compact] · cluster · batch
Intents         intent add|confirm|update|mark|delete|retire|source|tag|list|show
Edges           edge explore {ground|issue|independent|fix} · edge implement|unimplement · edge list|show
Code            codefile add|list|show|remove
Quality         rule add|list|apply|check|verdict · rule seed <iso5055|mobile|web-ui|service|data|concurrency>
Validation      validation add|mark|update|delete|list · validate <id>|--all · saga add|run|list
Pre-decision    hypothesis add|target|prove|adopt|reject|list|show
Personas        persona add|list|show · persona serve {ground|issue|independent} · persona journey
Vocab & layers  vocab add|list|suggest|merge · layer order|list|clear   (`domain` = deprecated alias of `layer`)
Memory & hatch  note add|prune|list · ignore add|list · delegate add|list
```

The active runtime store is embedded SQLite at `.loom/graph.sqlite`.
`loom.graph.json` is the portable committed export. `loom serve` is a retired
stub kept only to give old scripts a clear error; normal CLI dispatch opens
SQLite directly and does not auto-spawn a daemon.

Edge ids are DERIVED from endpoints — `rt:<a>:<b>` (`hy`/`imp`/`gov`/`val`/`tgt`/`srv`/`jrn`
prefixes for the other types), never stored, stable across export/import. Intents and rules are
addressable by id, exact name, or unique name fragment.

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
  - Symbol accountability has no open actionable gaps once it reaches the
    audit gate; raw helper/test symbol coverage is diagnostic, not the target.
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
2. "Graph persistence via SQLite" (component) — src/db/mod.rs, src/db/sqlite.rs
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
│   ├── mod.rs            graph root resolution + read-handle abstraction
│   ├── sqlite.rs         SQLite schema, import/export bridge, runtime reads/writes
│   ├── schema.rs         THE schema vocabulary: labels/edges/props + per-field owner role,
│   │                     version, required-property tables
│   └── queries/          pure snapshot analysis: search, queues, integrity, stats, smells
│       ├── mod.rs          module wiring + flat re-export
│       ├── vocab.rs        VocabTerm registry + intent tags (bounded vocabulary:
│       │                   normalize/merge/rarity-weighted collision + look-alike)
│       ├── meta.rs         GraphMeta and transition defaults
│       ├── completeness.rs vertical-completeness spine (tree + realization join)
│       ├── relates_to.rs   unresolved RELATES_TO analysis
│       ├── scoring.rs      priority scoring + per-mode candidate selection (loom next)
│       ├── snapshot.rs     QuerySnapshot — one graph load feeding scoring/stats/compass
│       │                   coherently (the production read path; no per-query reloads)
│       ├── find.rs         BM25 keyword search over intents (loom find)
│       ├── smells.rs       derived problem signals (split-brain, scatter, tangle,
│       │                   layering, vocab drift, …) with per-finding remedy; plus
│       │                   the non-gating ADVISORIES — cochange_coupling (git) and
│       │                   nonlocal_proof (a `proven` leaf whose only test lives in
│       │                   another module — the proof-locality check) (loom smells)
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
    ├── batch.rs          loom batch (bulk JSONL verdicts — the post-sync drain)
    ├── persona.rs        loom persona * (audience segments + SERVES/JOURNEYS edges)
    ├── layer.rs          loom layer order|list|clear (architecture dependency direction)
    ├── domain.rs         loom domain (DEPRECATED alias of `loom layer`)
    ├── report.rs         loom report (+ completeness gaps)
    ├── doctor.rs         loom doctor
    ├── guide.rs          loom guide
    ├── schema.rs         loom schema
    ├── find.rs           loom find (ask the map)
    ├── door.rs           loom door (the entrance: utterance → matches + landing menu)
    ├── session.rs        loom session (turn zero: ask-the-user playbook + offer menu)
    ├── hotspots.rs       loom hotspots
    ├── coverage.rs       loom coverage
    ├── smells.rs         loom smells (derived suspicions; OPEN findings gate green)
    ├── detect.rs         loom detect
    ├── ignore.rs         loom ignore *
    ├── delegate.rs       loom delegate (federation: hand a subtree to a child graph)
    ├── migrate.rs        loom migrate (SQLite schema verification)
    ├── export.rs         loom export [--check] (commit the graph as deterministic JSON)
    └── import.rs         loom import (rebuild a graph from an export)
```

## Build

```bash
cd /Users/laptopdp/Developer/damarpanuluh/loom
cargo build
```

The runtime graph store is SQLite via `rusqlite`. The default tree-sitter
grammar crates compile C sources for richer imports, so the default build still
needs a working C compiler. Release builds currently use thin LTO for faster
iteration. Use `cargo build --no-default-features` for the dependency-light
heuristic import path.

Run `cargo test` for the query-layer regression suite (in `db/queries/mod.rs`),
which covers the relationship reliability rule below. Also run
`cargo test --no-default-features` before release changes that touch sync or
import extraction so the fallback path stays green.

**Zero-warning, rustfmt-clean is the bar.** Formatting is a mechanical invariant
like the build-failing ratchet tests — the repo uses stock stable rustfmt
(`rustfmt.toml`, edition pinned to Cargo.toml). Before every commit:

```bash
cargo fmt                          # or `cargo fmt --check` to verify
cargo clippy --all-targets         # must be warning-free
cargo build                        # must be warning-free (default + --no-default-features)
cargo test                         # green
```

Test-only items (constants/imports used solely by `#[cfg(test)]` code) carry
`#[cfg(test)]` so the non-test profile stays warning-free — don't `#[allow]`
them. (No CI is wired yet; these are the pre-commit gate by hand. A
`cargo fmt --check` + `clippy -D warnings` CI step is the natural enforcement
when CI lands.)

## Key design decisions (why, not just what)

**Why SQLite now:** Loom's production operations are bounded graph reads/writes
with explicit queues, lifecycle state, and exportable JSON identity. SQLite gives
us durable embedded storage, transactions, normal tooling, easy migrations, and
fewer runtime locking surprises. Recursive CTEs and targeted adjacency indexes
cover the graph traversals loom actually executes today; more advanced graph
analysis can still be layered later as derived tables, virtual tables, or a
specialized analysis engine without replacing the core SQLite store.

**Why `independent` is a state not an edge type:** Independence is a verified claim about a relationship, not a different kind of relationship. Encoding it as state keeps the schema clean and queries uniform.

**Why Validation is a node not a property:** Validations are reusable, runnable, and have their own lifecycle (last_run, last_result). Nodes are the right abstraction for entities with identity and state.

**Why separate state from meta:** State drives the workflow engine. Meta provides evidence. Keeping them conceptually distinct prevents the agent from confusing "what do we know" with "how do we know it."

**Why loom sync propagates one hop:** Full graph propagation from a file change would be too aggressive — everything would reset. One hop (IMPLEMENTS → RELATES_TO neighbors) is the right blast radius for a file-level change. System-level changes require explicit re-initialization.

**Why edge identity is DERIVED, not stored (schema v4):** edges are unique per endpoint pair, so an edge's id is computed at read time — `schema::edge_key(type, from, to)` → `rt:<a>:<b>` / `imp:` / `gov:` / `val:` / `tgt:` / `hy:` — never written to the store. The uuid it replaced was redundant identity that regenerated on every import so note targets broke in transit and forced scan-to-find-by-id reads. Derived keys are stable across machines and exports by construction. SQLite stores endpoint columns directly and treats the derived key as API identity.
