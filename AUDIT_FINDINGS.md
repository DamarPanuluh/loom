# loom — Exhaustive Codebase Audit Findings

**Date:** 2026-07-02
**Auditor:** Automated exhaustive trace of all 66 git-tracked files
**Method:** Every `.rs` file read in full (explicit line ranges, no elided summaries); docs read in full; data validated programmatically; tests cross-checked against CLI surface and public API.

---

## Coverage Summary

| Category | Files | Audited | Excluded | Clean | Files w/issues |
|---|---|---|---|---|---|
| src (Rust) | 37 | 37 | 0 | 19 | 18 |
| tests (Rust) | 10 | 10 | 0 | 0 | 10 |
| docs (Markdown) | 10 | 9 | 1 | 9 | 0 |
| scripts | 2 | 2 | 0 | 2 | 0 |
| config | 6 | 6 | 0 | 6 | 0 |
| data | 1 | 1 | 0 | 0 | 1 |
| **Total** | **66** | **65** | **1** | **36** | **29** |

**Test execution note:** `cargo test --test ring7 --test ring8 --test ring9 --test ring10` passed (48 tests, per tests-3 subagent). `cargo test --test ring4 --test ring5 --test ring6 --no-run` compiled clean (per tests-2 subagent). ring1-3 test execution was not independently verified.

---

## Issues by Severity

### BLOCKER / HIGH (13 issues)

#### H-1: `redefine_intent` doesn't ripple `implements` edges
**File:** `src/store.rs:616`
**Category:** logic error / stale graph
`redefine_intent` promises to ripple every verdict touching the intent, but it includes `EdgeKind::Implements` in a loop that only queries `edges_with(kind, None, Some(id))`. `Implements` is `Intent → CodeFile`, so implementation edges for the redefined intent have the intent as `from_id` and are never reopened. **Impact:** Redefining an intent leaves its grounding verdicts settled when they should become stale.

#### H-2: INV-6 weakened — `independent` accepts empty criterion
**File:** `src/store.rs:773-778`
**Category:** invariant mismatch
The file-level contract says INV-6 requires non-empty criterion and evidence for `passing`, `failing`, and `independent` verdicts (line 11), but the `Independent` branch only checks evidence and accepts an empty criterion. **Impact:** Weakened write-boundary invariant the module advertises.

#### H-3: `Store::init` race condition
**File:** `src/store.rs:168`
**Category:** race condition / idempotence
`Store::init` documents idempotent initialization but computes `fresh = !db_path.exists()` before acquiring the graph lock at line 169. Two concurrent `loom init` processes can both decide the DB is fresh; the second then waits, opens the already-initialized DB, enters the stale `fresh` branch, and hits duplicate plain `INSERT` meta writes. **Impact:** Concurrent init can corrupt meta rows.

#### H-4: `Agent::parse` falls through typos to Solo
**File:** `src/store.rs:91`
**Category:** lane enforcement gap
`Agent::parse` falls through every unrecognized `LOOM_AGENT` value to `Agent::Solo`. A typo such as `llm:qualtiy` silently disables lane gates instead of failing closed. **Impact:** Role-based write restrictions can be bypassed accidentally.

#### H-5: `exposes` edge uniqueness prevents dual truth-class
**File:** `src/registry.rs:163-165`
**Category:** schema/registry inconsistency
The registry explicitly allows `exposes` edges to be both derived and asserted for the same logical relationship, but the SQLite edge table is unique only on `(from_id, to_id, kind)` and `add_derived_edge` only ignores conflicts on deterministic `id`. An asserted and a derived `exposes` edge with the same endpoints cannot coexist despite the registry contract.

#### H-6: `rebuild_findings` wipes external diagnostic findings
**File:** `src/sync.rs:459`
**Category:** data loss
`rebuild_findings` wipes the entire derived graph but only rebuilds structural findings. External scan diagnostics are persisted as derived `Finding` nodes, so a normal sync deletes all external diagnostic findings until the scan adapter is rerun. **Impact:** `loom sync` silently destroys scan-derived findings.

#### H-7: Undeclared-coupling import matching broken for Rust
**File:** `src/signal.rs:207`
**Category:** coherence / import resolution
Undeclared-coupling smells resolve imports by substring-matching extracted import text against registered codefile paths. Rust extraction stores module syntax (`crate::model::...`), registered files are paths (`src/model.rs`). Most Rust imports invisible to coupling detection.

#### H-8: Journey silently skips unresolved step intents
**File:** `src/journey.rs:381`
**Category:** proof recording
Recorded journey runs silently skip a step when `step.intent` cannot resolve, because verdict recording is inside `if let Ok(intent)`. The HTTP outcome is still pushed and the journey can be marked `passed` with no `validates` verdict for that step. **Impact:** Green journey with unlinked steps.

#### H-9: `intent show` drops `--json` flag
**File:** `src/commands/intent.rs:35`
**Category:** json-output-gap
`IntentCmd::Show` is the only intent subcommand whose dispatch drops the `json` flag. `loom --json intent show` always prints human text.

#### H-10: `TaskCmd` mutations ignore `--json`
**File:** `src/commands/misc_cmd.rs:345`
**Category:** json-output-gap
`TaskCmd::Add/Start/Close/Abandon` mutate the graph but unconditionally print plain text. They ignore the `json` argument unlike `TaskCmd::Remove/Show/List`.

#### H-11: Intent lifecycle/level/visibility not validated
**File:** `src/commands/intent.rs:116`
**Category:** validation-gap
Intent lifecycle/level/visibility values are not bounded despite CLI help listing finite vocabularies. `intent_add` stores arbitrary strings, `intent_mark` writes any lifecycle. A typo like `implemeted` becomes active but neither build-gated nor proof-gated, allowing `realized` to look met.

#### H-12: `queue_counts` ignores observed graph flag
**File:** `src/workitem/queues.rs:621`
**Category:** queue-coherence
`queue_counts` does not account for observed graphs. `workitem::next` suppresses build/fix/coverage/elaborate when `identity().observed` is true, but `queue_counts` still reports those backlogs unconditionally. `loom status`/`next --all` disagree with served queues on observed graphs.

#### H-13: Journey predicate inconsistency (type vs proof_kind)
**File:** `src/commands/journey.rs:1055`
**Category:** logic-inconsistency
`is_journey_validation` treats `body.type == journey|saga` as a journey, but `current_l5_journey_validations` and `coverage_discover` only count validations whose `body.proof_kind == journey`. A validation with `--type journey` but no `--proof-kind journey` lists as a journey yet never satisfies effective coverage.

---

### MEDIUM (18 issues)

#### M-1: `prove_item` never called in default `next` chain
**File:** `src/workitem/mod.rs:189`
The default `next(store, None)` priority chain never calls `prove_item`. Proposed hypotheses invisible to plain `loom next`, starved unless `--mode prove`.

#### M-2: Blocked-proof prompt contract mismatch
**File:** `src/workitem/contracts.rs:70`
The Validates write-back prompt says `--evidence` for blocked proofs, but `mark_validation` uses `reason` for blocked. Worker puts blocker text in a field the handler ignores.

#### M-3: `SetLocator` writes facet but `edge_show` never reads it
**File:** `src/commands/edge.rs:327`
`SetLocator` writes a `locator` facet, but `edge_show` prints/serializes only the bare `Edge`. The correction is invisible through `loom edge show`.

#### M-4: `HypothesisCmd::Prove` accepts empty evidence
**File:** `src/commands/domain_cmd.rs:84`
Accepts `supported`/`refuted` with empty `--evidence` and immediately sets status. No evidence required for truth-changing decision, unlike findings/rule verdicts/validation marks.

#### M-5: `HypothesisCmd::Adopt` bypasses intent shape
**File:** `src/commands/domain_cmd.rs:112`
Spawns Intent with empty body, no level facet, bypasses `intent add` shape and symbol-name validation. Adopted hypotheses create intents missing metadata other code expects.

#### M-6: Maturity ladder accepts unvalidated lifecycle strings
**File:** `src/maturity.rs:78`
Only exact `planned|needs_change`/`implemented` strings count. Typo `implemeted` becomes active but neither build-gated nor proof-gated. (Compounds with H-11.)

#### M-7: Export format version not checked on import
**File:** `src/travel.rs:77`
`Export::from_json` never checks `format` field against current FORMAT constant. `format: 999` accepted and restored as current schema.

#### M-8: Scan adapter exit status not checked
**File:** `src/scan.rs:265`
`run_adapter_command` returns stdout/stderr without inspecting exit status. Failed adapter → no diagnostics → existing findings deleted (convergence on failure).

#### M-9: `layer_order` malformed metadata silently ignored
**File:** `src/signal.rs:777`
Uses `unwrap_or_default()`. Malformed `layer_order` silently becomes empty list, disabling all layering checks.

#### M-10: Derived `exposes` edges never rebuilt by sync
**File:** `src/sync.rs:193`
Sync ripple consumes `EdgeKind::Exposes` to reset contracts, registry allows derived exposes, but no `add_derived_edge(Exposes)` producer exists. Derived exposes modeled and wiped but never rebuilt.

#### M-11: `redefine_intent` discards validation reset errors
**File:** `src/store.rs:621`
Validation reset failures from `set_node_status` are discarded with `.ok()`. Failed reset leaves validation showing old status while command reports success.

#### M-12: `add_edge` permits non-deterministic derived edges
**File:** `src/store.rs:675`
`add_edge` accepts `TruthClass::Derived` then generates random id and live timestamp. Public API permits derived edges through non-deterministic path instead of forcing `add_derived_edge`.

#### M-13: `set_derived_status` accepts asserted-verdict statuses
**File:** `src/store.rs:813-823`
Verifies only that edge is derived, then writes any `InspectionStatus`. Can store `passing`/`failing`/`independent` on a derived edge, contradicting the model that `current` is the resting state.

#### M-14: `restore` doesn't validate facet/tag target_ids
**File:** `src/store.rs:1101-1118`
`restore` validates edges before writing, but inserts facets and tags without validating `target_id` points to an imported node or edge. No FK on `target_id` in schema. Orphaned facets/tags can be imported.

#### M-15: `ValidationType` enum unwired / vocabulary drift
**File:** `src/cli/subcommands.rs:373`, `src/model.rs:142-149`
CLI advertises `journey` as a `--type`, but `ValidationType` enum has `Saga => "saga"` and no `journey` variant. The validation add handler stores the raw string verbatim rather than using `ValidationType`. Type vocabulary not enforced, can drift.

#### M-16: `row_to_node`/`row_to_edge` silently mask corruption
**File:** `src/store.rs:1798-1818`
Invalid `body` JSON → `{}`, invalid `depends_on` JSON → `[]`. Corrupted DB row masked as empty data instead of error. Export could persist data loss.

#### M-17: `identity` masks malformed schema version
**File:** `src/store.rs:292`
Parses `meta.schema_version` with `unwrap_or(SCHEMA_VERSION)`. Malformed version reported as current, hiding corruption or migration drift.

#### M-18: Graph name mismatch in committed export
**File:** `loom.graph.json`
Graph name is `'t'`, but dogfood.sh creates graph with `--name loom`. Committed export may be stale or from a different run.

---

### LOW (17 issues)

#### L-1: `export_is_fresh` hides I/O errors
**File:** `src/travel.rs:97`
Converts all `read_to_string` errors to `Ok(false)`. Permission/IO errors hidden as drift.

#### L-2: Layering smell import matching too narrow
**File:** `src/signal.rs:308`
Uses `imp.contains(*p)` — even narrower than undeclared-coupling. Layering violations likely missed.

#### L-3: Layering multi-owner first-only
**File:** `src/signal.rs:304`
Selects only first owner layer for importing and imported files. Multi-owner files miss violations or attribute arbitrarily.

#### L-4: Bulk-stamped governs grid in export data
**File:** `loom.graph.json`
100 governs edges = 5 rules × 20 intents = ALL pairs. 80/97 passing at confidence=0.78. Only 22 unique criterion texts. Not individually inspected.

#### L-5 through L-14: Duplicated test infrastructure
**Files:** `tests/ring{1..10}.rs`
`COUNTER`/`Tmp`/`Drop` temp-dir helper duplicated across ALL 10 test files. Pattern is correct but cross-file drift risk. Additional CLI helper duplication between ring5/ring6.

#### L-15: `unwrap().unwrap()` masking in tests
**Files:** `tests/ring1.rs:573`, `tests/ring2.rs:137`, `tests/ring3.rs:127`
Double-unwrap on `get_node`/`get_edge`/`workitem::next` doesn't distinguish Result error from Option None.

#### L-16: `run()` helper in ring5 loses command context
**File:** `tests/ring5.rs:34`
Unwraps `loom::commands::run` without including Command value in panic. Hard to identify failing command.

#### L-17: mock_server discards I/O results
**File:** `tests/ring6.rs:354,459`
Discards `stream.read` and `stream.write_all` results. Socket I/O failure hidden.

---

### INFO (5 items)

| ID | File | Description |
|---|---|---|
| I-1 | `src/main.rs:9-10` | Only unsafe code: Unix SIGPIPE restoration. Tightly scoped, intentional, under audit. |
| I-2 | `loom.graph.json` | Config has no `layer_order`. Correct: dogfood.sh never calls `loom layer order`. |
| I-3 | `loom.graph.json` | All referential integrity checks pass: 0 dupes, 0 dangling, 0 wrong-type endpoints, 0 statistical edges, 0 INV-4/INV-6 violations. |
| I-4 | `tests/ring10.rs:399` | Function name typo: `waiver_facilitiy` should be `waiver_facility`. |
| I-5 | `src/model.rs:142-149` | `ValidationType` enum defines vocabulary but has no references outside its own definition. Dead code. |

---

## Test-Specific Findings

| ID | File:Line | Severity | Description |
|---|---|---|---|
| T-1 | `tests/ring2.rs:260` | medium | Labels derived-edge assertion as INV-3/INV-5 corollary, but INV-3 = statistical signals never stored. Should be INV-5 only. |
| T-2 | `tests/ring3.rs:1` | medium | build-plan assigns ring3 INV-1/INV-4/INV-7 but file only tests INV-7. No INV-1 (O(N) hierarchy) test in ring1-3. |
| T-3 | `tests/ring3.rs:274` | medium | Test named `next_analyze_serves_uninspected_then_fix_after_stale` but never creates stale edge or asserts Fix behavior. |
| T-4 | `tests/ring9.rs:1053` | medium | Test asserts inbox mark error names routed/rejected/deferred but omits 'duplicate' disposition. Would pass if 'duplicate' disappeared. |

---

## Clean Files (no issues found)

### Source (19 files)
`src/lib.rs`, `src/main.rs` (info only), `src/cli.rs`, `src/commands.rs`, `src/commands/codefile_cmd.rs`, `src/commands/status_cmd.rs`, `src/commands/diagnostics_cmd.rs`, `src/commands/proof_cmd.rs`, `src/commands/proposal_cmd.rs`, `src/commands/pulse.rs`, `src/truth.rs`, `src/prescan.rs`, `src/packs.rs`, `src/fsglob.rs`, `src/extract/mod.rs`, `src/extract/rust.rs`, `src/extract/langs.rs`, `src/workitem/context.rs`, `src/completeness.rs`

### Docs (9 files)
All 9 canonical docs are internally coherent and consistent with each other and the CLI surface.

### Scripts (2 files)
`scripts/release.sh`, `scripts/dogfood.sh` — both clean.

### Config (6 files)
`Cargo.toml`, `Cargo.lock`, `WATCHDOG.yml`, `.gitignore`, `README.md`, `CHANGELOG.md` — all clean.

---

## Cross-Cutting Themes

### 1. String-typed enums not enforced at write boundaries
Intent lifecycle (`planned|implemented|needs_change|deprecated`), level, visibility, and validation type are stored as raw strings. The `ValidationType` Rust enum exists but is never used for validation. This causes H-11, M-6, and M-15 — typos silently bypass gates and classifiers.

### 2. `--json` flag inconsistently threaded
Two commands drop the `json` flag entirely (H-9, H-10). The README and docs/commands.md promise `--json` across all read/show/list commands and mutating commands, but the implementation has gaps.

### 3. Sync rebuild wipes data it can't recreate
`rebuild_findings` wipes all derived data but only rebuilds structural findings (H-6) and doesn't rebuild derived `exposes` edges (M-10). External scan diagnostics and derived interface surfaces are silently destroyed by `loom sync`.

### 4. Import/export integrity gaps
Format version unchecked (M-7), facet/tag targets unvalidated (M-14), corrupted JSON silently masked (M-16), schema version masked (M-17). The import path is less defensive than the edge validation path.

### 5. Error handling suppresses failures
Multiple `.ok()` / `unwrap_or_default()` / `unwrap_or(false)` patterns silently swallow errors that should surface: validation resets (M-11), layer_order parsing (M-9), export freshness (L-1), adapter failures (M-8), corruption (M-16).

### 6. Test infrastructure duplication
The `Tmp`/`Drop` pattern is copy-pasted across all 10 test files. The `run_cli` helper is duplicated between ring5/ring6 with different behavior. Centralizing would reduce drift risk.

---

## Exclusions

| File | Reason |
|---|---|
| `docs/scratchpad.md` (2412ln) | Explicitly non-canonical per `docs/README.md`: "Raw design staging area and working log \| No". Not a shipped artifact. |

## Limited-Scope Audits

| File | Scope |
|---|---|
| `Cargo.lock` | Version coherence verified (loom 0.13.0, all deps resolve, serde_yaml 0.9.34+deprecated noted). Not line-audited — auto-generated lock file. |

## Methodology

- **Manifest:** `git ls-files` — 66 tracked files, all accounted for.
- **Source (37 .rs files):** Read in full via explicit line ranges (300-line chunks). No elided summaries accepted. Delegated to 3 parallel subagents (core-models, pipeline, commands) each with per-file coverage matrices.
- **Tests (10 .rs files):** Read in full via explicit line ranges. Delegated to 3 parallel subagents (tests-1, tests-2, tests-3) each with per-file coverage matrices.
- **Docs (10 .md files):** Read in full by main agent (explicit ranges for files >300 lines). 9 audited, 1 excluded as non-canonical.
- **Scripts (2 files):** Read in full by main agent.
- **Config (6 files):** Read in full by main agent. Cargo.lock version coherence verified.
- **Data (loom.graph.json):** Programmatically validated: node ID uniqueness, edge endpoint existence, edge endpoint type correctness per registry, INV-3/INV-4/INV-6 invariant compliance, config key completeness, graph identity coherence.
- **Cross-checks:** CLI subcommand enums vs handler match arms (179 paths, no mismatches); invariant IDs across docs (INV-1 through INV-7 consistent); edge kind count (17 in docs and code); test execution (ring7-10 passed, ring4-6 compiled).
