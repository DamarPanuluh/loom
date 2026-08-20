# loom v2 — Build Plan

**Status:** archive. Historical record of MVP ring sequencing. Rings 1–7 **shipped** and the sequencing continued past this plan: rings 8–12 (findings triage, review queue + work packets, sensors + Definition-of-Complete, grounding roles, apply-batch + federation — see the addendum at the end and `CHANGELOG.md`) landed after ring 7, and most of "Milestone 2" shipped with them. The ring definitions below are kept as the record of what each ring was required to prove; `commands.md` and the compiled `--help` describe the current surface.

---

## Principle

**Each ring must be green before the next begins.**

"Green" means:
- compiles without warnings (rustfmt-clean, clippy-clean)
- ring-specific invariant tests pass
- dogfood command works against a scratch graph
- `loom export --check` passes (export deterministic)

No ring is "done" with a passing skeleton. Done means the feature is correct end to end.

---

## Reference crates (locked in `design.md §10`)

```text
clap            CLI surface and dispatch
rusqlite        SQLite graph store (bundled — no system SQLite dependency)
tree-sitter     code extraction (+ grammars: rust, python, go, typescript, javascript)
reqwest         HTTP for journey runner (rustls, blocking)
serde / serde_json  serialization
std::fs::File locking  cross-process file lock for concurrent access (was fs2; std stabilized locking in Rust 1.89)
```

v1 (`../../loom`) is a **read-only reference oracle**. Read it for:

| v1 mechanism | Reference value |
|---|---|
| `repo.rs`, `ts_imports.rs` | Tree-sitter extraction quirks per language |
| `db/sqlite*` | WAL mode, busy timeout, locking patterns |
| `commands/sync.rs` | Staleness edge cases, ripple ordering |
| `journey/` | Journey spec format and HTTP execution |
| `commands/status.rs`, `db/queries/stats.rs` | Compass/maturity logic (reference intent, not code) |

Never copy v1 code. Re-derive clean.

---

## MVP — Rings 1–7

### Ring 1 — Core model and store

**What:**

- Node table with `NodeType` enum and core fields
- Unified `edge` table with `EdgeTruthClass`, `InspectionStatus`, `kind`, `criterion`, `evidence`, `confidence`, `depends_on`
- `edge_kind_registry` table (kind → from/to types, allowed_truth_classes, owner_role)
- `property_schema_registry` and `tag_vocabulary` tables
- `facet` table (node + edge facets with truth_class)
- `loom init` — idempotent, creates `.loom/graph.sqlite`, stamps identity
- `loom import` — two-phase (validate-then-write; never partial graph on error)
- `loom export` — deterministic JSON; same graph → identical bytes
- Basic node CRUD (add, show, list) for Intent and CodeFile

**Invariants to test in this ring:**

- INV-4: `independent` rows exist only with non-empty evidence (write-time rejection)
- INV-5: Derived-only writer cannot touch asserted columns; asserted-only writer cannot touch derived — enforced at the repository layer
- INV-6: Asserted edge with empty criterion or evidence → rejected at write time
- INV-ATOM: symbol-named intents rejected unless `--allow-symbol-name` + behavioral description
- Edge-kind registry: wrong endpoint types and disallowed truth classes rejected at write time
- Import round-trip: export → fresh init → import → export → byte-identical; malformed import rejected loudly; import refuses a non-empty graph
- (INV-2 is a ring-2 invariant: it requires `sync` to exist. Verified there, not stubbed here.)

**Entry criterion:** design.md §4 unified edge schema agreed and terminology.md stable.
**Exit criterion:** `loom init` + `loom export` + `loom import` green on a scratch graph.

---

### Ring 2 — Structural plane

**What:**

- `loom codefile add '<glob>'` — register files, detect language/role
- Tree-sitter extraction: symbols, imports, loc, content hash, generated/vendor/test classification
- `loom sync` — content-hash change detection; re-extracts on change; never false-flags on mtime
- Ripple from `loom sync`: realizing implements locators → needs_reverification + dependent ripple; consumes/configures/verifies groundings reopen only on seam-locator drift; governs → needs_reverification; Validation.last_result → not_run + linked validates → needs_reverification; asserted relationships remain settled only when their own stamped citations fully cover the exact changed dependency files and remain byte-intact
- Derived `Finding` nodes for structural occurrences (oversized_file, complex_symbol, panic_marker) + `flags`/`assesses` edges — derived, recomputed by sync
- InterfaceSurface extraction is not current; `exposes` edges are asserted-only, not derived by sync
- `loom codefile show` — ownership view: owners, symbols, imports, stale count
- Blast-radius preview is deferred/not current; use WorkItem context/read_set and `loom status` instead.

**Invariants to test:**

- INV-2: Wipe all derived facts → sync → byte-identical result (the golden sync test)
- Localized-edit fan-out: ten intact relationship claims create zero packets; the changed grounding plus missing/rewritten relationship evidence creates at most three full-judgment packets
- Sync is content-hash based: touching mtime without content change → no ripple
- Derived facts never enter the asserted residue queue
- `loom sync` output: N changed, M staled, K validations reset

**Reference:** `../../loom/src/commands/sync.rs` for staleness edge cases.
**Exit criterion:** `loom sync` correct on a multi-file repo; golden sync test green.

---

### Ring 3 — Judgment plane

**What:**

- Intent full CRUD: `loom intent add / update / set / mark / confirm / retire / show / list / tag`
- Intent update ripple: description change → one-hop needs_reverification + old wording in Note
- Intent retire: status=deprecated, invisible to computation, fallout reported
- Edge CRUD for asserted edge kinds through `loom edge relate <kind> <from> <to>` where kind is `hierarchy`, `requires`, `scenario-of`, `variant-of`, `triggers`, `sequence`, or `relates`
- `loom edge explore ground / issue / independent` (verdict commands with evidence gates)
- `loom edge implement --role realizes|consumes|configures|verifies`, `loom edge call`, `loom edge remove`, `loom edge set-locator`, `loom edge set-role`, `loom edge rehome`
- Role gates: write-time check of owner_role vs edge_kind_registry
- `loom next` — asserted residue router: one WorkItem + PromptContract per call
  - Modes: build, coverage, fix, analyze/discovery, validate, quality, prove, triage, review
  - `--all` closeout view
- `loom door "<utterance>"` + InboxItem landing menu + `loom inbox` add/list/show/mark/remove
- **TaskRecord** — `loom task add / start / close / abandon / show / list`; lightweight operational work record; lives alongside InboxItem as an intake/work-tracking node
- `loom session` — turn-zero offer menu
- `loom status` — maturity + compass + graph_state including low-confidence and bounded adversarial Review debt
- `loom challenge record/show/list` — one snapshot-bound adversarial attempt per Verdict revision; counterexamples atomically route to Finding/Triage
- `loom guide --role <role>` — PromptContract for a lane

**Invariants to test:**

- INV-1: Adding N unrelated intents → O(N) hierarchy edges, not O(N²) rows
- INV-4: `independent` edge without evidence → rejected
- INV-7: Write from wrong role → rejected
- Asserted residue queue: only `truth_class=asserted AND status IN (uninspected, needs_reverification)`
- Role gate: `quality` role cannot write `hierarchy` edge
- `loom next --json` output carries `work_item` + `prompt_contract` + `next_step` + `graph_state`
- TaskRecord never counts toward maturity or coverage
- **INV-ATOM: Atomization guard.** `loom intent add` rejects symbol-pattern names (snake_case with no spaces) unless both `--allow-symbol-name` AND a non-empty behavioral `--description` are provided. Override is recorded on the node. `loom doctor` surfaces all overrides as an audit trail for function-level granularity drift

**Exit criterion:** `loom next` serves correct WorkItem + PromptContract; role gates enforced; intent ripple correct; TaskRecord CRUD green and excluded from maturity/coverage.

---

### Ring 4 — Maturity and compass

**What:**

- Maturity ladder: Seeded → Realized → Proven → Hardened → Excellent → Exported
  - Each rung is a vector, not a scalar; lowest unmet rung is routing focus
  - `Excellent` gates on open findings/smells, not raw statistical-debt instance count
  - `Hardened` gates on asserted coupling residue (stale/uninspected), never grid denominator
- `loom status` — full maturity output, compass, queue counts, graph_state, alarms
- `loom next --all` — closeout view: every queue + gaps + doctor health
- `loom status` compass: single recommended next command
- `loom guide` — full driving protocol; `--role builder|analyzer|fixer|validator|quality|monitor`; `--json truth_axes` includes each axis's `correct_when`
- `loom completeness [<intent>]` — Definition-of-Complete scorecard; `loom intent waive <intent> <axis> --reason` records deliberate waivers that reopen when meaning changes
- `loom schema` — live node/edge/property/status vocabulary from registry
- `loom find "<query>"` — keyword-substring search across intents, codefiles, surfaces, rules, validations
- Queue priority scoring (centrality, effort, staleness, confidence)
- `loom detect` — repo heuristics, project type, seedable pack recommendations

**Invariants to test:**

- INV-3: Statistical signal never enters maturity gate or required queue count
- `Excellent` gate: 18k advisory signals do not produce 18k required items
- `loom status --json` structure: maturity, graph_state, validation_summary, code_ownership, queues, debt
- `loom next --all` returns compass, graph_state, and the top item for each queue

**Exit criterion:** `loom status` and `loom next` correctly route; maturity ladder advances and regresses correctly.

---

### Ring 5 — Quality, hypothesis, journey model, vocab, and interface surfaces

**What:**

- `QualityRule` with enriched seeded-pack fields: `inspection_guide`, `detection_hints`, `evidence_template`, `passing_example`, `failing_example`
- `governs` edge + `loom rule verdict` (asserted, role=quality)
- `loom rule seed <pack>` — iso5055, service, web-ui, data, concurrency, docker; 29 rules total; pack rules ship pre-authored with all guidance fields
- `loom rule add / remove / unlink / list / show` — current custom-rule CLI is deliberately small; rich guidance arrives through seeded packs
- `loom next --mode quality` — uninspected/stale governs edges only; failing governs route to fix; if none exists, fallback proposes the first never-measured rule × root implemented intent pair and the verdict creates the edge
- `CodeRule` + `Finding` derived pipeline — sync detects oversized files, complex symbols, panic markers → Finding nodes + flags/assesses edges; derived, recomputed by sync
- `loom codefile show` — includes governing rules and findings
- `loom next --mode validate` — uninspected/stale validates edges only; failing validates route to fix
- `loom validation add / verdict / update / unlink / remove / show / list`
- `loom validation run [<intent-or-validation>] | --all`
- Note commands: `loom note add <target> / list`
- **Hypothesis plane** — `Hypothesis` node CRUD; `targets` edge; `loom hypothesis add / prove / adopt / reject / show / list`; `loom next --mode prove`
- **Journey model** — `loom journey add <spec>` creates Validation(type=journey) + validates edges to step intents + calls edges to InterfaceSurfaces; spec parsing and graph write-back
- **InterfaceSurface asserted** — `loom surface add / show / update / remove / gaps / list`; asserted `exposes` edges; `calls` edges from Validation → InterfaceSurface
- **Vocab and layer** — `loom vocab add / remove / list`; `loom intent tag add / remove`; `loom layer order / list / clear`; arms layering violation and duplicated-responsibility smells

**Invariants to test:**

- Quality verdict at component altitude covers descendants (hierarchy-aware)
- `independent` quality verdict requires evidence of non-applicability
- Finding nodes are derived: wipe + sync → byte-identical Finding set
- Pack seeding is idempotent; all guidance fields present after seed
- Validate `--all` does not re-run settled verdicts
- Quality fallback after pack seed produces actionable work for never-measured rule × root implemented intent pairs
- Fix/quality/validate queues are disjoint: failing governs/validates → fix; stale/uninspected governs → quality; stale/uninspected validates → validate
- Hypothesis invisible to coverage and maturity until adopted
- `loom journey add` creates correct Validation + edge set; no HTTP call made
- Vocab term collision arms `duplicated_responsibility` smell
- Layer order arms `layering_violation` smell; no layer order = no false positives

**Exit criterion:** Quality, hypothesis, journey spec, surface, vocab, and layer commands green. `loom next --mode prove` serves items.

---

### Ring 6 — Signal plane and journey runner

**What:**

- **Journey HTTP runner** — `loom journey run <spec>` executes HTTP journeys with reqwest/rustls; response capture; response threading into subsequent steps; stamps passing/failing evidence on `validates` edges; failing boundary records exact broken expectation and reopens never-reached previously-passing steps
- **`loom journey diagnose <spec>`** — dry-run without graph writes; explains failure roots (missing env, auth failures, 404s, template mismatches)
- **Journey coverage/invariant/prompt** — `loom journey coverage add/list/discover/drift`, `loom journey invariant add/list`, and `loom journey prompt <intent>`
- `loom smells` — all structural signals now fully armed:
  - twin intents, overlapping ownership, tangle, undeclared coupling
  - layering violations (armed by `loom layer order` from ring 5)
  - complex symbols, hub files, happy-path-only groups
  - duplicated responsibility (armed by vocab from ring 5), unjourneyed surfaces
  - vocab drift (armed by vocab registry from ring 5), `consumer_owned_file`
  - Each finding carries exact remedy command; open findings gate `Excellent` rung
- `loom debt` — statistical cluster feed: shipped = LOC `size_outlier` + git-history `co_change` with stable `cluster_id`s and `loom debt promote` (one asserted Finding, `source: debt_promotion`); clone/shotgun/recurrence/proof-locality remain deferred design examples; computed on demand, never stored as edges
- `loom scan add/list/remove/run` — external diagnostic adapters (linters/type-checkers) whose diagnostics become derived findings in the normal triage lifecycle
- `loom doctor` — schema conformance, evidence vacuity, provenance, role gate audit, and `consumes_without_seam`
- Centrality hotspot and focused-dig helpers are deferred/not current; use `loom next` WorkItem context/read_set instead.

**Invariants to test:**

- Journey run stamps passing evidence on validates edges for each passing step
- Journey run records failing edge at the exact boundary step; subsequent steps are reopened if previously passing and never reached
- Journey diagnose does not write to the graph
- INV-3: `loom debt` output contains zero stored edge rows for statistical signals
- `loom smells` layering violation fires after `loom layer order` declared; silent before
- `loom smells` duplicated_responsibility fires after vocab tag collision; lexical fallback always runs
- `loom doctor` non-zero exit on any integrity violation
- Co-change signal does not appear in `loom next` required queue (holds non-vacuously now that co-change is computed)

**Reference:** `src/journey.rs` and `src/commands/journey.rs` for HTTP execution, JSONPath-style field checks, and evidence-stamping semantics.
**Exit criterion:** `loom journey run` stamps correct evidence; `loom smells` fully armed; `loom debt` + `loom doctor` correct.

---

### Ring 7 — Coverage, export/import, dogfood

**What:**

- `loom coverage` — vertical spine: intent tree shape, leaf grounding, file ownership, unaccounted files
- `loom ignore add / remove / list` — exclusions with recorded reason
- `loom export` — deterministic `loom.graph.json`; `--check` exits non-zero on drift
- `loom import <file>` — two-phase restore into a fresh graph; planned-port import (`--as-planned`) is deferred/not current
- `loom whoami` — acting identity + role + lane enforcement status
- **Dogfood:** run loom on loom's own codebase
  - Seed intents for loom v2 rings 1–7
  - Ground to src files
  - Run quality packs
  - Export `loom.graph.json` committed to repo
  - `loom export --check` green in CI

**Invariants to test (full suite):**

- INV-1: O(N) hierarchy rows for N unrelated intents
- INV-2: Derived plane rebuildable byte-for-byte
- INV-3: No statistical row in edge table
- INV-4: `independent` only with evidence
- INV-5: Derived writer / asserted writer are partitioned
- INV-6: Evidence gate rejects empty criterion
- INV-7: Role gate rejects wrong-lane write
- `loom export --check` exits non-zero when graph has unsaved changes
- Import round-trip: export → init → import → export → byte-identical
- Planned-port import that drops groundings/proofs is deferred; current import round-trip remains byte-identical.

**Exit criterion:** All 7 invariants green; dogfood graph exists and `loom export --check` passes in CI; loom can map itself.

---

## Milestone 2 (originally deferred — mostly shipped since)

These were deferred from the MVP as significant new subsystems. Their eventual fate:

| Feature | Status |
|---|---|
| Wiki projection | **Shipped** (v0.20.0) as `loom wiki plan/next/record/list/remove` — smaller than the `wiki-projection.md` design (no generate/verify/publish pipeline; the graph governs truth + freshness, an agent writes prose) |
| Federation | **Shipped** (v0.22 line) as `loom graph link/unlink/list` + `loom edge depends-on` over committed exports; permanent dispose via `unlink --prune` / `prune-orphans` (`--cascade` for remaining DependsOn); observed graph mode via `loom mode` / `init --observed` |
| Personas | Still deferred — niche; no pressing model demand |

---

## Test strategy

### Ring-local tests

Each ring owns tests for its own invariants. Tests live beside the code.

```text
ring 1: storage/contract/regression
ring 2: golden sync test, extraction accuracy per language
ring 3: role gate, evidence gate, queue routing, ripple
ring 4: maturity ladder, compass routing, keyword find
ring 5: quality pack idempotency, finding derivation, validate --all
ring 6: journeys, smells correctness, debt never stored, doctor audit
ring 7: all invariants, export/import round-trip
```

### Invariant tests

The 7 invariants from `graph-model.md` are first-class tests, not assertions buried in unit tests. Each is independently runnable and named for its invariant. Where they live:

```text
INV-1  tests/ring3.rs   inv1_no_grid_materialization (O(N) edges for N unrelated intents)
INV-2  tests/ring2.rs   inv2_derived_plane_rebuildable (+ wipe/rebuild variants in ring8, ring12)
INV-3  tests/ring6.rs   debt_size_outlier_is_not_stored, inv3_debt_never_gates_or_queues
INV-4  tests/ring1.rs   inv4_independent_requires_evidence
INV-5  tests/ring1.rs   inv5_verdict_path_rejects_derived_edge / inv5_derived_path_rejects_asserted_edge
INV-6  tests/ring1.rs   inv6_passing_requires_criterion_and_evidence
INV-7  tests/ring3.rs   inv7_wrong_lane_rejected_right_lane_allowed / inv7_verdict_lane_enforced
```

### Dogfood as integration test

`loom export --check` in CI is the top-level integration test: it proves the graph is consistent, the export is deterministic, and the CLI commands work end to end on a real codebase (loom itself).

### Test philosophy

- Test behavior, not plumbing
- No mocks — test against real SQLite
- A config or string change should not break a test
- Assert logical behavior and invariants, not current state snapshots
- Aim at conditional branches, edge values, invariants, and error handling

---

## Dogfood milestone

The dogfood milestone is the exit condition for the full MVP. It is defined as:

```text
1. loom v2 binary exists and builds (zero warnings, rustfmt, clippy)
2. loom.graph.json committed to loom_new repo
3. loom.graph.json covers rings 1–7 intent hierarchy, grounded to src files
4. loom export --check green
5. loom status returns meaningful maturity (not empty/seed phase)
6. loom next serves at least one real WorkItem with PromptContract
7. loom smells returns zero open findings or every finding has a decision note
8. loom doctor exits zero
9. All 7 invariant tests green
```

When the dogfood milestone passes, the binary is ready to be used as a companion on other codebases.

---

## Addendum — rings shipped after this plan (8–12)

The plan above ends at ring 7. Development continued ring-by-ring under the same green-before-next principle; each later ring has its own integration suite under `tests/`:

```text
ring 8   findings triage: durable adjudication verdicts across syncs, smells materialized as findings
ring 9   weak-worker fidelity: review queue (confidence floor), disjoint queue partition,
         self-contained work packets, door landing, quality packs with pre-screening
ring 10  sensors + Definition-of-Complete: scan adapters, completeness scorecard + waivers,
         elaborate lane, calibrated thresholds, portable config
ring 11  grounding roles on implements edges (realizes|consumes|configures|verifies),
         coverage ownership, seam drift, observed-graph lane gating
ring 12  loom apply atomic batches; cross-graph federation (graph link, UpstreamIntent
         shadows, depends-on edges); wiki projection (v0.20); prove→adopt→build handoff (v0.21);
         queue depth views + graph mode (v0.22)
```

`CHANGELOG.md` is the authoritative per-release record for this period.

## Addendum — LLM-fidelity hard cuts (post 0.24)

After the LLM-perspective review, these process gates apply until residue economics are proven on dogfood:

1. **No new CLI families** until `routing_hint` / cheap-reconfirm routing and hybrid relates ripple are in use, and a typical single-file edit does not regenerate a mid-effort analyze wall dominated by one-sided `relates` fanout.
2. **Operator skills must not duplicate command recipes** — the optional global `loom-driver` skill (if installed at a global skill root) points at `docs/llm-driver.md` / `docs/commands.md` / `loom guide --json`. loom itself requires no skill: `docs/` and the compiled help are the authoritative instruction surface. CI (`tests/intake_fidelity.rs`) forbids teaching rejected inbox sources.
3. **Statistical signals never enter required queues** (INV-3 unchanged). `loom debt` stays advisory.
4. **Bootstrap drafts only** — `loom bootstrap suggest` writes a Proposal of non-authoritative Journey clues. Inspect product evidence, then author and register `loom.journey/v1` roots; never auto-create product meaning, verdicts, or `implemented` lifecycle.
5. **Detector economics** — resolving finding adjudications reopen on metric-band worsen, not every content-hash bump; dogfood runs `loom calibrate --write` before mass oversized triage.
6. **Completeness ↔ maturity consistency** — a recorded `waiver:journey` closes the journey axis on the scorecard **and** stops counting that intent's journey-proof smell against the `proven` rung. Prefer real S3-or-stronger journey proofs: CLI `run:` steps for tool surfaces, HTTP for APIs — so unknown repos climb without waiving when a runner exists.
7. **Quality measures leaves, not roll-ups or scenarios** — `unmeasured_quality_pairs` targets leaf implemented intents; hierarchy parents and scenario children (`scenario-of` / sad|fallback|edge_case) are excluded so ISO-style rules are not measured against empty parents or surroundings.

## What the build plan does not contain

- Feature timeline or delivery dates — not loom's concern
- Model/vendor selection — effort tiers are the harness's mapping
- UI or GUI — CLI only; viewer is milestone 2+
- Breaking-change policy — v2 is greenfield; no compatibility obligation to v1 graph format until a v1→v2 importer is built (optional milestone 2 item)
