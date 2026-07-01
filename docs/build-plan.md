# loom v2 — Build Plan

Status: canonical draft. This is the implementation sequencing plan: MVP rings, milestone 2 scope, test invariants, and dogfood milestone. Follows `design.md §9` locked decisions. All design is settled in the preceding docs before a line of code is written.

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
reqwest         HTTP for saga runner (rustls, blocking — milestone 2)
serde / serde_json  serialization
fs2             cross-process file lock for concurrent access
```

v1 (`../../loom`) is a **read-only reference oracle**. Read it for:

| v1 mechanism | Reference value |
|---|---|
| `repo.rs`, `ts_imports.rs` | Tree-sitter extraction quirks per language |
| `db/sqlite*` | WAL mode, busy timeout, locking patterns |
| `commands/sync.rs` | Staleness edge cases, ripple ordering |
| `saga/` | Saga spec format and HTTP execution (milestone 2) |
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
- Ripple from `loom sync`: implements locators → needs_reverification, governs → needs_reverification, Validation.last_result → not_run + linked validates → needs_reverification, asserted relationships → needs_reverification
- Derived `Finding` nodes for structural occurrences (oversized_file, complex_symbol, panic_marker) + `flags`/`assesses` edges — derived, recomputed by sync
- Derived `exposes` edges for deterministically extractable interface surfaces (HTTP decorators, CLI annotations)
- `loom codefile show` — ownership view: owners, symbols, imports, stale count
- `loom impact` — blast radius preview for one file or intent

**Invariants to test:**

- INV-2: Wipe all derived facts → sync → byte-identical result (the golden sync test)
- Sync is content-hash based: touching mtime without content change → no ripple
- Derived facts never enter the asserted residue queue
- `loom sync` output: N changed, M staled, K validations reset

**Reference:** `../../loom/src/commands/sync.rs` for staleness edge cases.
**Exit criterion:** `loom sync` correct on a multi-file repo; golden sync test green.

---

### Ring 3 — Judgment plane

**What:**

- Intent full CRUD: `loom intent add / update / mark / confirm / retire / show / list / context`
- Intent update ripple: description change → one-hop needs_reverification + old wording in Note
- Intent retire: status=deprecated, invisible to computation, fallout reported
- Edge CRUD for all asserted edge kinds: `hierarchy`, `requires`, `scenario_of`, `variant_of`, `triggers`, `sequence`, `relates`
- `loom edge explore ground / issue / independent` (verdict commands with evidence gates)
- `loom edge implement / unimplement`
- Role gates: write-time check of owner_role vs edge_kind_registry
- `loom next` — asserted residue router: one WorkItem + PromptContract per call
  - Modes: build, fix, analyze, align
  - `--take N` bulk read for batch loops
  - `--compact` minimal projection
- `loom door "<utterance>"` + InboxItem + `loom inbox` full CRUD
- **TaskRecord** — `loom task add / start / close / abandon / list`; lightweight operational work record; lives alongside InboxItem as an intake/work-tracking node
- `loom session` — turn-zero offer menu
- `loom status` — maturity + one_turn plan (compass, queue counts, human_gated)
- `loom guide --role <role>` — PromptContract for a lane
- `loom batch -` — newline-delimited JSON bulk writes

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

- Maturity ladder: Seeded → Realized → Proven → Hardened → Excellent
  - Each rung is a vector, not a scalar; lowest unmet rung is routing focus
  - `Excellent` gates on triaged `DebtCluster` impact threshold, not raw instance count
  - `Hardened` gates on asserted coupling residue (stale/uninspected), never grid denominator
- `loom status` — full maturity output, compass, queue counts, human_gated total, alarms
- `loom next --all` — closeout view: every queue + gaps + doctor health
- `loom status` `one_turn` plan: single lane, role, guide command, next queue
- `loom guide` — full driving protocol; `--mode greenfield|brownfield|refactor|port|seed`
- `loom schema` — live node/edge/property/status vocabulary from registry
- `loom find "<query>"` — BM25 search across intents, codefiles, surfaces, rules, validations
- Queue priority scoring (centrality, effort, staleness, confidence)
- `loom detect` — repo heuristics, project type, pack recommendations

**Invariants to test:**

- INV-3: Statistical signal never enters maturity gate or required queue count
- `Excellent` gate: 18k advisory signals do not produce 18k required items
- `loom status --json` structure: maturity, queues, human_gated, one_turn, alarms
- `loom next --all` returns bounded output with `+N more` markers

**Exit criterion:** `loom status` and `loom next` correctly route; maturity ladder advances and regresses correctly.

---

### Ring 5 — Quality, hypothesis, saga model, vocab, and interface surfaces

**What:**

- `QualityRule` + `CodeRule` full CRUD with enriched fields: `detection_kind`, `patterns[]`, `inspection_guide`, `detection_hints`, `evidence_template`, `passing_example`, `failing_example`
- `governs` edge + `loom rule verdict` (asserted, role=quality)
- `loom rule seed <pack>` — iso5055, service, data, concurrency, web-ui, mobile, docker; pack rules ship pre-authored with all guidance fields
- `loom rule add / update` — custom rules with `--pattern-kind regex|tree_sitter --pattern '<query>' [--pattern-scope file|symbol]` for machine-executable patterns (scope `file` runs against codefile text, `symbol` against extracted symbol bodies); `--detection-hint` for LLM-facing prose guidance only
- `loom next --mode quality` — uninspected + failing governs edges grouped by intent neighborhood; quality WorkItem PromptContract embeds rule's inspection guide, hints, evidence template, examples, and pre_screened_hits
- Pattern pre-screening: for `QualityRule.detection_kind=pattern`, sync runs patterns[] against codefiles and attaches hits as `pre_screened_hits` in the WorkItem; LLM confirms verdict
- `CodeRule` + `Finding` derived pipeline — sync detects oversized files, complex symbols, panic markers → Finding nodes + flags/assesses edges; derived, recomputed by sync
- `loom codefile show` — includes governing rules and findings
- `loom next --mode validate` — Validation.last_result=not_run + failing validates edges
- `loom validation add / mark / update / delete / list`
- `loom validate <intent> | --all`
- Note commands: `loom note add / list`
- **Hypothesis plane** — `Hypothesis` node CRUD; `targets` edge; `loom hypothesis add / prove / adopt / reject / list`; `loom next --mode prove`
- **Saga model** — `loom saga add <spec>` creates Validation(type=saga) + validates edges to step intents + sequence edges + calls edges to InterfaceSurfaces; spec parsing and graph write-back
- **InterfaceSurface asserted** — `loom surface add / show / list`; asserted `exposes` edges; `loom interface gaps`; `calls` edges from Validation → InterfaceSurface
- **Vocab and layer** — `loom vocab add / list / merge`; `loom intent tag add / remove`; `loom layer order / list / clear`; arms layering violation and duplicated-responsibility smells

**Invariants to test:**

- Quality verdict at component altitude covers descendants (hierarchy-aware)
- `independent` quality verdict requires evidence of non-applicability
- Finding nodes are derived: wipe + sync → byte-identical Finding set
- Pack seeding is idempotent; all guidance fields present after seed
- Validate `--all` does not re-run settled verdicts
- Pattern rule: sync runs patterns[] and populates pre_screened_hits; LLM verdict still required
- `loom rule add/update --detection-kind pattern` with no `--pattern-*` flags → rejected at write time with an error; silent downgrade to llm_judgment is not permitted
- detection_hints never executed by sync (LLM prose only)
- Hypothesis invisible to coverage and maturity until adopted
- `loom saga add` creates correct Validation + edge set; no HTTP call made
- Vocab term collision arms `duplicated_responsibility` smell
- Layer order arms `layering_violation` smell; no layer order = no false positives

**Exit criterion:** Quality, hypothesis, saga spec, surface, vocab, and layer commands green. `loom next --mode prove` serves items.

---

### Ring 6 — Signal plane and saga runner

**What:**

- **Saga HTTP runner** — `loom saga run <spec>` executes HTTP journey with reqwest/rustls; RFC 9535 JSONPath capture; response threading into subsequent steps; stamps passing/failing evidence on `validates` edges; failing boundary records exact broken expectation
- **`loom saga diagnose <spec>`** — dry-run without graph writes; explains failure roots (missing env, auth failures, 404s, template mismatches); decodes bearer JWT scopes for auth diagnosis
- `loom smells` — all structural signals now fully armed:
  - twin intents, overlapping ownership, tangle, undeclared coupling
  - layering violations (armed by `loom layer order` from ring 5)
  - complex symbols, hub files, happy-path-only groups
  - duplicated responsibility (armed by vocab from ring 5), unjourneyed surfaces
  - vocab drift (armed by vocab registry from ring 5)
  - Each finding carries exact remedy command; open findings gate `Excellent` rung
- `loom debt` — statistical cluster feed (co-change, clone, shotgun, recurrence, proof-locality); computed on demand, never stored as edges
- `loom doctor` — schema conformance, evidence vacuity, provenance, role gate audit
- `loom hotspots` — centrality + tangle
- `loom dig <work-item>` — focused read set for one WorkItem

**Invariants to test:**

- Saga run stamps passing evidence on validates edges for each passing step
- Saga run records failing edge at the exact boundary step; subsequent steps untouched
- Saga run does not write to graph during `diagnose` mode
- INV-3: `loom debt` output contains zero stored edge rows for statistical signals
- `loom smells` layering violation fires after `loom layer order` declared; silent before
- `loom smells` duplicated_responsibility fires after vocab tag collision; lexical fallback always runs
- `loom doctor` non-zero exit on any integrity violation
- Co-change signal does not appear in `loom next` required queue

**Reference:** `../../loom/src/saga/` for HTTP execution, JSONPath, and evidence-stamping semantics.
**Exit criterion:** `loom saga run` stamps correct evidence; `loom smells` fully armed; `loom debt` + `loom doctor` correct.

---

### Ring 7 — Coverage, export/import, dogfood

**What:**

- `loom coverage` — vertical spine: intent tree shape, leaf grounding, file ownership, unaccounted files
- `loom ignore add / remove / list` — exclusions with recorded reason
- `loom export` — deterministic `loom.graph.json`; `--check` exits non-zero on drift
- `loom import --as-planned` — porting mode: intents planned, groundings dropped, proofs not_run
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
- `--as-planned` import: all intents lifecycle=planned, all Validation.last_result=not_run with linked validates edges=uninspected, all implements edges absent

**Exit criterion:** All 7 invariants green; dogfood graph exists and `loom export --check` passes in CI; loom can map itself.

---

## Milestone 2 (deferred)

Genuinely deferred: significant new subsystems or niche capabilities not needed for MVP dogfood.

| Feature | Why deferred |
|---|---|
| Wiki projection (`loom wiki plan/generate/verify/publish/update`) | Significant standalone generation surface; depends on a stable, dogfooded graph |
| Federation (`loom delegate`, observed graphs) | Cross-repo graph composition; only needed for monorepos |
| Personas | Niche; no pressing model demand in MVP scope |

---

## Test strategy

### Ring-local tests

Each ring owns tests for its own invariants. Tests live beside the code.

```text
ring 1: storage/contract/regression
ring 2: golden sync test, extraction accuracy per language
ring 3: role gate, evidence gate, queue routing, ripple
ring 4: maturity ladder, compass routing, BM25 search
ring 5: quality pack idempotency, finding derivation, validate --all
ring 6: smells correctness, debt never stored, doctor audit
ring 7: all invariants, export round-trip, as-planned import
```

### Invariant tests

The 7 invariants from `graph-model.md` are first-class tests, not assertions buried in unit tests. Each must be independently runnable and named clearly.

```text
test_inv1_no_grid_materialization
test_inv2_derived_rebuildable
test_inv3_statistical_never_stored
test_inv4_independent_requires_evidence
test_inv5_class_partitioned_writes
test_inv6_evidence_gate_rejects_empty
test_inv7_role_gate_rejects_wrong_lane
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

## What the build plan does not contain

- Feature timeline or delivery dates — not loom's concern
- Model/vendor selection — effort tiers are the harness's mapping
- UI or GUI — CLI only; viewer is milestone 2+
- Breaking-change policy — v2 is greenfield; no compatibility obligation to v1 graph format until a v1→v2 importer is built (optional milestone 2 item)
