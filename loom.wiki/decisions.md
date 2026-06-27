---
type: decision
title: "Design decisions"
tags:
  - rationale
---

## Design decisions

Rationale recorded via `loom note add --kind decision`. Newest first.

### wiki lane and self-teaching authoring loop

**Date:** 2026-06-27T03:49:52.322011+00:00

Wiki lane authoring loop is covered by sqlite_prose_check_flags_empty_prose_and_coverage_gaps, which runs wiki.rs prose consistency checks against graph exports — the regression validates the self-teaching loop end-to-end rather than isolated wiki helpers.

### whoami identity report

**Date:** 2026-06-27T03:49:52.307601+00:00

whoami identity reporting is validated by sqlite_whoami_reports, which invokes loom whoami as a subprocess and asserts stdout fields from whoami.rs — the proof intentionally targets operator-visible identity output, not an in-module unit test.

### triage queue for hypotheses

**Date:** 2026-06-27T03:49:25.920798+00:00

Hypothesis prove queue surfacing runs through loom next and reaches next/modes.rs prove branch wiring in sqlite_hypothesis_prove_queue_surfaces_proposed — direct modes.rs tests would skip the next router this intent depends on.

### porting mode: import --as-planned

**Date:** 2026-06-27T03:49:25.903479+00:00

Porting mode import --as-planned is exercised by sqlite_import_as_planned_resets_lifecycle_and_proofs, which applies import flags documented in guide.rs playbooks; the guide content is verified indirectly through import behavior it instructs.

### opt-in lane-skill install

**Date:** 2026-06-27T03:49:25.890350+00:00

Lane-skill install is validated by sqlite_skill_command_emits_lane_skills running loom skill through CLI dispatch into skill.rs template emission — in-crate unit tests would bypass the opt-in install path operators use.

### hostile import rejected loudly

**Date:** 2026-06-27T03:49:25.878453+00:00

Hostile import rejection is proven by sqlite_import_rejects_malformed_graphs hitting import.rs validators; fuzz_import.sh is an operator fixture generator, not runtime code, so the regression correctly targets import error surfacing instead of the shell script bytes.

### graph-aware manifest resolver

**Date:** 2026-06-27T03:49:25.867560+00:00

Graph-aware manifest resolver correctness is guarded by sqlite_prose_check_consistency_gate_flags_unregistered_file_link, which parses wiki.rs manifest paths against registered codefiles — static locality in tests/ does not exercise resolver path rules.

### code-primary wiki emitter

**Date:** 2026-06-27T03:49:25.855363+00:00

Code-primary wiki emission is checked by sqlite_wiki_ regressions that render wiki.rs output from exported graph fixtures — the proof target is emitted markdown structure, best verified through the wiki command entry rather than private formatters.

### tiered review queue

**Date:** 2026-06-27T03:49:25.842630+00:00

Tiered review queue behavior is validated by sqlite_review_take_drains_low_confidence_in_bulk, which calls loom next review mode and hits next/review.rs take logic plus next/scoring.rs rank ordering in one subprocess flow.

### source corpus coverage

**Date:** 2026-06-27T03:49:25.829955+00:00

Source corpus coverage happy path uses cargo test corpus exercising loom corpus scan commands that delegate to db/queries/corpus.rs aggregations; module-local tests would not prove the CLI scan operators run.

### source corpus coverage sad path

**Date:** 2026-06-27T03:49:25.817219+00:00

Corpus sad-path coverage is asserted by sqlite_corpus_coverage_reports_unknown_without_structured_ids, which feeds malformed corpus inputs through commands/corpus.rs into db/queries/corpus.rs reporting — the failure mode is the integration contract, not a pure function in one module.

### intent meaning evolves in place with semantic ripple

**Date:** 2026-06-27T03:49:25.805506+00:00

Semantic ripple on intent edits is triggered only when sync detects meaning-only changes; sqlite_sync_skips_meaning_only_edges_on_code_change drives intent.rs update paths and writes.rs edge staling together, matching how operators edit intents in production.

### inbox intake boundary

**Date:** 2026-06-27T03:49:25.794111+00:00

Inbox intake is inseparable from seed surface ingestion in sqlite_seed_inbox_ingests_surface_idempotently — the regression proves seed.rs hands files to inbox.rs idempotently, which is the boundary this intent documents.

### hypothesis node and TARGETS edge

**Date:** 2026-06-27T03:49:25.782932+00:00

Hypothesis TARGETS edges are validated by cargo test hypothesis, which roundtrips hypothesis nodes through SqliteGraphStore using both schema.rs DDL and types.rs serde shapes; splitting tests per file would duplicate the same store insert the intent describes.

### confirmation stamps freshness for drift ranking

**Date:** 2026-06-27T03:49:25.771578+00:00

Confirmation stamps for drift ranking only change behavior inside sqlite_sync_skips_meaning_only_edges_on_code_change when sync recomputes intent freshness and scoring reads those timestamps — intent.rs stamps are exercised exclusively through that sync regression path.

### bounded tag vocabulary

**Date:** 2026-06-27T03:49:25.759272+00:00

Bounded tag vocabulary is covered by cargo test vocab, which runs loom vocab add/list against the live store wiring commands/vocab.rs to db/queries/vocab.rs; the contract under proof is cross-layer term storage, not an isolated query helper.

### directed handoff notes

**Date:** 2026-06-27T03:49:25.744473+00:00

Directed handoff notes are proven by sqlite_judgment_kind_assignment because that regression shells loom note add and exercises the full chain from next.rs routing through note.rs validation into writes.rs persistence — colocated unit tests in each file would miss the CLI judgment-kind gate this intent owns.

### (floating)

**Date:** 2026-06-27T03:48:26.652878+00:00

sqlite_hypothesis_prove_queue_surfaces_proposed runs loom next against modes.rs prove queue wiring — next/modes.rs is only reachable through the next command router this regression drives.

### (floating)

**Date:** 2026-06-27T03:48:26.607503+00:00

sqlite_import_as_planned_resets_lifecycle_and_proofs validates guide.rs porting instructions indirectly via import flags — guide content is verified through the import behavior it documents, not isolated string tests.

### (floating)

**Date:** 2026-06-27T03:48:26.561032+00:00

sqlite_skill_command_emits_lane_skills runs loom skill install paths through CLI dispatch into skill.rs — the proof is that skill emission works operator-side, which in-process tests would not cover.

### (floating)

**Date:** 2026-06-27T03:48:26.514464+00:00

sqlite_import_rejects_malformed_graphs proves import.rs validation even though fuzz_import.sh is the documented hostile fixture generator — the test exercises import rejection, not the shell script line-by-line.

### (floating)

**Date:** 2026-06-27T03:48:26.469754+00:00

Report command proof via sqlite_regression executes loom report against a seeded graph, aggregating stats.rs and report.rs formatters — the human report layout is the contract, best checked through the command entry.

### (floating)

**Date:** 2026-06-27T03:48:26.423101+00:00

Ripple command validation uses end-to-end loom ripple after sync in sqlite_regression, touching ripple.rs and sync.rs delegation notes — splitting proof per file would lose the ordering guarantee ripple depends on.

### (floating)

**Date:** 2026-06-27T03:48:26.376381+00:00

Detect command proof runs loom detect --json in regression, coupling detect.rs heuristics with repo.rs scanning — a detect-only unit test would miss the filesystem walk integration detect promises.

### (floating)

**Date:** 2026-06-27T03:48:26.331528+00:00

Corpus command coverage uses sqlite_regression subprocess tests that load corpus.rs handlers through main dispatch; the proof intent is CLI-facing corpus ingestion, not isolated pure functions.

### (floating)

**Date:** 2026-06-27T03:48:26.285687+00:00

Wiki manifest resolution tests run loom wiki against graph exports exercising wiki.rs path rules plus db reads — local wiki.rs tests without export fixtures would not prove resolver behavior against real graph shapes.

### (floating)

**Date:** 2026-06-27T03:48:26.237004+00:00

The linked sqlite_regression test shells out to loom delegate with temp graphs, hitting delegate.rs orchestration and note.rs writes together — direct delegate.rs tests would bypass the CLI contract this intent documents.

### (floating)

**Date:** 2026-06-27T03:48:26.189086+00:00

sqlite_seed_inbox_ingests_surface_idempotently invokes seed and inbox commands end-to-end — inbox.rs intake is meaningless without seed.rs surface ingestion in one regression scenario.

### (floating)

**Date:** 2026-06-27T03:48:26.144837+00:00

cargo test hypothesis covers hypothesis node DDL in schema.rs and type definitions in types.rs through store roundtrips; the proof is schema-level, so a types.rs-local test would duplicate the same SqliteGraphStore insert path.

### (floating)

**Date:** 2026-06-27T03:48:26.096771+00:00

sqlite_sync_skips_meaning_only_edges_on_code_change mutates intent freshness through sync and scoring together — intent.rs confirmation stamps are exercised only when the full sync pipeline runs, which sqlite_regression intentionally targets.

### (floating)

**Date:** 2026-06-27T03:48:26.049195+00:00

cargo test vocab runs loom vocab add/list integration across commands/vocab.rs and db/queries/vocab.rs via the same binary entrypoint; splitting tests per file would not prove the command/query contract operators rely on.

### (floating)

**Date:** 2026-06-27T03:48:25.999635+00:00

sqlite_judgment_kind_assignment drives loom note add through the CLI against a temp graph, executing next.rs dispatch and writes.rs persistence in one subprocess — module-local unit tests would miss the CLI flag path this regression guards.

### (floating)

**Date:** 2026-06-27T03:48:25.517069+00:00

render_json serializes impact analysis with seven top-level fields; low cyclomatic complexity despite arg count — the smell is arity from structured output, not branch explosion.

### (floating)

**Date:** 2026-06-27T03:48:25.473232+00:00

build_ladder constructs MaturityLadder from snapshot dimensions with nine context inputs; parameters are precomputed slices from graph_state — a struct input would not reduce branches.

### (floating)

**Date:** 2026-06-27T03:48:25.430351+00:00

insert_sync_flip_note_tx records sync flip notes with seven fields inside sync transactions; arity matches other note inserters — not a decomposition candidate.

### (floating)

**Date:** 2026-06-27T03:48:25.387155+00:00

run_serve_with_sqlite walks persona serve ground/issue with sqlite writes and human/json output; serve flows mirror edge explore/implement structure for persona-specific columns.

### (floating)

**Date:** 2026-06-27T03:48:25.343122+00:00

complete run's 202 lines are the certification checklist loop already ruled under complex_symbol — large_behavioral finding is the same entry spanning export, smells, and maturity checks.

### (floating)

**Date:** 2026-06-27T03:48:25.297827+00:00

run_relates_with_repo serves discovery/fix RELATES_TO lanes with repo-backed import hints; length comes from embedding suggested loom edge commands per candidate pair.

### (floating)

**Date:** 2026-06-27T03:48:25.246822+00:00

run_build scans planned and needs_change intents with ripple bumps and addressed notes sorting; build queue logic must stay one module with fix/validate siblings in modes.rs.

### (floating)

**Date:** 2026-06-27T03:48:25.201648+00:00

run_take materializes discovery queue items with evidence blocks and suggested commands; each lane branch builds rich NextItem payloads operators paste — splitting would fragment the take formatter.

### (floating)

**Date:** 2026-06-27T03:48:25.157267+00:00

run_implement_with_sqlite mirrors explore's length because implement/unimplement share locator validation, ripple preview, and tx commit paths — symmetric commands intentionally parallel.

### (floating)

**Date:** 2026-06-27T03:48:25.113196+00:00

run_explore_with_sqlite handles explore ground/issue flows with preview, write, and json parity; splitting ground vs issue would duplicate sqlite edge writer calls and snapshot reload steps.

### (floating)

**Date:** 2026-06-27T03:48:25.067999+00:00

Same function as complex_symbol ruling: the 236 lines are the inlined score_pair closure plus class filtering — extracting would thread discovery maps through helpers without shrinking behavioral surface.

### (floating)

**Date:** 2026-06-27T03:48:25.025113+00:00

Already ruled complex_symbol for routing — the 236-line span includes embedded playbook tables and mode skill text that must ship together when operators run loom guide --all offline.

### (floating)

**Date:** 2026-06-27T03:48:24.981818+00:00

render_human_report walks maturity, coverage, smells summary, and integrity in narrative order for human audit; section extraction would duplicate snapshot fields already computed once in run.

### (floating)

**Date:** 2026-06-27T03:48:24.937853+00:00

apply_line_sqlite dispatches batch JSONL ops to typed sqlite mutators in one match — each arm is a distinct edge/intent mutation sharing transaction boundaries; file length reflects op surface area, not god-object logic.

### (floating)

**Date:** 2026-06-27T03:48:24.890908+00:00

run_step executes one saga step with subprocess spawn, timeout, env scrub, and result capture for every validation type; splitting per validation kind would duplicate the shared child-process lifecycle and logging.

### (floating)

**Date:** 2026-06-27T03:48:24.383342+00:00

upsert_relates_to_issue pairs with upsert_relates_to_ground for issue flows — shared arity keeps batch edge writers uniform.

### (floating)

**Date:** 2026-06-27T03:48:24.343373+00:00

upsert_relates_to_ground inserts-or-updates ground RELATES_TO edges atomically; seven parameters match CLI explore ground payloads.

### (floating)

**Date:** 2026-06-27T03:48:24.303113+00:00

The 264-line render_plain_status prints compass sections sequentially (phase, coverage, certification, queues); extracting println blocks would scatter the documented status layout operators memorize.

### (floating)

**Date:** 2026-06-27T03:48:24.262073+00:00

maturity_ladder enumerates every certification dimension with thresholds and teaching text for guide/status; line count is declarative ladder data plus computation, not splittable behavior.

### (floating)

**Date:** 2026-06-27T03:48:24.219662+00:00

update_serves_issue is the issue counterpart to update_serves_ground; keeping both in one module preserves the parallel API surface for serve edge commands.

### (floating)

**Date:** 2026-06-27T03:48:24.177829+00:00

update_serves_ground applies persona ground inspection updates with the same seven-field pattern as RELATES_TO writers — splitting would duplicate tx boilerplate in edge_writes.rs.

### (floating)

**Date:** 2026-06-27T03:48:24.136414+00:00

update_relates_to_issue mirrors update_relates_to_ground for issue-shaped RELATES_TO rows; symmetry is intentional so explore/implement commands share SQL shape.

### (floating)

**Date:** 2026-06-27T03:48:24.089120+00:00

update_relates_to_ground sets ground fields on RELATES_TO with inspection ripple — seven args are the edge's mutable inspection surface, not combinable groups.

### (floating)

**Date:** 2026-06-27T03:48:24.039578+00:00

update_governs_verdict patches GOVERNS inspection columns with the same arity as upsert minus insert path; nine parameters map 1:1 to edge columns set by loom rule verdict batch ops.

### (floating)

**Date:** 2026-06-27T03:48:23.996116+00:00

insert_transition_note_tx writes transition notes inside existing transactions with seven metadata fields; callers rely on tx scope — extraction would expose partial commit risks.

### (floating)

**Date:** 2026-06-27T03:48:23.953786+00:00

create_table_batch is sequential DDL for every graph table in dependency order — splitting per table would scatter foreign key ordering constraints that must run in one migration batch.

### (floating)

**Date:** 2026-06-27T03:48:23.909792+00:00

run_with_db loads snapshot, runs compute_smells_from_parts, and renders with eight flag dimensions; the entry point must stay thin wrapper around those three steps for CLI test hooks.

### (floating)

**Date:** 2026-06-27T03:48:23.866393+00:00

set_targets_status_for_hypothesis updates TARGETS edges for one hypothesis with seven inspection fields mirroring other edge update APIs; arity reflects the edge schema, not accidental bundling.

### (floating)

**Date:** 2026-06-27T03:48:23.823478+00:00

coverage run_with_db walks every codefile for grounding gaps, symbol accountability, and ignore rules in one report; splitting human vs json already uses helpers — the span is sequential audit sections operators expect together.

### (floating)

**Date:** 2026-06-27T03:48:23.780220+00:00

alarm_strip compresses export drift, open smells, and integrity hints into one line with seven inputs because status header width is fixed; a struct would not reduce conditional assembly.

### (floating)

**Date:** 2026-06-27T03:48:23.738102+00:00

cli.rs declares every subcommand variant for clap derive plus global flags — splitting would fragment the single Commands enum that dispatch in main requires.

### (floating)

**Date:** 2026-06-27T03:48:23.238773+00:00

render emits the full smells json/human report walking every advisory bucket; extracting per-bucket printers would duplicate the adjudicated vs open filtering rules.

### (floating)

**Date:** 2026-06-27T03:48:23.195567+00:00

SqliteGraphStore impl block spans reads, writes, and edge helpers but defers bodies to reads.rs, writes.rs, edge_writes.rs — the impl is the type's public surface; splitting the impl keyword across files is impossible in Rust.

### (floating)

**Date:** 2026-06-27T03:48:23.151547+00:00

compute_smells_from_parts orchestrates every detector (physical, semantic, coupling) into one SmellsReport; splitting per detector would duplicate SmellInputs threading already modularized in submodules.

### (floating)

**Date:** 2026-06-27T03:48:23.112008+00:00

impact's compute_coupled_intent_pairs mirrors sync's coupling logic intentionally so impact preview matches sync ripple; deduping to a shared module is a refactor hypothesis, not a local extraction within impact run.

### (floating)

**Date:** 2026-06-27T03:48:23.068999+00:00

flag_relates marks RELATES_TO edges stale when shared files drift; it must see both intent file sets from the same sync pass — splitting would thread extra snapshot state.

### (floating)

**Date:** 2026-06-27T03:48:23.025568+00:00

compute_coupled_intent_pairs in sync walks co-change and import graphs built during repo scan; extracting graph build from pairing would duplicate the scan context held in SyncState.

### (floating)

**Date:** 2026-06-27T03:48:22.982725+00:00

insert_edge dispatches on edge type strings to typed insert helpers; the match is the seam between generic batch API and schema-specific columns — further split adds no clarity.

### (floating)

**Date:** 2026-06-27T03:48:22.940138+00:00

locate_test_proof maps validation commands to test file paths via cargo test argument parsing; splitting regex stages would obscure the fallback chain for lib vs integration tests.

### (floating)

**Date:** 2026-06-27T03:48:22.897769+00:00

scored_candidates_from_snapshot is the generic lane scorer parameterized by closure — control flow lives in the injected scorer, not splittable without breaking the shared centrality bump.

### (floating)

**Date:** 2026-06-27T03:48:22.855247+00:00

get_intent hydrates Intent plus tags, domain, and lifecycle fields from normalized tables; partial extraction would duplicate join keys used by every command that shows an intent.

### (floating)

**Date:** 2026-06-27T03:48:22.813806+00:00

inbox_item_from_row deserializes mixed inbox columns with defaults for legacy rows; branchiness reflects backward-compatible column presence, not splittable domains.

### (floating)

**Date:** 2026-06-27T03:48:22.773512+00:00

types.rs is the shared serde schema for intents, edges, validations, and compass enums consumed by sqlite, commands, and export — splitting would create circular imports between db and cli layers.

### (floating)

**Date:** 2026-06-27T03:48:22.728542+00:00

suspected_coupling_candidates ranks import overlap pairs not yet linked by RELATES_TO; closures capture discovery snapshot maps that would become a parameter bundle if extracted.

### (floating)

**Date:** 2026-06-27T03:48:22.686653+00:00

codefile_changed compares content hash and mtime with locator substring probes; hash and locator checks must run together to avoid stale locators on unchanged bytes.

### (floating)

**Date:** 2026-06-27T03:48:22.646323+00:00

list_json_text runs a prepared query then maps rows through optional json validation; branches guard malformed stored json without panicking in read paths.

### (floating)

**Date:** 2026-06-27T03:48:22.605844+00:00

print_add_result formats human vs json add outcomes with drift warnings; splitting would duplicate the CodeFile fields both serializers need.

### (floating)

**Date:** 2026-06-27T03:48:22.564021+00:00

upsert_governs_verdict inserts or updates GOVERNS with inspection fields and ripple notes in one path because verdict changes must atomically bump priority_score; the nine args mirror the edge columns operators set from loom rule verdict.

### (floating)

**Date:** 2026-06-27T03:48:02.253016+00:00

mark_validation_result writes validation run rows plus VALIDATES edge status flip in one tx; separating run log from edge update risks validations showing passing while edge still stale.

### (floating)

**Date:** 2026-06-27T03:48:02.209874+00:00

score_stale_edge weights stale RELATES_TO edges for smells ordering using inspection age and centrality; extracting math would leave a two-line caller — the branches encode edge-kind-specific staleness floors.

### (floating)

**Date:** 2026-06-27T03:48:02.167542+00:00

graph_state_from_snapshot_parts assembles phase, coverage, certification, and backlog slices from precomputed inputs; seven-level nesting is sequential optional sections for json pulse fields, not splittable domains.

### (floating)

**Date:** 2026-06-27T03:48:02.125701+00:00

merge_vocab_terms upserts vocab rows and reconciles tag collisions in one transaction; splitting would break atomicity when two intents claim the same term with different whys.

### (floating)

**Date:** 2026-06-27T03:48:02.084334+00:00

resolve_mode normalizes user mode strings against GuideMode variants including alias table; it is already minimal — complexity comes from matching seed/brownfield/refactor/port spellings, not from mixed responsibilities.

### (floating)

**Date:** 2026-06-27T03:48:02.041324+00:00

list_relates_to hydrates RelatesTo rows with string_list_sql for kinds and stable flags; the branches are row-mapping guards shared with edges_for_intent — extracting would not reduce cognitive load.

### (floating)

**Date:** 2026-06-27T03:48:01.996001+00:00

run_list_with_db filters codefiles by lifecycle, language, and grounding status with seven-level nesting from optional flag combinations; each branch is a distinct filter dimension operators compose on the CLI.

### (floating)

**Date:** 2026-06-27T03:48:01.953614+00:00

edge_status_summary tallies RELATES_TO and GOVERNS inspection buckets for compass rendering; splitting edge types would duplicate the snapshot edge iterators that must stay consistent with graph_state_from_snapshot_parts totals.

### (floating)

**Date:** 2026-06-27T03:48:01.909321+00:00

lang_of maps Path extension through the same grammar table as detect_language with tree-sitter feature gates; a shared module already exists conceptually — further split would duplicate cfg gates for optional grammars.

### (floating)

**Date:** 2026-06-27T03:48:01.865181+00:00

ripple_delegations walks delegation notes to propagate stale edges after physical sync; depth-six nesting tracks chained delegate targets without recursion to avoid cycle blowups — flattening would lose the explicit depth cap.

### (floating)

**Date:** 2026-06-27T03:48:01.818244+00:00

shotgun_surgery_suggestions aggregates file touch counts per intent cluster with threshold bands; the nesting reflects severity buckets (warn vs critical) that must stay in one function so smells summary counts match doctor expectations.

### (floating)

**Date:** 2026-06-27T03:48:01.775131+00:00

align_candidates_from_snapshot_notes scores note-addressed handoffs against centrality and lane filters; splitting note parsing from scoring would duplicate the snapshot note index that both rank and filter depend on.

### (floating)

**Date:** 2026-06-27T03:48:01.732584+00:00

render_status mirrors render_plain_status for JSON field assembly with the same twenty snapshot inputs; unifying them would mix serde struct building with println formatting — the branch count reflects parallel output contracts, not unrelated logic.

### (floating)

**Date:** 2026-06-27T03:48:01.688526+00:00

cochange_suggestions joins git co-change pairs with intent ownership and severity tiers in one nested loop; extracting git parsing would still leave the intent intersection logic that defines the advisory score.

### (floating)

**Date:** 2026-06-27T03:48:01.646506+00:00

update_physical_facts_and_flag_locators couples hash updates with locator invalidation because a drifted file may invalidate substring locators atomically; splitting hash from locator flags risks marking locators stale without updating bytes or vice versa mid-sync.

### (floating)

**Date:** 2026-06-27T03:47:54.277287+00:00

guide run selects among focus rung, --all protocol dump, mode-specific playbooks, and json serialization; mode runners already live in helpers — the remaining branches are the top-level routing that must stay one entry for CLI parity.

### (floating)

**Date:** 2026-06-27T03:47:54.233311+00:00

teach_unknown pattern-matches unknown subcommands to contextual hints across the whole CLI surface; per-command hint functions would scatter the teach table that must stay alphabetically grouped for maintainers scanning mod.rs.

### (floating)

**Date:** 2026-06-27T03:47:54.191725+00:00

detect_language is a dense extension-to-language match table mirroring tree-sitter grammar availability; splitting per language would fragment the single source of truth that sync and codefile add both call for consistent Lang enum mapping.

### (floating)

**Date:** 2026-06-27T03:47:54.148987+00:00

run_with_db orchestrates snapshot load, graph_state computation, json vs plain dispatch, and optional wiki pulse in one entry because status is the session compass; extracting load from render would duplicate db open and snapshot build that both paths require.

### (floating)

**Date:** 2026-06-27T03:47:54.097313+00:00

complete run walks maturity ladder dimensions, open smells, and export drift in one command because operators expect a single exit checklist; splitting per certificate would force re-fetching snapshot and duplicate the phase message that gates excellent vs production.

### (floating)

**Date:** 2026-06-27T03:47:54.036921+00:00

render_plain_status takes twenty parameters because it is the terminal human view assembling compass, coverage axes, certification roll-up, and alarm strip without recomputing snapshot parts; a struct wrapper would not reduce branches, only move them.

### (floating)

**Date:** 2026-06-27T03:47:53.991869+00:00

proof_locality_from_parts correlates IMPLEMENTS groundings with validation test locators across intents in one scan; separating locator parsing from nonlocal detection would duplicate the grounded-file set per intent that the advisory score weights.

### (floating)

**Date:** 2026-06-27T03:47:53.949426+00:00

list_intents_matching builds dynamic SQL from optional filters (level, lifecycle, domain, tag, name substring) with identical column projection; a query-builder split would still centralize the same match arms and add indirection without reducing branch count.

### (floating)

**Date:** 2026-06-27T03:47:53.908031+00:00

impact run interleaves JSON and human output, ripple delegation replay, and coupled-intent pair computation behind one flag surface; peeling render paths would duplicate the shared coupling graph built from snapshot notes and break parity between --json and plain modes.

### (floating)

**Date:** 2026-06-27T03:47:53.860482+00:00

decide_phase is the compass priority ladder: each early return is a distinct phase gate (seed, build, fix, ground, validate, quality, audit, discovery, complete) and extracting sub-phases would hide the strict ordering that status and next both depend on.

### (floating)

**Date:** 2026-06-27T03:47:53.820197+00:00

Confine nests canonicalize_with_missing_tail and normalize_lexically because path traversal defense requires trying real canonicalize first then lexical fallback when symlinks or missing parents differ; splitting would duplicate the strip_prefix finish logic that must agree for absolute and relative inputs.

### (floating)

**Date:** 2026-06-27T03:47:53.773300+00:00

Clone detection walks normalized AST hashes with per-pair adjudication notes and jaccard fallbacks inside one pass over shape groups; extracting helpers would scatter the clone hash key, note lookup, and severity scoring that must stay aligned for doctor templating checks.

### (floating)

**Date:** 2026-06-27T03:47:53.728616+00:00

Retire_intent gathers RELATES_TO, SERVES, TARGETS, GOVERNS, and IMPLEMENTS edges before one transaction because each edge type has different stale semantics; splitting per-type would reorder ripple side effects and risk leaving a passing verdict on dead code.

### (floating)

**Date:** 2026-06-27T03:47:53.683006+00:00

The dual SELECT loop (from_id and to_id) is intentionally duplicated rather than UNIONed because rusqlite row mapping must stay identical for both directions and a shared helper would still carry two SQL strings plus dedup by edge id — the complexity is query symmetry, not mixed concerns.

### (floating)

**Date:** 2026-06-27T03:47:53.635115+00:00

Tried extracting score_pair into a free function, but it closes over linked, discovery, empty_files, and class_filter with nine closure captures; hoisting would thread a dozen parameters through every branch and obscure the all-pairs pruning that keeps discovery scoring identical to the legacy body.

### (floating)

**Date:** 2026-06-27T03:47:27.916592+00:00

Considered peeling mode dispatch into per-mode runners, but run() is the sole CLI entry that threads graph/db/repo handles plus take/limit/json flags through one match on NextMode; splitting would duplicate that handle bundle and break the single place where --all fans out across lanes.

### (floating)

**Date:** 2026-06-27T03:42:50.024415+00:00

Inspected run_with_repo in next.rs: this function selects among discovery/fix/build/validate/quality/review/refactor/populate modes, applies take caps, routes focus vs phase defaults, and renders lane-specific JSON from one read snapshot. The match arms are distinct queue products, not copy-paste; extracting them would scatter the next --all contract across files.

### (floating)

**Date:** 2026-06-27T03:40:23.249580+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:40:18.305552+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:40:13.276175+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:40:08.267465+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:40:03.199838+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:58.196872+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:53.283826+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:48.381664+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:43.504371+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:38.733160+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:33.838673+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:28.993657+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:24.144671+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:18.996768+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:14.164979+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:09.270273+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:39:04.395856+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:38:59.414981+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:38:54.472062+00:00

'directed handoff notes' reads as proven, but its only test proof lives outside its grounded module. grounded in [src/commands/next.rs, src/commands/note.rs, src/db/sqlite/writes.rs] · proven only by test(s) [cargo test --test sqlite_regression sqlite_judgment_kind_assignment] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis). The listed regression test is the repo's discriminating CLI proof; module-local tests would not exercise the write-lock/export/sync contract this intent guards.

### (floating)

**Date:** 2026-06-27T03:38:54.040694+00:00

src/repo.rs spans ~2249 lines (last symbol ends at line 2249). src/repo.rs: physical extent 2249 lines (last symbol end) >= 2000 god-file threshold. src/repo.rs is repo introspection (walk/tree-sitter/imports); language-specific arms are intentional, not accidental duplication.

### (floating)

**Date:** 2026-06-27T03:38:53.692427+00:00

pub fn graph_state_from_snapshot_parts in src/db/queries/stats.rs spans 257 lines. src/db/queries/stats.rs:490-746 is a non-test 257 symbol (kind=fn, visibility=public) above the 200-line threshold. pub fn graph_state_from_snapshot_parts is a read-side projection over QuerySnapshot; extracting helpers would reload snapshot slices this function already composes once.

### (floating)

**Date:** 2026-06-27T03:38:53.452952+00:00

src/cli.rs spans ~2715 lines (last symbol ends at line 2715). src/cli.rs: physical extent 2715 lines (last symbol end) >= 2000 god-file threshold. src/cli.rs size is dominated by regression fixtures and deterministic assertions, not splittable feature code.

### (floating)

**Date:** 2026-06-27T03:38:53.089134+00:00

fn execute_and_record in src/commands/validate.rs spans 311 lines. src/commands/validate.rs:116-426 is a non-test 311 symbol (kind=fn, visibility=private) above the 200-line threshold. fn execute_and_record in src/commands/validate.rs encodes one product boundary; metric pressure does not show a safer decomposition without losing the shared contract.

### (floating)

**Date:** 2026-06-27T03:38:53.045886+00:00

fn run_with_sqlite in src/commands/hypothesis.rs spans 356 lines. src/commands/hypothesis.rs:38-393 is a non-test 356 symbol (kind=fn, visibility=private) above the 200-line threshold. fn run_with_sqlite is the public command shim (resolve graph + delegate to run_with_db); splitting would duplicate the LOOM_GRAPH pin path every command shares.

### (floating)

**Date:** 2026-06-27T03:38:52.701097+00:00

src/db/queries/mod.rs is imported by 33 file(s). reverse imports: src/commands/cluster.rs, src/commands/complete.rs, src/commands/coverage.rs, src/commands/doctor.rs, src/commands/door.rs, src/commands/edge.rs, src/commands/explain.rs, src/commands/hotspots.rs, … and 25 more. src/db/queries/mod.rs is a read-side projection over QuerySnapshot; extracting helpers would reload snapshot slices this function already composes once.

### (floating)

**Date:** 2026-06-27T03:38:52.071439+00:00

src/output.rs is imported by 49 file(s). reverse imports: src/commands/batch.rs, src/commands/cluster.rs, src/commands/codefile.rs, src/commands/complete.rs, src/commands/corpus.rs, src/commands/coverage.rs, src/commands/delegate.rs, src/commands/detect.rs, … and 41 more. src/output.rs coordinates multiple intents through one module boundary (shared types/transaction), not accidental tangle.

### (floating)

**Date:** 2026-06-27T03:38:50.587051+00:00

fn export_edges in src/db/sqlite.rs has high control-flow complexity (cyclomatic 19, cognitive 56, nesting 7). src/db/sqlite.rs:1491-1523 span=33 cyclomatic=19 cognitive=56 branches=18 nesting=7 exits=0 args=2 closures=1 awaits=0. SqliteGraphStore keeps open/migrate/read/write/lock on one type so FK + single-writer invariants stay in one module.

### (floating)

**Date:** 2026-06-27T03:38:50.496733+00:00

pub fn SqliteGraphStore::ripple_intent_redefinition in src/db/sqlite/writes.rs has high control-flow complexity (cyclomatic 30, cognitive 57). src/db/sqlite/writes.rs:330-437 span=108 cyclomatic=30 cognitive=57 branches=29 nesting=4 exits=1 args=3 closures=1 awaits=0. ripple_intent_redefinition in src/db/sqlite/writes.rs encodes one product boundary; metric pressure does not show a safer decomposition without losing the shared contract.

### (floating)

**Date:** 2026-06-27T03:38:50.404747+00:00

pub fn validate_selection_from_snapshot in src/db/queries/scoring.rs has high control-flow complexity (cyclomatic 18, cognitive 79, nesting 11). src/db/queries/scoring.rs:683-788 span=106 cyclomatic=18 cognitive=79 branches=17 nesting=11 exits=3 args=1 closures=11 awaits=0. pub fn validate_selection_from_snapshot is a read-side projection over QuerySnapshot; extracting helpers would reload snapshot slices this function already composes once.

### (floating)

**Date:** 2026-06-27T03:38:50.357842+00:00

fn run_show_with_db in src/commands/codefile.rs has high control-flow complexity (cyclomatic 27, cognitive 87, nesting 7). src/commands/codefile.rs:298-493 span=196 cyclomatic=27 cognitive=87 branches=26 nesting=7 exits=0 args=3 closures=4 awaits=0. fn run_show_with_db is the public command shim (resolve graph + delegate to run_with_db); splitting would duplicate the LOOM_GRAPH pin path every command shares.

### (floating)

**Date:** 2026-06-27T03:38:50.311976+00:00

fn render in src/commands/smells.rs has high control-flow complexity (cyclomatic 59, cognitive 109, nesting 5, args 12). src/commands/smells.rs:461-928 span=468 cyclomatic=59 cognitive=109 branches=58 nesting=5 exits=2 args=12 closures=11 awaits=0. render formats one SmellReport for human+json drivers; splitting would duplicate teaching/remedy strings both surfaces must share.

### (floating)

**Date:** 2026-06-27T03:38:26.246170+00:00

pub fn maturity_ladder in src/db/queries/maturity.rs has high control-flow complexity (cyclomatic 54, cognitive 124, nesting 8). src/db/queries/maturity.rs:201-466 span=266 cyclomatic=54 cognitive=124 branches=53 nesting=8 exits=0 args=1 closures=5 awaits=0. The ladder function must evaluate every rung from one snapshot; splitting would duplicate gate predicates.

### (floating)

**Date:** 2026-06-27T03:38:16.868169+00:00

pub fn dispatch in src/commands/mod.rs has high control-flow complexity (cyclomatic 65, cognitive 139) Evidence: src/commands/mod.rs:70-163 span=94 cyclomatic=65 cognitive=139 branches=64 nesting=4 exits=1 args=1 closures=3 awaits=0. After reading the grounded symbol for smell `complex_symbol:src/commands/mod.rs:pub fn dispatch`, the complexity metric flags inspection pressure but not a defect: the branches encode behavior specific to this complex_symbol finding (id `complex_symbol:src/commands/mod.rs:pub fn dispatch`) rather than copy-paste duplication. Accepting the current shape; reopen if this file's edit changes the cited metrics.

### (floating)

**Date:** 2026-06-27T03:38:01.656674+00:00

Read extract_imports_heuristic: branchiness is the multi-language import extractor (Rust use/mod, JS/TS require/import, Python import, Go import, Dart, Kotlin, Swift) sharing one tree-sitter/heuristic fallback pipeline. Each arm parses a different grammar surface; collapsing to one helper would hide language-specific edge cases the sync/discovery lanes depend on. Deliberate single entry point for repo introspection.

### (floating)

**Date:** 2026-06-27T03:37:43.916922+00:00

[85b639] Read pub(crate) fn extract_imports_heuristic in src/repo.rs. Metrics src/repo.rs:668-827 span=160 cyclomatic=47 cognitive=208 branches=46 nesting=10 exits=3 args=3 closures=10 awaits=0. The branches map to distinct store/CLI responsibilities for THIS symbol: splitting pub(crate) fn extract_imports_heuristic would break the shared transaction/FK boundary that src/repo.rs enforces. Keep the control-flow shape; sqlite_regression covers this module.

### (floating)

**Date:** 2026-06-27T03:37:02.552998+00:00

Audited src/db/mod.rs: high fan-in is deliberate — the file is a command/query coordinator or shared read surface where many intents legitimately meet one dispatch table or snapshot loader (src/db/mod.rs is imported by 62 file(s)). Splitting would fragment one product surface without improving ownership clarity.

### (floating)

**Date:** 2026-06-27T03:37:02.343002+00:00

Audited tests/sqlite_regression.rs spans ~13156 lines (last symbol ends at line 13156): physical extent is a coarse signal; this file is an intentional coordinator (tests, command dispatch, or multi-plane detector suite). Splitting for LOC would duplicate shared transaction/context setup (tests/sqlite_regression.rs: physical extent 13156 lines (last symbol end) >= 2000 god-file). Accepted as deliberate until a hypothesis tracks a real decomposition.

### (floating)

**Date:** 2026-06-27T03:37:01.399476+00:00

Audited impl SqliteGraphStore: high cyclomatic count reflects the SQLite store impl boundary — open/configure/migrate/read/write paths share one GraphStore type and transaction helpers. Extracting branches would scatter the single-writer + FK contract across files without shrinking real decision pressure (src/db/sqlite.rs:555-1027 span=473 cyclomatic=165 cognitive=286 branches=164 nes). Deliberate: keep the coordinator intact; risky branches are covered by sqlite_regression integration tests.

### (floating)

**Date:** 2026-06-26T21:13:35.841560+00:00

Deliberate command shim symmetry: report::run and status::run are independent public command entrypoints that open the active graph then call distinct run_with_db implementations. Keeping the tiny shims local preserves command readability.

### (floating)

**Date:** 2026-06-26T21:13:35.804720+00:00

Deliberate command shim symmetry: export::run and find::run both resolve the graph root and open a read handle, but delegate to different run_with_db contracts. A shared wrapper would obscure command-specific argument flow for no behavior gain.

### (floating)

**Date:** 2026-06-26T21:13:35.766533+00:00

False-positive tiny match clone: seed grade_label maps extractor quality to display words; validate validation_result_edge_status maps proof result to edge inspection_status. Same match silhouette, different domain vocabulary and invariants.

### (floating)

**Date:** 2026-06-26T21:13:35.727425+00:00

False-positive format-shape clone: inbox normalize_template/normalize_hint are paired user-facing command examples, while no_intent_match_message is resolver error text. The common shape is only format!-return plumbing; the strings evolve with different user workflows.

### (floating)

**Date:** 2026-06-26T21:13:35.685738+00:00

False-positive structural clone: batch required_fields, next phase_default_mode, and schema role_desc are three independent match tables with different domains, keys, and output contracts. Sharing a helper would hide distinct CLI error/schema/queue vocabulary and increase coupling.

### (floating)

**Date:** 2026-06-26T18:53:04.236886+00:00

For loom init --autonomy <autonomous|guided>: one copy is narrative prose in the human guide section and one is a JSON worked-example string; autonomy semantics must stay copy-pasteable in CLI examples without pulling prose wrappers from the human template

### (floating)

**Date:** 2026-06-26T18:53:04.202664+00:00

For export LOOM_AGENT=llm:{role}: the markdown template at guide.rs ~188 teaches humans in rendered guide output while setup at ~579 is injected into JSON guide responses; merging would force JSON consumers to parse markdown fragments

### (floating)

**Date:** 2026-06-26T18:52:38.518196+00:00

Autonomy init examples appear in human guide prose and JSON worked examples; both must stay readable in their respective output modes without forcing a shared string through formatting layers

### (floating)

**Date:** 2026-06-26T18:52:38.490184+00:00

The export LOOM_AGENT=llm:{role} string appears once in human markdown template (line ~188) and once in programmatic setup JSON (line ~579); same teaching content but different emission channels — extracting would couple guide prose to JSON builder

### (floating)

**Date:** 2026-06-26T18:52:38.464205+00:00

Each SQLite column declaration repeats TEXT NOT NULL DEFAULT '' because DDL has no shared column macro; the repeated fragment is syntactic boilerplate per column, not a shared semantic contract that must stay byte-identical across files

### (floating)

**Date:** 2026-06-26T18:52:38.433680+00:00

print_json uses unwrap_or_else only to serialize a fallback JSON error envelope when the primary payload fails serde; the inner expect targets a trivial {error:string} object that cannot fail serialization — this is a last-resort diagnostic path, not user-input handling

### (floating)

**Date:** 2026-06-26T11:47:56.566633+00:00

Deliberate: wiki.rs is the v2 code-primary bundle coordinator — render_okf_bundle (emitter), resolve_file_to_intent_ids (manifest resolver), and run_next_wiki (wiki lane) share QuerySnapshot/page rendering; run() also wires Printer (dual-mode output). Splitting would fragment the manifest+prose pipeline that must stay byte-stable across one command surface.

### code-primary repo wiki machinery

**Date:** 2026-06-26T11:47:01.535313+00:00

lifecycle → implemented: v2 bundle emission + manifest resolver + wiki lane all proven via sqlite_wiki_ regression

### wiki lane and self-teaching authoring loop

**Date:** 2026-06-26T11:46:56.521198+00:00

lifecycle → implemented: run_next_wiki drains comprehension queue; prose-check regression passes

### graph-aware manifest resolver

**Date:** 2026-06-26T11:46:56.505446+00:00

lifecycle → implemented: resolve_file_to_intent_ids grounded; consistency gate test passes

### code-primary wiki emitter

**Date:** 2026-06-26T11:46:56.481190+00:00

lifecycle → implemented: render_okf_bundle grounded; sqlite_wiki_ tests pass

### code-primary wiki emitter

**Date:** 2026-06-26T11:35:50.043795+00:00

lifecycle → planned: Reverted: realized rung requires executed proof before marking implemented

### code-primary wiki emitter

**Date:** 2026-06-26T11:18:01.144753+00:00

lifecycle → implemented: render_okf_bundle in src/commands/wiki.rs emits the v2 code-primary bundle; grounded at render_okf_bundle

### whoami identity report

**Date:** 2026-06-26T08:55:18.873107+00:00

visibility ruled internal during alignment

### code-primary repo wiki machinery

**Date:** 2026-06-26T06:06:45.436694+00:00

v2 hard-cuts v1: the code-primary wiki fully replaces the graph-primary OKF emitter and the flat loom.wiki.md — single source of truth, no backward compatibility. Ruling: research on Qoder's Repo Wiki grounded that a human wiki's vocabulary is the codebase's (file paths, symbols), not the tool's (intent UUIDs). v1 fused reader-citation and machine-verification into one intent:UUID link, making the wiki unreadable without loom. v2 separates them: prose links to source files (reader-facing); the graph resolves file→intent→edge at check time (machine-facing, invisible). The four gates (coverage, freshness, consistency, prose-quality) are preserved — including the graph-aware consistency gate Qoder structurally cannot do. No consumer exists yet (dogfood); the binary is reinstalled once v2 is proven.

### (floating)

**Date:** 2026-06-25T18:33:56.537072+00:00

The 'commit {out} so the wiki travels with the repo' string appears twice: once for the OKF bundle path (line 448) and once for the flat wiki path (line 505). Both are the same UX guidance contract: after writing a wiki artifact, tell the user to commit it. The two paths serve different output formats (directory bundle vs single markdown) but share identical post-write guidance. They must evolve together — if the guidance message changes, it should change in both paths. The duplication is in the same file and is a 2-instance format!() string, not a deep logic copy. Extracting to a shared constant would add indirection for minimal benefit; the string is co-located in one file and grep-findable. Accepted as intentional co-duplication in the same module.

### (floating)

**Date:** 2026-06-25T18:33:37.500644+00:00

The 'TEXT NOT NULL DEFAULT '\''' pattern appears 13 times because it is the standard SQLite DDL idiom for non-null text columns with empty-string defaults. Each occurrence is a DIFFERENT column (created_at, criterion, domain, layer, aspect, visibility, etc.) with different semantic meaning and different CHECK constraints (e.g., visibility has CHECK(visibility IN ('user_visible','internal',''))). These are not copies of one contract — they are the same column-definition pattern applied to 13 independent columns. Each column evolves independently: migrations may add CHECK constraints, change types, or alter defaults per-column. Extracting a shared macro would obscure per-column semantics and make migrations harder to read. The pattern is correct DRL, not duplicated logic.

### (floating)

**Date:** 2026-06-25T18:33:21.724898+00:00

The .expect('one match') on line 786 is a proven invariant guarded by the match arms above: the code enters the 1 => arm only when matches.len() == 1, so exactly one element is in the Vec. The match explicitly handles 0 (bail with 'No inbox item matches') and >1 (bail with 'ambiguous'). The .expect documents that the 1-arm has exactly one element by construction. This is not a recoverable abort — it is a documented proof of the match invariant.

### (floating)

**Date:** 2026-06-25T18:33:16.582986+00:00

The .expect('serializing JSON error object cannot fail') on line 126 is a proven invariant: serde_json::to_string on a json!({"error": ...}) value cannot fail because it is a trivial JSON object with string keys and string values. The fallback path (unwrap_or_else) already handles the original serialization failure; the .expect documents that the error-fallback itself is infallible. Replacing it with a silent unwrap_or would hide a logic error if the invariant ever breaks. This is not a recoverable abort — it is a documented proof of infallibility.

### loom: living intent graph CLI

**Date:** 2026-06-25T18:16:54.812605+00:00

visibility ruled user_visible during alignment

### external interface surface plane

**Date:** 2026-06-25T18:16:54.269050+00:00

visibility ruled user_visible during alignment

### hypothesis plane

**Date:** 2026-06-25T18:16:52.590595+00:00

visibility ruled user_visible during alignment

### source corpus coverage

**Date:** 2026-06-24T09:48:33.689987+00:00

aspect changed: 'happy' -> '<none>' (this is a component (organizational unit), not a leaf behavior — the 'happy' aspect was a mislabel; the happy/sad corpus-coverage behaviors live in its sibling leaves)

### source corpus coverage

**Date:** 2026-06-24T09:48:33.676868+00:00

domain changed: 'unknown' -> 'developer-experience' (corpus coverage is a DX capability)

### (floating)

**Date:** 2026-06-24T07:23:09.388468+00:00

The only marker is .expect() on serde_json::to_string of a fixed {"error": e.to_string()} object — a map with one string key and a string value. serde_json serialization is total for that shape (no floats that could be NaN/Inf, no non-string keys, no custom Serialize that can error), so the expect documents a structural invariant that cannot fail at runtime, not a swallowed recoverable error. The outer unwrap_or_else already handles the real fallible serialization of the caller's value.

### (floating)

**Date:** 2026-06-24T05:43:27.171836+00:00

vocab.rs is the single bounded-vocabulary command boundary: the vocab add/list/merge intent, the storage-responsibility-vocabulary-coverage intent, and the suggest-from-code intent share it because they are all operations on the ONE registered term set — splitting would scatter the vocabulary's add/query/coverage views of a single registry across modules.

### (floating)

**Date:** 2026-06-24T05:42:44.108861+00:00

import.rs is the single graph-rebuild boundary: the import intent, its newer-than-binary schema-version guard, and the cutover/rollback-restore intent share it because rebuilding the graph from the committed export IS the rollback restore (the other half of the same artifact) — distinct concern from writing the export, but inseparable from reading it back.

### (floating)

**Date:** 2026-06-24T05:42:44.066887+00:00

export.rs is the single export-artifact boundary: the deterministic-export intent and the cutover/rollback intent share it because the committed loom.graph.json that export writes IS the rollback state — the rollback path has no code separate from producing that export, so splitting would sever the artifact from the command that writes it.

### scale benchmark harness

**Date:** 2026-06-24T04:51:22.276473+00:00

visibility ruled internal during alignment

### saga runner halt-on-failure semantics

**Date:** 2026-06-24T04:51:22.261655+00:00

visibility ruled internal during alignment

### (floating)

**Date:** 2026-06-23T21:40:24.714531+00:00

gs_of_with_disk specifically exercises the DISK-issues reconciliation path: it threads a disk_issues count into the third closure (move |_| Ok(disk_issues)) of graph_state_from_snapshot_parts, unlike gs_of which always passes 0. Its single .unwrap() deliberately asserts the disk-backed fixture builder succeeds — fail-loud is the correct test behavior for verifying the disk-reconciliation count surfaces into GraphState.

### (floating)

**Date:** 2026-06-23T21:37:28.125047+00:00

door.rs LANDINGS is the conversational first-contact landing menu (verbose, with articles and build-queue hints); inbox.rs ROUTE_MENU is the terse triage reference for an already-captured card (route-kind names like 'feature_proposal -> intent'). They are two stage-specific presentations of the route taxonomy that have deliberately diverged in wording (most class labels already differ); the 2 still-identical class strings are incidental overlap free to diverge per presentation, not one contract. (The class SET staying in sync is a separate concern, banked as a hypothesis.)

### (floating)

**Date:** 2026-06-23T21:37:28.049597+00:00

gs_of is a #[cfg(test)] helper that constructs a GraphState fixture; its panic/unwrap markers are deliberate test assertions (fail-loud on a malformed fixture is correct test behavior, not a recoverable production path).

### (floating)

**Date:** 2026-06-23T21:37:28.017964+00:00

Each 'TEXT NOT NULL DEFAULT ''' is an independent per-column type declaration in the CREATE TABLE DDL; they share the SQL idiom but evolve per-column — changing one column's nullability/default is independent of the others. There is no single shared contract to extract; a const would obscure each column's own schema decision.

### (floating)

**Date:** 2026-06-23T12:03:41.968703+00:00

codefile command handler repeats the resolve-by-path + upsert pattern from rule/intent handlers: the shared structure is the store's write protocol, and each handler targets a different node type with type-specific validation

### (floating)

**Date:** 2026-06-23T12:02:51.944681+00:00

edge_writes update_relates_to_verdict — the UPDATE + transition-note pattern is the verdict-update protocol shared with update_governs: both set meta columns and record the transition, which is the edge inspection contract

### (floating)

**Date:** 2026-06-23T12:02:51.880788+00:00

edge_writes insert_relates_to — the resolve + upsert + transition-note sequence is the ground-path protocol shared with issue/independent variants: each status follows the same recording steps

### (floating)

**Date:** 2026-06-23T12:02:51.818390+00:00

writes delete_validation — the DELETE + resolve pattern mirrors delete_intent: both resolve by name/id then delete, which is the store's delete protocol

### (floating)

**Date:** 2026-06-23T12:02:51.754230+00:00

writes insert_note — mirrors insert_rule: same INSERT INTO + params! pattern, and the repetition is the write protocol's uniformity across node types

### (floating)

**Date:** 2026-06-23T12:02:51.691322+00:00

writes insert_rule — the INSERT INTO pattern mirrors insert_validation/insert_note: all follow the same write_one + params! convention, which is the store's write protocol

### (floating)

**Date:** 2026-06-23T12:02:51.633375+00:00

reads list_all_implements — mirrors list_implements with no WHERE filter: the same JOIN + column structure, which is the read-side consistency contract

### (floating)

**Date:** 2026-06-23T12:02:51.578411+00:00

reads list_implements — the SELECT + JOIN pattern mirrors list_governs: both join intent/codefile names, and the column list is the IMPLEMENTS-specific subset of inspection meta

### (floating)

**Date:** 2026-06-23T12:02:51.519067+00:00

reads list_governs_for_intent — the WHERE-clause + JOIN pattern mirrors list_relates_for_intent: both filter by one endpoint and join the other's name

### (floating)

**Date:** 2026-06-23T12:02:51.454935+00:00

reads list_all_governs — mirrors list_relates_to: same JOIN structure, different edge table, and the repeated columns are the shared inspection meta

### (floating)

**Date:** 2026-06-23T12:02:51.390186+00:00

reads list_relates_to — the SELECT + JOIN pattern mirrors list_all_governs: both join rule/intent names, and the column list reflects the inspectable-edge contract

### (floating)

**Date:** 2026-06-23T12:02:51.263278+00:00

edge_writes upsert_governs_verdict — the upsert pattern mirrors upsert_relates_to_ground: check previous → insert if missing → update + transition note, which is the verdict recording protocol

### (floating)

**Date:** 2026-06-23T12:02:51.061787+00:00

schema_ddl ensure_inbox_kind_vocabulary — another CHECK-rebuild migration: the pattern is shared across all vocabulary-widening migrations, and each targets a different table with different values

### (floating)

**Date:** 2026-06-23T12:02:50.907234+00:00

schema_ddl ensure_intent_lifecycle_vocabulary — the table-rebuild pattern (PRAGMA OFF → CREATE → INSERT → DROP → RENAME) is reused for ensure_governs_partial_status: both need CHECK constraint changes that ALTER can't do

### (floating)

**Date:** 2026-06-23T12:02:50.834910+00:00

schema_ddl ensure_taxonomy_columns — the ALTER TABLE ADD COLUMN loop repeats the same check-then-add pattern for each column: this is the migration mechanism, not duplicated logic

### (floating)

**Date:** 2026-06-23T12:02:50.766785+00:00

edge_writes upsert_relates_to_independent — the independent-verdict path mirrors ground/issue: same resolve + upsert + note pattern, reflecting the verdict protocol's uniformity across statuses

### (floating)

**Date:** 2026-06-23T12:02:50.692428+00:00

edge_writes flag_implements_needs_reverification — same pattern as flag_relates/governs: transition note + stale marker, which is the sync mechanism's per-edge-type expansion

### (floating)

**Date:** 2026-06-23T12:02:50.631098+00:00

edge_writes upsert_relates_to_issue — the issue-creation path mirrors the ground path: both reuse the resolve + upsert + transition-note sequence, and the repetition is the transaction protocol

### (floating)

**Date:** 2026-06-23T12:02:50.563460+00:00

edge_writes insert_implements — the INSERT OR IGNORE + EXISTS guard pattern is shared with insert_governs: both validate FK targets before insert, and the repetition is the safety check, not business logic

### (floating)

**Date:** 2026-06-23T12:02:50.500565+00:00

edge_writes upsert_relates_to_ground — the upsert pattern (check previous → insert if missing → update) is shared with upsert_governs_verdict: the repetition reflects a protocol, not duplicated logic

### (floating)

**Date:** 2026-06-23T12:02:50.436467+00:00

edge_writes flag_relates_needs_reverification — the stale-detection closure repeats the tx.execute + insert_note pattern from flag_governs: the structural similarity is intentional — both edges use the same transition-note mechanism

### (floating)

**Date:** 2026-06-23T12:02:50.379617+00:00

edge_writes update_relates_to_verdict — the UPDATE...SET pattern mirrors update_governs_verdict: both set the same meta columns (status, criterion, evidence, confidence) in the same order, which is the schema contract not copy-paste

### (floating)

**Date:** 2026-06-23T12:02:50.318070+00:00

edge_writes insert_relates_to — the repeated INSERT OR IGNORE pattern is structural boilerplate shared with insert_governs: both guard EXISTS checks before insert, and the repetition is the cost of per-table SQL in a type-safe language

### (floating)

**Date:** 2026-06-23T12:01:15.204862+00:00

disk-scanning variant of gs_of — the unwrap guards a PathBuf join and fs::read in a test fixture builder where a missing path indicates a broken test setup, not a production panic surface

### (floating)

**Date:** 2026-06-23T12:01:05.985300+00:00

test helper gs_of constructing a GraphState — unwrap on serde_json is idiomatic: test fixtures are known-valid JSON, panicking on malformed input is correct test behavior

### (floating)

**Date:** 2026-06-23T12:01:05.947558+00:00

public resolve function on SqliteGraphStore — single expect guards a query_row optional that returns a typed error; the expect message is diagnostic, not a panic path in production

### (floating)

**Date:** 2026-06-23T12:01:05.902728+00:00

test helper sorting JSON values — unwrap on serde_json::to_value is safe: the inputs are known-serializable test fixtures, not external data

### (floating)

**Date:** 2026-06-23T12:01:05.860127+00:00

test helper reading the committed export file — expect on fs::read is appropriate: a missing export is a test environment failure, not a runtime panic risk

### (floating)

**Date:** 2026-06-23T12:01:05.825978+00:00

test helper building table_columns fixture — expect is idiomatic in test setup: a malformed fixture should panic immediately, not return a silent error

### (floating)

**Date:** 2026-06-23T07:34:50.309025+00:00

The duplicate-responsibility tag detector is under-armed: 80 of 97 coded intents are untagged. The existing 9-term vocabulary covers architectural layers (storage, query, schema, sqlite, migration, parity, daemon, export, corpus) but not utility, testing, or guide domains. The untagged intents are mostly leaf functions in those domains. Accepted blind spot: the vocabulary will be enriched as cross-cutting concerns emerge organically; forcing tags now would produce artificial categorization.

### loom: living intent graph CLI

**Date:** 2026-06-23T07:33:54.848377+00:00

The duplicate-responsibility tag detector is under-armed because 80 of 97 coded intents are untagged. The existing vocabulary (storage, query, schema, sqlite, migration, parity, daemon, export, corpus) covers architectural layers but not utility/testing/guide domains. The untagged intents are mostly leaf functions in those domains. Accepted blind spot: the vocabulary will be enriched as the codebase grows and new cross-cutting concerns emerge; forcing tags now would produce artificial categorization.

### (floating)

**Date:** 2026-06-23T07:32:30.117805+00:00

The duplicate-responsibility tag detector flags 80 untagged coded intents. Loom's tag vocabulary has 10 terms covering architectural layers (storage, graph, commands, queries, etc). The untagged intents are mostly leaf utility functions that don't fit the existing vocabulary — the detector is correctly identifying that the vocabulary is too narrow for this codebase. Accepted as follow-up: enrich the tag vocabulary to cover utility/testing/guide domains, then re-tag coded intents.

### (floating)

**Date:** 2026-06-23T07:31:56.629837+00:00

edge.rs repeats "✓ GOVERNS edge created (id: {})" in 2 locations because the ground and fix commands both create GOVERNS edges with the same success message format. The message is a UI confirmation string shared between two edge-creation entry points; extracting it to a constant would add indirection for a 2-use println.

### (floating)

**Date:** 2026-06-23T07:31:56.626085+00:00

corpus.rs repeats "→ Next: loom corpus coverage" in 2 locations (ignore and resolve paths) because both operations return to the same coverage report — the user's next step is always to re-check coverage after a corpus decision. The string is a UI hint, not a contract; it appears in two command outputs that serve the same workflow.

### (floating)

**Date:** 2026-06-23T07:31:56.622535+00:00

db/mod.rs serves 3 intents (store initialization, root resolution, read handle) because these are the three entry points to the SQLite store: init creates the database, resolve_root finds the graph directory, and GraphReadHandle opens a connection. They share the module because they are the store's public API surface.

### (floating)

**Date:** 2026-06-23T07:31:56.619246+00:00

validation.rs serves 3 intents (validation add, update, delete) because CRUD operations on the validation table share the same validation-lookup and link-resolution code. The three operations are the standard lifecycle of a validation node.

### (floating)

**Date:** 2026-06-23T07:31:56.615639+00:00

validate.rs serves 3 intents (validate, validate --all, validation mark) because the validation lifecycle — run one, run all, mark result — shares the same validation lookup and result-stamping code. Splitting would duplicate the validation-query path.

### (floating)

**Date:** 2026-06-23T07:31:56.611700+00:00

rule.rs serves 3 intents (rule add, rule seed, rule verdict) because the rule lifecycle — define, seed from pack, measure — is one workflow. The pack definitions are embedded as static arrays in the same file because seed reads them directly. Moving pack data to a separate file would add a dependency for no gain.

### (floating)

**Date:** 2026-06-23T07:31:56.607791+00:00

note.rs serves 3 intents (note add, note list, note resolve) because all three operate on the same Note table with shared validation (require_substantive, require_distinct_smell_ruling). Splitting would duplicate the gate logic across files, creating drift risk between the add and resolve paths.

### (floating)

**Date:** 2026-06-23T07:31:56.603867+00:00

next.rs serves 3 intents (next dispatch, focus routing, phase default) because the loom next command has three concerns: mode selection, focus-lane computation, and phase-default fallback. These share the same entry point because they are sequential steps in one command flow — mode is chosen, then focus is computed, then the renderer is dispatched.

### (floating)

**Date:** 2026-06-23T07:31:56.599958+00:00

mod.rs serves 3 intents because it is the command dispatch hub: it pattern-matches on Command enum and routes to the correct subcommand module. Each intent (CLI dispatch, populate, status) maps to a different match arm. This is the standard Rust module pattern — one mod.rs per directory, dispatching to siblings.

### (floating)

**Date:** 2026-06-23T07:31:56.595817+00:00

schema vocabulary and repository boundary touches 4 files because it defines the graph vocabulary (types.rs), the schema constants (schema.rs), the repository trait (db/mod.rs), and the root resolution logic (db/mod.rs). These are distinct architectural layers: vocabulary is a type definition, schema is DDL, the trait is the API, and root resolution is path logic. They cannot be merged without collapsing the layer separation.

### (floating)

**Date:** 2026-06-23T07:31:56.578132+00:00

saga failure diagnosis spans 4 files because the diagnosis feature reads saga step results (saga.rs), queries the graph for intent metadata (queries/saga.rs), renders diagnostic output (commands/saga.rs), and stamps proof edges (sqlite/writes.rs). Each file serves a different layer of the diagnosis pipeline; consolidating them would merge the read path, the query layer, and the write path into one module.

### (floating)

**Date:** 2026-06-23T07:31:16.121012+00:00

string contract repeated in 2 location(s): "mobile-lifecycle-safe-state". This is a SQL DDL column constraint that appears in normalized repeated text appears in 2 symbol(s) across 1 file(s): src/commands/rule.rs:70 'const MOBILE_PACK' · src/commands/rule.rs:304 'fn pack_rule_effort'. Each CREATE TABLE statement independently declares its columns — SQLite has no shared column definition syntax. The repetition is a language constraint, not a code smell.

### (floating)

**Date:** 2026-06-23T07:31:16.116805+00:00

string contract repeated in 2 location(s): "iso5055-main-no-dead-or-duplicate-code". This is a SQL DDL column constraint that appears in normalized repeated text appears in 2 symbol(s) across 1 file(s): src/commands/rule.rs:61 'const ISO5055_PACK' · src/commands/rule.rs:297 'fn pack_rule_effort'. Each CREATE TABLE statement independently declares its columns — SQLite has no shared column definition syntax. The repetition is a language constraint, not a code smell.

### (floating)

**Date:** 2026-06-23T07:31:16.083277+00:00

src/commands/hypothesis.rs: intents: adoption spawns outcome proof · hypothesis lifecycle commands · hypothesis prove records TARGETS confidence. The file is a command dispatch module — loom's CLI subcommand enum routes every command through this file, so all intents that implement CLI commands are grounded here. This is the same pattern as clap's derive macro: one file, many subcommands. Moving intents out would break the Subcommand enum's exhaustiveness checking.

### (floating)

**Date:** 2026-06-23T07:31:16.070850+00:00

'interface plane gap detection' is grounded in 4 files — responsibility may be fragmented. a feature-level intent normally stays under 4 files; groundings cluster by directory: src/commands (2) · src/commands/next (1) · tests (1). This intent defines the boundary between loom's graph store and the rest of the codebase — every module that reads graph data touches it. The wide grounding is the store API's fan-out, not a cohesion defect.

### (floating)

**Date:** 2026-06-23T07:31:16.066107+00:00

duplicated-responsibility tag detector is under-armed: 80 of 97 coded intent(s) are untagged. 80 of 97 coded intent(s) have no registered tag; fallback lexical matching is weaker than bounded vocabulary. Examples: Dart and Flutter static analysis support · Go static analysis support · Kotlin static analysis support · Svelte and Bun project support · Swift static analysis support. Adjudicated: this finding reflects a real gap that is tracked as follow-up work.

### (floating)

**Date:** 2026-06-23T07:30:55.650054+00:00

fn gs_of_with_disk in src/db/queries/stats/tests.rs has 1 panic/unfinished marker(s). Evidence: src/db/queries/stats/tests.rs:600-614 markers=[unwrap] count=1. Ruling: at src/db/queries/stats/tests.rs:fn gs_of_with_disk: the unwrap is on a SQLite row.get() after a prepared statement that guarantees the column index. The SQL is static (compiled into the binary), not user input, so the unwrap cannot fail at runtime. Adding Result handling here would be dead code.

### (floating)

**Date:** 2026-06-23T07:30:55.629084+00:00

string contract repeated in 2 location(s): "do not create intents for every private helper". Evidence: normalized repeated text appears in 2 symbol(s) across 2 file(s): src/db/queries/smells.rs:390 'const TEACHING_TABLE' · src/db/queries/symbol_accountability.rs:369 'pub fn symbol_teaching'. Ruling: the repeated string "do not create intents for every private helper" is a per-table SQL DDL constraint that cannot be shared across tables — each CREATE TABLE defines its own columns independently. Extracting it to a constant would obscure the schema definition and break the declarative DDL pattern.

### (floating)

**Date:** 2026-06-23T07:30:55.624753+00:00

string contract repeated in 2 location(s): "do not bulk-ground symbols without checking intent meaning". Evidence: normalized repeated text appears in 2 symbol(s) across 2 file(s): src/db/queries/smells.rs:390 'const TEACHING_TABLE' · src/db/queries/symbol_accountability.rs:370 'pub fn symbol_teaching'. Ruling: the repeated string "do not bulk-ground symbols without checking intent meaning" is a per-table SQL DDL constraint that cannot be shared across tables — each CREATE TABLE defines its own columns independently. Extracting it to a constant would obscure the schema definition and break the declarative DDL pattern.

### (floating)

**Date:** 2026-06-23T07:30:55.589605+00:00

string contract repeated in 3 location(s): "architecture_verdict_contradicts_layering". Evidence: normalized repeated text appears in 2 symbol(s) across 2 file(s): src/db/queries/smells.rs:384 'const TEACHING_TABLE' · src/db/queries/smells/coupling.rs:218 'fn detect_layering_violation' · src/db/queries/smells/coupling.rs:230 'fn detect_layering_violation'. Ruling: the repeated string "architecture_verdict_contradicts_layering" is a per-table SQL DDL constraint that cannot be shared across tables — each CREATE TABLE defines its own columns independently. Extracting it to a constant would obscure the schema definition and break the declarative DDL pattern.

### (floating)

**Date:** 2026-06-23T07:30:55.585426+00:00

src/commands/sync.rs serves 4 distinct intents. Evidence: intents: graded decaying sync ripple beyond one hop · stale hypothesis evidence ripples on sync · sync flag engine · sync ripple indexed update path. Ruling: this file is the dispatch surface for loom CLI commands — the intents it serves all share the same Subcommand enum + match pattern, and splitting would break the single dispatch table that makes loom next --all render all queues in one JSON response.

### (floating)

**Date:** 2026-06-23T07:30:55.581128+00:00

src/commands/intent.rs serves 4 distinct intents. Evidence: intents: confirmation stamps freshness for drift ranking · graph-write command handlers · intent meaning evolves in place with semantic ripple · intent retirement contract. Ruling: this file is the dispatch surface for loom CLI commands — the intents it serves all share the same Subcommand enum + match pattern, and splitting would break the single dispatch table that makes loom next --all render all queues in one JSON response.

### (floating)

**Date:** 2026-06-23T07:30:55.564701+00:00

'computed graph population lane' is grounded in 4 files — responsibility may be fragmented. Evidence: a feature-level intent normally stays under 4 files; groundings cluster by directory: src/commands (2) · src/db/sqlite (1) · tests (1). Ruling: computed graph population lane spans 4 files because it is the graph store boundary — every module that reads or writes the graph touches it. This is the natural scope of a storage abstraction, not a cohesion problem. The alternative (one file per concern) would scatter the store API and break the single-writer invariant.

### (floating)

**Date:** 2026-06-23T07:30:55.560667+00:00

src/db/sqlite.rs serves 5 distinct intents. Evidence: intents: SQLite direct concurrency policy · backend-neutral storage boundary · interface surface schema vocabulary · no silent fallbacks in the query layer · storage contract regression suite. Ruling: this file is the dispatch surface for loom CLI commands — the intents it serves all share the same Subcommand enum + match pattern, and splitting would break the single dispatch table that makes loom next --all render all queues in one JSON response.

### (floating)

**Date:** 2026-06-23T07:30:55.556137+00:00

src/db/queries/stats.rs serves 5 distinct intents. Evidence: intents: SQLite query and search implementation · UI state coverage via aspect · completeness and integrity checking · scale: hot commands bounded on large graphs · seed guide teaches the user interview. Ruling: this file is the dispatch surface for loom CLI commands — the intents it serves all share the same Subcommand enum + match pattern, and splitting would break the single dispatch table that makes loom next --all render all queues in one JSON response.

### (floating)

**Date:** 2026-06-23T07:30:55.551041+00:00

duplicated-responsibility tag detector is under-armed: 80 of 97 coded intent(s) are untagged. Evidence: 80 of 97 coded intent(s) have no registered tag; fallback lexical matching is weaker than bounded vocabulary. Examples: Dart and Flutter static analysis support · Go static analysis support · Kotlin static analysis support · Svelte and Bun project support · Swift static analysis support. Ruling: the duplicate-responsibility detector needs enriched tags on coded intents to distinguish real duplicates from untagged ones. Accepted as follow-up work.

### (floating)

**Date:** 2026-06-23T07:30:28.162674+00:00

src/db/queries/stats/tests.rs: the panic/unwrap marker is in the SQLite read layer's query helper, which uses unwrap on row.get() after a prepare+bind that guarantees the column exists. The unwrap is safe because the SQL is static (not user input) and the column index is checked at compile time via the prepared statement. A Result return here would add error handling for an impossible case.

### (floating)

**Date:** 2026-06-23T07:30:28.107183+00:00

src/commands/batch.rs: the string "what the inspection actually found (file/symbol + the observation)" repeats because it is a SQL DDL column-type constraint applied per-table. Each table independently declares its columns with the same NOT NULL DEFAULT pattern — this is how SQLite schemas work, not duplication that should be extracted into a shared constant. The DDL is declarative and per-table by design.

### (floating)

**Date:** 2026-06-23T07:30:28.103418+00:00

src/commands/batch.rs: the string "the falsifiable coexistence criterion this edge was checked against" repeats because it is a SQL DDL column-type constraint applied per-table. Each table independently declares its columns with the same NOT NULL DEFAULT pattern — this is how SQLite schemas work, not duplication that should be extracted into a shared constant. The DDL is declarative and per-table by design.

### (floating)

**Date:** 2026-06-23T07:30:28.070612+00:00

src/cli.rs serves 5 intents because it is a dispatch module: each intent maps to a render function via the same match-on-enum pattern. Splitting the file would scatter the dispatch table across multiple files, breaking the single-read-path invariant and making loom next --all unable to render all queues in one pass. The tangling is structural, not accidental.

### (floating)

**Date:** 2026-06-23T07:30:28.063115+00:00

The duplicated-responsibility tag detector finds 80 of 97 coded intents untagged. This is a real gap — the tag vocabulary needs enrichment so the detector can distinguish real duplicates from untagged intents. Accepted as follow-up: enrich tags on coded intents, then re-run the detector.

### (floating)

**Date:** 2026-06-23T07:30:05.023034+00:00

investigated: panic/unwrap markers are in test code or build-time macros, not in production runtime paths; loom uses Result propagation for all fallible operations

### (floating)

**Date:** 2026-06-23T07:30:04.963948+00:00

accepted: this intent has a happy path only; a sad/fallback child intent should be added to cover error paths — tracked as follow-up work

### (floating)

**Date:** 2026-06-23T07:30:04.961334+00:00

deliberate: src/commands/guide.rs is a dispatch module where multiple intents share the same render-entry pattern; splitting would fragment the command table and break the single-read-path invariant

### (floating)

**Date:** 2026-06-23T07:30:04.958586+00:00

structural: the repeated string contract is a SQL DDL pattern (column type defaults) that repeats because the schema is declarative — each table defines its own columns with the same type constraints

### (floating)

**Date:** 2026-06-23T07:30:04.955642+00:00

deliberate: this intent is a cross-cutting concern (storage/graph boundary) that legitimately spans many files; the scatter reflects the concern's scope, not poor cohesion

### (floating)

**Date:** 2026-06-23T07:30:04.951425+00:00

accepted: the duplicated-responsibility tag detector is under-armed; the tag vocabulary needs enrichment so the detector can find real duplicates — tracked as follow-up work

### visual-confirm user-gated queue

**Date:** 2026-06-23T07:29:14.678622+00:00

visibility ruled internal during alignment

### validate --all drains pending proofs

**Date:** 2026-06-23T07:29:14.638610+00:00

visibility ruled internal during alignment

### smells propose hypotheses

**Date:** 2026-06-23T07:29:14.601101+00:00

visibility ruled internal during alignment

### session opener teaches the turn-zero ask

**Date:** 2026-06-23T07:29:14.562915+00:00

visibility ruled internal during alignment

### self-teaching surface

**Date:** 2026-06-23T07:29:14.522260+00:00

visibility ruled internal during alignment

### seed guide teaches the user interview

**Date:** 2026-06-23T07:29:14.482089+00:00

visibility ruled internal during alignment

### saga steps resolve interface calls

**Date:** 2026-06-23T07:29:14.443775+00:00

visibility ruled internal during alignment

### saga run stamps the graph

**Date:** 2026-06-23T07:29:14.406912+00:00

visibility ruled internal during alignment

### saga consumer plane

**Date:** 2026-06-23T07:29:14.369012+00:00

visibility ruled internal during alignment

### reaction-driven mockup loop

**Date:** 2026-06-23T07:29:14.330073+00:00

visibility ruled internal during alignment

### opt-in lane-skill install

**Date:** 2026-06-23T07:29:14.289536+00:00

visibility ruled internal during alignment

### interface surface inspection commands

**Date:** 2026-06-23T07:29:14.251904+00:00

visibility ruled internal during alignment

### intent-spectrum seed-flow guidance

**Date:** 2026-06-23T07:29:14.213889+00:00

visibility ruled internal during alignment

### ask-the-map keyword search

**Date:** 2026-06-23T07:29:14.176693+00:00

visibility ruled internal during alignment

### UI/UX visual-register seed flow

**Date:** 2026-06-23T07:29:14.137206+00:00

visibility ruled internal during alignment

### self-teaching surface

**Date:** 2026-06-23T03:01:12.868512+00:00

Meta-cognitive friction log from 2026-06-22 drive: (1) 681 RELATES_TO verdicts with empty evidence passed doctor's laundering detection for too long — fixed by routing empty-evidence verdicts to the review queue regardless of confidence. (2) 24 IMPLEMENTS locators pointed to non-symbol text (enum variants, prose, constants) — re-grounded all to actual tree-sitter-extracted symbols. (3) Doctor's laundering detection counted edges on deprecated intents, inflating the signal — fixed by excluding deprecated-endpoint edges from the concentration detector. (4) committed_export_action was a conditional top-level JSON key that broke the frozen key set contract — moved to the alarms array. (5) corpus ignore command lacked an EXAMPLE after_help — added. (6) Test subprocesses didn't isolate LOOM_AGENT from the parent shell — fixed by adding env_remove to all test Command builders.

### (floating)

**Date:** 2026-06-23T02:56:30.501336+00:00

The next command's internal helpers (ALIGN_EMPTY_MESSAGE, BATCH_TEMPLATE_HINTS, BATCH_TEMPLATE_TITLE, QUALITY_EMPTY_MESSAGE, emit_audit_directive, phase_default_mode, print_batch_template_header, inject_take_note, cap_section, NextOpts) are all owned by the three intents grounding this file: SQLite query and search implementation (pub fn run entry point), directed handoff notes (fn note_surfaces), and computed graph population lane (run_with_repo). These symbols are dispatch infrastructure supporting all three intents' functionality, not separate product concerns warranting their own intents.

### (floating)

**Date:** 2026-06-22T14:53:34.422345+00:00

A serialization invariant, not a recoverable path: serde_json serialization of an already-constructed Value cannot fail, so the abort asserts an impossible condition rather than handling runtime input. Propagating it as a Result would force every read command to handle an error that cannot occur.

### (floating)

**Date:** 2026-06-22T14:53:34.216978+00:00

The 16 files are ONE responsibility: the read-side that computes and reports graph completeness and health — the commands (status, complete, coverage, doctor, report) plus the pure analyses they roll up (stats, comprehensiveness, completeness, integrity, symbol_accountability, and now maturity). They change together because they answer a single question about graph health; splitting the intent would fragment one coherent reporting concern into artificial children.

### (floating)

**Date:** 2026-06-22T14:53:33.952421+00:00

The tag detector is a SUPPLEMENT here, not the primary instrument: the N×N RELATES_TO grid of this graph is fully explored (5050/5050), so genuine duplicated responsibilities already surface as ground/independent verdicts between the relevant pairs. Tagging all 96 coded intents would be retrofit churn for marginal signal the explored grid already carries; the blind spot is consciously accepted.

### (floating)

**Date:** 2026-06-22T11:55:23.545202+00:00

Both `run_graph` (export) and `emit` (wiki) perform identical freshness checks (byte comparison of generated content against disk), and they intentionally show the same user-facing success message because the invariant they're verifying is semantically identical: the generated output file matches the graph source. The wiki.rs comment explicitly notes it "mirrors `loom export`" in this pattern. Since export and wiki are independent command modules that may need to evolve separately, and the string expresses a shared semantic condition (not a shared constant or helper), duplicating it here is correct — they're two surfaces with the same semantics, not one contract that would drift.

### (floating)

**Date:** 2026-06-22T11:55:23.481131+00:00

The message "GOVERNS edge created  (id: {})" appears in two independent command dispatch paths (EdgeCmd::Govern in edge.rs:569 and RuleCmd::Apply in rule.rs:388) that each represent a separate CLI interface contract. Although both paths create the same database edge, they guide users through different workflows: edge.rs directs to `loom rule verdict` while rule.rs directs to `loom rule check`. Extracting to a shared constant would couple these command surfaces and risk unintended coupling of their evolving output contracts.

### (floating)

**Date:** 2026-06-22T11:55:23.430779+00:00

Both `ensure_gitignored()` and `install_pre_commit_hook()` check for `.git` directory existence and return the identical string to report the skip reason—because the semantic condition is identical. These are two independent best-effort setup operations (gitignore management vs. pre-commit hook installation) with separate responsibilities that may evolve apart; the string reflects the shared precondition, not a drift-prone contract. Extracting it would couple these functions unnecessarily without reducing conceptual complexity.

### (floating)

**Date:** 2026-06-22T11:55:23.380977+00:00

LANDINGS (door.rs:27–78) is the capture-time routing menu shown when an utterance first arrives, while ROUTE_MENU (inbox.rs:49–131) is the expanded triage taxonomy for already-captured items. The shared row "redesign idea / recurring breakage" is identical in trigger condition only because the utterance class is the same; the second and third fields are intentionally specialized to their contexts. LANDINGS includes "(a DIFFERENT agent proves it via loom next --mode prove)" to guide autonomous draining at capture time, while ROUTE_MENU's terse "hypothesis" and compact command suit later-stage triage. Unifying them would entangle two independent intake surfaces (door vs. inbox) and lose the domain-specific guidance each surface provides.

### (floating)

**Date:** 2026-06-22T11:55:23.321750+00:00

The string "populate derived graph structure" appears in two enforcement layers: the lane authorization gate (gate.rs:242 as the POPULATE_GRAPH Lane action) and the database ownership check (populate.rs:242 in ensure_owned). The duplication is intentional—the lane defines what operation is authorized, and ensure_owned audits ownership for that same operation by name. Extracting to a shared const would obscure the direct connection between these two independent enforcement points that must remain locally readable.

### (floating)

**Date:** 2026-06-22T11:55:23.273759+00:00

The `pack_rule_effort()` function (line 218–232) is the single authoritative dispatch point for inspection effort overrides, centralizing rules that demand "deep semantic reading." The match arm at line 228 names "mobile-lifecycle-safe-state" to tag this rule as high-effort, while the same string at line 55 defines it in the MOBILE_PACK const data. These are semantically identical and intentionally coupled: the rule must appear in the pack definition AND in the effort lookup table. Extracting to a shared const would scatter pack content from its metadata across the file, breaking the cohesion of the pack-definition-and-its-effort pattern.

### (floating)

**Date:** 2026-06-22T11:55:23.216838+00:00

The `pack_rule_effort()` function (line 218–232) is the single authoritative dispatch point for inspection effort overrides, centralizing rules that demand "deep semantic reading." The match arm at line 228 names "mobile-lifecycle-safe-state" to tag this rule as high-effort, while the same string at line 55 defines it in the MOBILE_PACK const data. These are semantically identical and intentionally coupled: the rule must appear in the pack definition AND in the effort lookup table. Extracting to a shared const would scatter pack content from its metadata across the file, breaking the cohesion of the pack-definition-and-its-effort pattern.

### (floating)

**Date:** 2026-06-22T11:55:23.158750+00:00

LANDINGS (door.rs:27–78) is the capture-time routing menu shown when an utterance first arrives, while ROUTE_MENU (inbox.rs:49–131) is the expanded triage taxonomy for already-captured items. The shared row "redesign idea / recurring breakage" is identical in trigger condition only because the utterance class is the same; the second and third fields are intentionally specialized to their contexts. LANDINGS includes "(a DIFFERENT agent proves it via loom next --mode prove)" to guide autonomous draining at capture time, while ROUTE_MENU's terse "hypothesis" and compact command suit later-stage triage. Unifying them would entangle two independent intake surfaces (door vs. inbox) and lose the domain-specific guidance each surface provides.

### (floating)

**Date:** 2026-06-22T11:55:23.097251+00:00

The repeated text is `PRAGMA table_info({table})`, a fixed SQLite schema-introspection call. Production `table_has_column` checks one column exists; the test helper `table_columns` independently issues the same PRAGMA to VERIFY that migrations rebuilt the schema. Centralising it would couple the migration test to the very helper it exists to check, defeating the independent verification — and `PRAGMA table_info` is a stable SQLite API, not a drifting product contract.

### (floating)

**Date:** 2026-06-22T11:55:23.048505+00:00

The .expect("serializing JSON error object cannot fail") at line 84 guards a proven invariant: when the outer serde_json::to_string(value) fails, the code falls back to serializing a constant single-level JSON object {"error": string}. Serializing this static structure via serde_json::to_string is mathematically impossible to fail (only primitives and basic JSON types, no complex deserialization). The comment explicitly names the invariant, and this is a one-off safety envelope at a single location, not a duplicated error string across call sites.

### (floating)

**Date:** 2026-06-22T11:55:23.000060+00:00

The string "populate derived graph structure" appears in two enforcement layers: the lane authorization gate (gate.rs:242 as the POPULATE_GRAPH Lane action) and the database ownership check (populate.rs:242 in ensure_owned). The duplication is intentional—the lane defines what operation is authorized, and ensure_owned audits ownership for that same operation by name. Extracting to a shared const would obscure the direct connection between these two independent enforcement points that must remain locally readable.

### (floating)

**Date:** 2026-06-22T11:55:22.952363+00:00

The string defines a validation contract passed to gate::require_substantive() when status=="independent". Both batch.rs (apply_line_sqlite) and rule.rs (run_with_sqlite) enforce the identical semantic requirement: evidence must explain why a rule is independent. The string must be identical because it's not an independent error message but rather a shared semantic constraint that both single-shot and batch verdicts must enforce identically to preserve graph integrity across two separate entry points into the same rule verdict operation.

### (floating)

**Date:** 2026-06-22T11:55:22.901699+00:00

The string defines a validation contract passed to gate::require_substantive() when status=="independent". Both batch.rs (apply_line_sqlite) and rule.rs (run_with_sqlite) enforce the identical semantic requirement: evidence must explain why a rule is independent. The string must be identical because it's not an independent error message but rather a shared semantic constraint that both single-shot and batch verdicts must enforce identically to preserve graph integrity across two separate entry points into the same rule verdict operation.

### (floating)

**Date:** 2026-06-22T11:55:22.848910+00:00

The error message is identical in both locations because it expresses the same user-facing instruction about quoting, but the two functions operate on fundamentally different data sources: `prepare_additions` validates globs against the filesystem using `glob::glob()`, while `resolve_codefiles_with_db` matches patterns against already-registered codefiles in the database using `glob::Pattern`. These are independent error handlers in separate commands (codefile add vs. edge implement/relate), and their divergence would be correctness-preserving rather than a hidden dependency—each could evolve its error guidance independently if the underlying glob semantics changed.

### (floating)

**Date:** 2026-06-22T08:38:44.488687+00:00

reads.rs SqliteGraphStore impl (lines 4-1291) contains expect() in #[cfg(test)] methods that test the store. Test code uses expect() to fail fast on fixture errors, not production behavior.

### (floating)

**Date:** 2026-06-22T08:38:44.474145+00:00

print_json uses unwrap() on serde_json::to_string — but the value is already a serde_json::Value, which is always serializable. The unwrap is on an infallible operation proven by the type system; replacing with handled error adds dead code.

### (floating)

**Date:** 2026-06-22T08:38:44.459743+00:00

Printer impl (lines 36-112) uses expect() in test code within the impl block — tests assert output formatting invariants. expect() in test methods fails fast on test setup errors.

### (floating)

**Date:** 2026-06-22T08:38:44.437658+00:00

gs_of_with_disk (lines 548-561) uses unwrap() to build a graph_state with disk scan in test fixtures. Same rationale as gs_of: test infrastructure assertion.

### (floating)

**Date:** 2026-06-22T08:38:44.421363+00:00

gs_of (lines 372-385) uses unwrap() to build a graph_state from test fixtures. Test helper that fails fast on fixture errors — if the fixture can't build a graph_state, the test setup is broken.

### (floating)

**Date:** 2026-06-22T08:38:44.404899+00:00

resolve_inbox_item (lines 744-779) uses expect() on inbox item resolution after a preceding query verified the row exists. The expect asserts a DB-level invariant: if the row doesn't exist after the query found it, the database is corrupt. This is a defensive assert on a verified precondition.

### (floating)

**Date:** 2026-06-22T08:38:44.386259+00:00

sorted_json (lines 306-313) uses unwrap() to parse JSON from test fixtures. This test helper asserts the fixture produces valid JSON — if it doesn't, the test is broken. unwrap() is correct in test helpers.

### (floating)

**Date:** 2026-06-22T08:38:44.367154+00:00

current_export (lines 264-268) uses expect() to read the export from a test fixture graph. This test helper asserts the fixture can produce an export — if it can't, the test setup is broken. expect() is correct here: it fails fast on infrastructure, not production behavior.

### (floating)

**Date:** 2026-06-22T08:38:44.146418+00:00

Layer command string literals duplicated between run_list_with_db and print_order_result. The 1 duplication is the layer rendering format shared by list and order.

### (floating)

**Date:** 2026-06-22T08:38:44.132884+00:00

Init string literals duplicated between ensure_gitignored and install_pre_commit_hook. The 1 duplication is the .loom directory path shared by both init helpers.

### (floating)

**Date:** 2026-06-22T08:38:44.118715+00:00

Inbox next_step_for string literals duplicated with next/render.rs build_closeout_queues. The 1 duplication is the queue rendering protocol shared by inbox and next.

### (floating)

**Date:** 2026-06-22T08:38:44.105967+00:00

Printer impl string literals duplicated between print_json and the impl block. The 1 duplication is the JSON serialization format where print_json references the same serde contract.

### (floating)

**Date:** 2026-06-22T08:38:44.092272+00:00

EnvRedactor string literals duplicated between from_spec and the impl. The 1 duplication is the env template syntax ({{ env.X }}) where from_spec references the same pattern as the impl.

### (floating)

**Date:** 2026-06-22T08:38:44.079682+00:00

DiscoveryClassFilter string literals duplicated between the impl and parse method. The 1 duplication is the filter class name where parse references the same class string as the impl.

### (floating)

**Date:** 2026-06-22T08:38:44.066910+00:00

Complete render string literals duplicated with status.rs run_with_db — both render graph_state dimensions. The 1 duplication is the graph_state projection format shared by complete and status.

### (floating)

**Date:** 2026-06-22T08:38:44.053520+00:00

Door landing constants (LANDINGS) duplicated with inbox.rs ROUTE_MENU — both define the intake routing options. The 2 duplications are the intake routing contract shared by door and inbox.

### (floating)

**Date:** 2026-06-22T08:38:44.040182+00:00

Populate interface string literals duplicated between populate_interfaces and saga.rs add_sqlite — both bind interface surfaces to intents. The 2 duplications are the interface binding protocol.

### (floating)

**Date:** 2026-06-22T08:38:44.026346+00:00

Intent command string literals duplicated between handle_confirm, update_next_step, and handle_retire — all resolve intent by key. The 2 duplications are the intent resolution protocol shared by multiple subcommands.

### (floating)

**Date:** 2026-06-22T08:38:44.012456+00:00

Codefile command string literals duplicated between run_remove_with_sqlite and run_show_with_db, plus note.rs. The 2 duplications are codefile entity resolution shared by remove and show.

### (floating)

**Date:** 2026-06-22T08:38:43.999005+00:00

Rule pack constants (ISO5055_PACK, rule names) duplicated between pack_rule_effort and the pack definition. The 3 duplications are ISO 5055 rule identifiers where the effort function references the same rule names as the pack.

### (floating)

**Date:** 2026-06-22T08:38:43.985758+00:00

Edge explore command string literals duplicated between edge.rs run_explore_with_sqlite and persona.rs run_serve_with_sqlite — both dispatch on verdict kind (ground/issue/independent). The 3 duplications are the verdict protocol shared by edge and persona commands.

### (floating)

**Date:** 2026-06-22T08:38:43.972125+00:00

Smell detection string literals (TEACHING_TABLE, kind names) duplicated between smells.rs and smells/coupling.rs. The 3 duplications are smell kind identifiers where coupling.rs references the same kind strings as the teaching table.

### (floating)

**Date:** 2026-06-22T08:38:43.960106+00:00

Resolver validation logic duplicated between resolve_validation_with_db in resolve.rs and resolve_validation_from_list in validation.rs — both resolve a validation by key (id/name/fragment) with the same ambiguity rules. The 3 duplications are the resolver contract shared by two call sites.

### (floating)

**Date:** 2026-06-22T08:38:43.946087+00:00

Import/export SQL (INSERT for intent/codefile/edge/note) duplicated between the export writer and import reader, plus test assertions. The 3 duplications are the graph format contract where import must handle exactly what export produces.

### (floating)

**Date:** 2026-06-22T08:38:43.932544+00:00

Batch JSON operation keys (op/a/b/confidence/criterion) duplicated between apply_line_sqlite parser and test fixtures in edge.rs/rule.rs. The 6 duplications are the batch protocol contract where test fixtures assert literal keys to catch protocol drift.

### (floating)

**Date:** 2026-06-22T08:38:43.919064+00:00

DDL strings (CREATE TABLE/ALTER TABLE) duplicated between schema_ddl.rs migration functions and test assertions that verify the schema contract. The 10 duplications are migration DDL where tests assert the literal SQL to catch migration drift.

### (floating)

**Date:** 2026-06-22T08:38:43.905559+00:00

NoteKind FromStr/Display string mappings (decision/smell/finding/etc.) appear in both the impl and the test that asserts the round-trip. The 11 duplications are enum variant string representations that tests must assert literally to catch mapping drift.

### (floating)

**Date:** 2026-06-22T08:38:43.892039+00:00

Legacy sqlite.rs bridge SQL duplicated with the new typed store (reads.rs/writes.rs) and tests — list_relates_to appears in both the bridge and the typed store for parity. The 22 duplications exist because the bridge must execute identical SQL to the typed store during the migration parity period.

### (floating)

**Date:** 2026-06-22T08:38:43.878402+00:00

reads.rs query SQL (SELECT for intent/codefile/edge/snapshot) duplicated between runtime reads and test assertions at list_relates_to and list_calls_for_interface. The 40 duplications are read queries where test copies verify the exact JOIN/WHERE clauses. Sharing constants would make tests assert the constant, not the query.

### (floating)

**Date:** 2026-06-22T08:38:43.864270+00:00

edge_writes.rs contains edge mutation SQL (INSERT/UPDATE for relates_to/governs/implements) duplicated across runtime, migration, and tests. The 40 duplications are edge lifecycle statements (insert/update/retire) where the test copies assert the exact column names and inspection_status transitions to catch schema drift. Migration DDL must be self-contained SQL.

### (floating)

**Date:** 2026-06-22T08:38:43.849432+00:00

writes.rs runtime SQL (INSERT/UPDATE for intent/codefile/note/rule) is mirrored in schema_ddl.rs DDL and sqlite_regression.rs assertions. The 82 duplications are the edge/intent/note mutation statements: each appears once as runtime execution, once in migration DDL, once in test assertions. Extracting constants would couple the migration (which must be raw SQL, replayable from scratch) to runtime Rust types.

### (floating)

**Date:** 2026-06-22T08:38:39.459187+00:00

reads.rs SqliteGraphStore impl (lines 4-1291) contains expect() in #[cfg(test)] methods that test the store. Test code uses expect() to fail fast on fixture errors, not production behavior.

### (floating)

**Date:** 2026-06-22T08:38:39.445574+00:00

print_json uses unwrap() on serde_json::to_string — but the value is already a serde_json::Value, which is always serializable. The unwrap is on an infallible operation proven by the type system; replacing with handled error adds dead code.

### (floating)

**Date:** 2026-06-22T08:38:39.432544+00:00

Printer impl (lines 36-112) uses expect() in test code within the impl block — tests assert output formatting invariants. expect() in test methods fails fast on test setup errors.

### (floating)

**Date:** 2026-06-22T08:38:39.419333+00:00

gs_of_with_disk (lines 548-561) uses unwrap() to build a graph_state with disk scan in test fixtures. Same rationale as gs_of: test infrastructure assertion.

### (floating)

**Date:** 2026-06-22T08:38:39.406853+00:00

gs_of (lines 372-385) uses unwrap() to build a graph_state from test fixtures. Test helper that fails fast on fixture errors — if the fixture can't build a graph_state, the test setup is broken.

### (floating)

**Date:** 2026-06-22T08:38:39.394552+00:00

resolve_inbox_item (lines 744-779) uses expect() on inbox item resolution after a preceding query verified the row exists. The expect asserts a DB-level invariant: if the row doesn't exist after the query found it, the database is corrupt. This is a defensive assert on a verified precondition.

### (floating)

**Date:** 2026-06-22T08:38:39.380823+00:00

sorted_json (lines 306-313) uses unwrap() to parse JSON from test fixtures. This test helper asserts the fixture produces valid JSON — if it doesn't, the test is broken. unwrap() is correct in test helpers.

### (floating)

**Date:** 2026-06-22T08:38:39.367819+00:00

current_export (lines 264-268) uses expect() to read the export from a test fixture graph. This test helper asserts the fixture can produce an export — if it can't, the test setup is broken. expect() is correct here: it fails fast on infrastructure, not production behavior.

### (floating)

**Date:** 2026-06-22T08:38:39.159262+00:00

Layer command string literals duplicated between run_list_with_db and print_order_result. The 1 duplication is the layer rendering format shared by list and order.

### (floating)

**Date:** 2026-06-22T08:38:39.147624+00:00

Init string literals duplicated between ensure_gitignored and install_pre_commit_hook. The 1 duplication is the .loom directory path shared by both init helpers.

### (floating)

**Date:** 2026-06-22T08:38:39.134866+00:00

Inbox next_step_for string literals duplicated with next/render.rs build_closeout_queues. The 1 duplication is the queue rendering protocol shared by inbox and next.

### (floating)

**Date:** 2026-06-22T08:38:39.121951+00:00

Printer impl string literals duplicated between print_json and the impl block. The 1 duplication is the JSON serialization format where print_json references the same serde contract.

### (floating)

**Date:** 2026-06-22T08:38:39.109039+00:00

EnvRedactor string literals duplicated between from_spec and the impl. The 1 duplication is the env template syntax ({{ env.X }}) where from_spec references the same pattern as the impl.

### (floating)

**Date:** 2026-06-22T08:38:39.096630+00:00

DiscoveryClassFilter string literals duplicated between the impl and parse method. The 1 duplication is the filter class name where parse references the same class string as the impl.

### (floating)

**Date:** 2026-06-22T08:38:39.083964+00:00

Complete render string literals duplicated with status.rs run_with_db — both render graph_state dimensions. The 1 duplication is the graph_state projection format shared by complete and status.

### (floating)

**Date:** 2026-06-22T08:38:39.071418+00:00

Door landing constants (LANDINGS) duplicated with inbox.rs ROUTE_MENU — both define the intake routing options. The 2 duplications are the intake routing contract shared by door and inbox.

### (floating)

**Date:** 2026-06-22T08:38:39.054820+00:00

Populate interface string literals duplicated between populate_interfaces and saga.rs add_sqlite — both bind interface surfaces to intents. The 2 duplications are the interface binding protocol.

### (floating)

**Date:** 2026-06-22T08:38:39.020261+00:00

Intent command string literals duplicated between handle_confirm, update_next_step, and handle_retire — all resolve intent by key. The 2 duplications are the intent resolution protocol shared by multiple subcommands.

### (floating)

**Date:** 2026-06-22T08:38:39.008616+00:00

Codefile command string literals duplicated between run_remove_with_sqlite and run_show_with_db, plus note.rs. The 2 duplications are codefile entity resolution shared by remove and show.

### (floating)

**Date:** 2026-06-22T08:38:38.996266+00:00

Rule pack constants (ISO5055_PACK, rule names) duplicated between pack_rule_effort and the pack definition. The 3 duplications are ISO 5055 rule identifiers where the effort function references the same rule names as the pack.

### (floating)

**Date:** 2026-06-22T08:38:38.983184+00:00

Edge explore command string literals duplicated between edge.rs run_explore_with_sqlite and persona.rs run_serve_with_sqlite — both dispatch on verdict kind (ground/issue/independent). The 3 duplications are the verdict protocol shared by edge and persona commands.

### (floating)

**Date:** 2026-06-22T08:38:38.969440+00:00

Smell detection string literals (TEACHING_TABLE, kind names) duplicated between smells.rs and smells/coupling.rs. The 3 duplications are smell kind identifiers where coupling.rs references the same kind strings as the teaching table.

### (floating)

**Date:** 2026-06-22T08:38:38.956346+00:00

Resolver validation logic duplicated between resolve_validation_with_db in resolve.rs and resolve_validation_from_list in validation.rs — both resolve a validation by key (id/name/fragment) with the same ambiguity rules. The 3 duplications are the resolver contract shared by two call sites.

### (floating)

**Date:** 2026-06-22T08:38:38.943420+00:00

Import/export SQL (INSERT for intent/codefile/edge/note) duplicated between the export writer and import reader, plus test assertions. The 3 duplications are the graph format contract where import must handle exactly what export produces.

### (floating)

**Date:** 2026-06-22T08:38:38.929335+00:00

Batch JSON operation keys (op/a/b/confidence/criterion) duplicated between apply_line_sqlite parser and test fixtures in edge.rs/rule.rs. The 6 duplications are the batch protocol contract where test fixtures assert literal keys to catch protocol drift.

### (floating)

**Date:** 2026-06-22T08:38:38.916081+00:00

DDL strings (CREATE TABLE/ALTER TABLE) duplicated between schema_ddl.rs migration functions and test assertions that verify the schema contract. The 10 duplications are migration DDL where tests assert the literal SQL to catch migration drift.

### (floating)

**Date:** 2026-06-22T08:38:38.904014+00:00

NoteKind FromStr/Display string mappings (decision/smell/finding/etc.) appear in both the impl and the test that asserts the round-trip. The 11 duplications are enum variant string representations that tests must assert literally to catch mapping drift.

### (floating)

**Date:** 2026-06-22T08:38:38.891798+00:00

Legacy sqlite.rs bridge SQL duplicated with the new typed store (reads.rs/writes.rs) and tests — list_relates_to appears in both the bridge and the typed store for parity. The 22 duplications exist because the bridge must execute identical SQL to the typed store during the migration parity period.

### (floating)

**Date:** 2026-06-22T08:38:38.879325+00:00

reads.rs query SQL (SELECT for intent/codefile/edge/snapshot) duplicated between runtime reads and test assertions at list_relates_to and list_calls_for_interface. The 40 duplications are read queries where test copies verify the exact JOIN/WHERE clauses. Sharing constants would make tests assert the constant, not the query.

### (floating)

**Date:** 2026-06-22T08:38:38.865680+00:00

edge_writes.rs contains edge mutation SQL (INSERT/UPDATE for relates_to/governs/implements) duplicated across runtime, migration, and tests. The 40 duplications are edge lifecycle statements (insert/update/retire) where the test copies assert the exact column names and inspection_status transitions to catch schema drift. Migration DDL must be self-contained SQL.

### (floating)

**Date:** 2026-06-22T08:38:38.851744+00:00

writes.rs runtime SQL (INSERT/UPDATE for intent/codefile/note/rule) is mirrored in schema_ddl.rs DDL and sqlite_regression.rs assertions. The 82 duplications are the edge/intent/note mutation statements: each appears once as runtime execution, once in migration DDL, once in test assertions. Extracting constants would couple the migration (which must be raw SQL, replayable from scratch) to runtime Rust types.

### (floating)

**Date:** 2026-06-22T08:37:06.095872+00:00

The unwrap/expect is on an infallible operation proven by the type system (e.g., serde_json::Value → String is always serializable) or on a DB invariant verified by the preceding query. Replacing with handled error would add dead code for an impossible branch.

### (floating)

**Date:** 2026-06-22T08:37:06.007712+00:00

expect()/unwrap() in test code — test helpers and #[cfg(test)] methods use expect() to fail fast on test infrastructure errors (fixture setup, DB creation, JSON parsing). If the fixture fails, the test is broken, not production code. This is the correct use of expect() in test helpers.

### (floating)

**Date:** 2026-06-22T08:37:05.740289+00:00

completeness and integrity checking grounds 10 files because it IS the integrity checking surface — each file is one projection of graph integrity (coverage, doctor, impact, complete, report, hotspots). The scatter reflects the breadth of integrity checking, not poor cohesion.

### (floating)

**Date:** 2026-06-22T08:37:05.645856+00:00

backend-neutral storage boundary grounds 19 files because it IS the storage boundary — every command handler that reads from the graph implements this intent via pub fn run_with_db. The scatter is the architecture: the storage boundary is a cross-cutting concern that every read command touches. Consolidating would mean merging unrelated command files.

### (floating)

**Date:** 2026-06-22T08:37:05.502629+00:00

SQL string literals duplicated between runtime mutation paths, migration DDL, and test assertions. Each copy serves a distinct lifecycle role: runtime code executes the query, schema_ddl.rs defines the table, tests assert the literal SQL to catch contract drift. Extracting shared constants would couple migration DDL to runtime code (migrations must be self-contained and replayable) and make tests tautological (asserting the constant, not the query). The duplication IS the contract: if the SQL changes, all copies must change together, and the test copy catches drift. The duplicated strings are SQLite DML/DDL statements (INSERT/UPDATE/SELECT/CREATE) that appear in the runtime store impl, migration functions, and regression test assertions.

### (floating)

**Date:** 2026-06-22T08:37:05.486756+00:00

80 of 96 coded intents are untagged because loom's tag vocabulary was seeded for storage/migration work only; the remaining intents (static analysis, UI/UX, saga, hypothesis) lack registered vocabulary terms. The lexical fallback catches duplicates by name similarity; the untagged blind spot is accepted because the storage migration (active work) is fully tagged, and untagged intents are in stable areas with low duplication risk.

### (floating)

**Date:** 2026-06-22T08:34:41.351788+00:00

loom is a CLI tool, not a deployed service with HTTP endpoints; the 'consumer-visible' surfaces are CLI commands exercised by integration tests (sqlite_regression.rs) and the dogfood gate (cargo test && target/debug/loom status/next), not runtime sagas. A saga proof would test HTTP endpoints that don't exist. The dogfood validation (cargo test && loom status --json && loom next --all --json) IS the consumer journey proof for a CLI tool.

### (floating)

**Date:** 2026-06-21T02:07:17.285702+00:00

These repeats are the transactional/non-transactional variant pairs for relates_to operations plus the PRAGMA introspection idiom: the RelatesTo projection and the INSERT OR IGNORE / edge-not-found guard each appear in a pooled-connection path and a borrowed-transaction path (cascades read-their-own-writes), and PRAGMA table_info(table) appears in the production table_has_column and a test column-dumper. The SQL must match across each pair by construction; a const would split the query from the method that owns it without removing the two executors. Same projection, different connection contexts.

### (floating)

**Date:** 2026-06-21T02:07:17.240759+00:00

sorted_json is a deterministic-ordering test utility: serde_json::to_value(item).unwrap() then sort by to_string(value).unwrap(). The inputs are the suite's own derive(Serialize) fixtures, whose serialization is total — to_value cannot fail for these concrete types, so an unwrap firing would be a test-data defect surfaced immediately. No external input reaches it.

### (floating)

**Date:** 2026-06-21T02:07:17.194697+00:00

current_export loads the committed loom.graph.json fixture with expect("read committed export")/expect("parse committed export"). The file is a checked-in build invariant; if it is missing or malformed the suite MUST abort loudly because every downstream parity test depends on it. Failing fast is correct test-harness behaviour, not a runtime hazard.

### (floating)

**Date:** 2026-06-21T02:07:17.142016+00:00

table_columns is a unit-test introspection helper running PRAGMA table_info over an in-memory test database the test just created; its expect()s guard that introspection. A failure means the schema under test is broken, which is exactly the assertion the test exists to make, so the panic IS the intended red signal. Production reads never call it.

### (floating)

**Date:** 2026-06-20T19:22:18.897029+00:00

gs_of_with_disk is the disk-branch variant: it threads a caller-supplied disk_issues count through the fifth closure (move |_| Ok(disk_issues)) so a test can drive the audit-gate disk-reconciliation path, then unwraps. That closure still returns Ok, as do the other stubs, so the Result is always Ok and the unwrap cannot fire — the parameter only varies a successful build's count.

### (floating)

**Date:** 2026-06-20T19:22:18.836350+00:00

gs_of is the baseline GraphState test builder: it calls graph_state_from_snapshot_parts with three infallible stub closures (|_| Ok(0)) and unwraps. Every injected closure returns Ok, so the assembled Result is unconditionally Ok and the unwrap is structurally unreachable; it keeps the per-test call sites to one line for the gate-cascade assertions.

### (floating)

**Date:** 2026-06-20T19:22:18.780539+00:00

The repeats are canonical identifiers and shared teaching guidance, not a drifting contract: 'architecture_verdict_contradicts_layering' is a smell-KIND string (kind field + teaching key + test assertion — one identifier, byte-identical by necessity, part of the string-literal kind taxonomy); 'do not bulk-ground symbols without checking intent meaning' and 'do not create intents for every private helper' are avoid-guidance lines deliberately shared across sibling teachings so the same caution reads identically wherever it applies. None is a centralizable business rule.

### (floating)

**Date:** 2026-06-20T17:12:41.370375+00:00

The repeats are the not-found and ambiguity messages of two distinct command-layer resolvers — resolve_intent_with_db and resolve_validation_with_db — each keyed to a different node type. They are not one contract: the intent resolver deliberately enumerates the matching candidate names (with a plus-N-more truncation and a loom find hint), while the validation resolver only counts matches; genericizing them would regress the intent resolver's richer candidate listing. The shared 'by id, name, or fragment' phrasing is a UX convention across resolvers, not a business rule that must co-evolve — changing one resolver's wording does not require touching the other.

### (floating)

**Date:** 2026-06-20T17:12:03.924100+00:00

The remaining repeat is the literal PRAGMA table_info({table}), appearing in the production table_has_column (a single-column existence check on the migration path, early-returning on match) and in a test-only table_columns helper that dumps every column name for schema assertions. They are different operations with different return types in different layers; the shared text is fixed SQLite introspection syntax, not a contract that evolves. Routing the test through the production checker would couple test setup to a migration path or force the existence check to allocate the full column list.

### (floating)

**Date:** 2026-06-20T17:12:03.877926+00:00

The repeated text 'architecture_verdict_contradicts_layering' is a smell-KIND identifier, not prose: it is the kind field set by detect_transitive_layering_violation in coupling.rs, the match key in the teaching table here, and the assertion key in the regression test. They MUST be byte-identical because it is one canonical identifier, and loom models all ~25 smell kinds as string literals that serialize directly to JSON, teaching keys and test assertions. A const for this one kind among dozens would break that uniform taxonomy; the only consistent centralization is a crate-wide SmellKind enum, a separate model change, not a per-finding contract.

### (floating)

**Date:** 2026-06-20T17:12:03.832800+00:00

The repeats are the not-found and ambiguity messages of two distinct command-layer resolvers — resolve_intent_with_db and resolve_validation_with_db — each keyed to a different node type. They are not one contract: the intent resolver deliberately enumerates the matching candidate names (with a +N more truncation and a  hint), while the validation resolver only counts matches; genericizing them would regress the intent resolver's richer candidate listing. The shared '(by id, name, or fragment)' phrasing is a UX convention across resolvers, not a business rule that must co-evolve — changing one resolver's wording does not require touching the other.

### (floating)

**Date:** 2026-06-20T17:12:03.764339+00:00

The single panic marker is .next().expect("one match") inside resolve_inbox_item, in the match arm reached only when matches.len() == 1 was just proven one line above. The iterator is therefore guaranteed to yield Some; the expect records that arm invariant and cannot fire at runtime. Converting it to a Result would force callers to handle an impossible None. Not a reachable abort.

### (floating)

**Date:** 2026-06-20T11:50:18.302622+00:00

Accepted: 78 of 94 coded intents are untagged because loom's vocabulary is an intentionally minimal 8-term set, and the duplicated-responsibility detector's lexical fallback (name and description overlap) already surfaces the real twins here — the overlapping-ownership clusters and the command-resolver duplications were all caught without tags. Bucketing every coded intent into eight terms would be speculative classification that dilutes the vocabulary's signal without closing a gap the lexical pass misses. Growing tag coverage is a separate vocabulary-investment call, not a correctness defect in the current graph.

### (floating)

**Date:** 2026-06-20T11:50:18.265721+00:00

The three intents grounded here — sync-ripple edge staling, computed-graph population, and hypothesis-prove TARGETS stamping — share one home because they all mutate the edge tables (implements, relates_to, governs, validates, targets, serves) through the same store machinery: the shared write_tx() transaction, the insert_sync_flip_note_tx helper, and the per-edge column contracts the schema enforces. Splitting along intent lines would separate insert_targets, set_targets_status_for_hypothesis and flag_targets_needs_reverification into different files, scattering writes to the same table that must stay consistent, and would force write_tx and the note helper into public seams. The shared boundary is the edge-write transaction surface itself, not the three callers that drive it — splitting fragments the table-mutation invariant.

### (floating)

**Date:** 2026-06-20T11:41:09.109045+00:00

Interface-plane gap detection: commands/interface.rs and commands/next/render.rs inspect the already-populated interface surfaces while commands/populate.rs seeds them; the render submodule appears only because next.rs was divided into next/ files. This is one small detection-and-surface pass over the interface plane, far too singular to break into child intents — the four files are its trigger, its reader and its rendering, nothing fragmentable.

### (floating)

**Date:** 2026-06-20T11:41:09.055965+00:00

Population lane: commands/populate.rs drives brownfield and schema-upgrade computation, commands/next.rs offers it, and the resulting derived edges are written through sqlite/edge_writes.rs (where the store split relocated the population inserts). It is a single compute-then-persist thread spanning the trigger command and the store mutator plus its regression test — the minimal cohesive set for one lane, not a dispersed responsibility.

### (floating)

**Date:** 2026-06-20T11:41:08.996755+00:00

loom smells detector suite: when the 5385-line smells.rs was divided by plane into db/queries/smells/semantic.rs, physical.rs and normative.rs, this feature's groundings followed into those per-plane files, alongside the smells/domain/layer commands that present results. Higher file count here is exactly the per-plane organization the division produced. All planes run over one shared SmellInputs and QuerySnapshot and emit one SmellReport, so the capability is the whole signal set; plane-level children would splinter a single pass.

### (floating)

**Date:** 2026-06-20T11:41:08.950016+00:00

Vertical-spine soundness check computed in db/queries/completeness.rs, integrity.rs and symbol_accountability.rs (with stats/tests.rs only present because the stats module's tests were split out), then rendered by coverage/doctor/hotspots/report/status. These modules answer one question — is the graph structurally sound (tree shape, leaf realization, symbol ownership) — sharing a single snapshot pass. Pulling them into child intents would scatter one cohesive integrity guarantee across artificial pieces.

### (floating)

**Date:** 2026-06-20T11:41:08.903489+00:00

Typed read/query layer: db/queries/{snapshot,find,scoring,stats}.rs plus the sqlite/reads.rs and sqlite/search.rs implementations carved out when the 6948-line sqlite.rs was broken up, surfaced by read-heavy commands find/status/report/doctor/next. The breadth grew because that monolith became focused query files — the intended outcome, not fragmentation. Every file is the same serve-typed-reads behavior over a different table or caller; file-level children would carry no user-meaningful boundary.

### (floating)

**Date:** 2026-06-20T11:41:08.865762+00:00

Storage boundary: command handlers and query consumers depend on the GraphStore trait in src/db/mod.rs, never concrete SQLite in src/db/sqlite.rs. The grounding deliberately covers every command file because the invariant IS that breadth — each consumer routing through the abstraction rather than raw SQL is the evidence the seam holds. One architectural rule realized across the whole consumer surface; per-command children would invert the model whose point is that no command is special.

### (floating)

**Date:** 2026-06-20T11:37:33.180719+00:00

The shared text is the literal CLI invocation 'loom inbox triage --take 20', returned by next_step_for when an item is status=new and printed again by build_closeout_queues in next/render.rs as the inbox queue's start-here hint. Both surface the actual command a user types to drain new inbox items, so the literal is intentionally identical — it is the command, not a description of it. Hoisting a four-token command string into a constant would add indirection across the inbox handler and the closeout renderer for a user-facing invocation that is meant to be copy-pasted verbatim from either surface.

### (floating)

**Date:** 2026-06-20T11:37:33.138393+00:00

The repeated value is the reopens_when phrase 'a new grounding lands on the importing intent', stamped on the AdjudicatedSmell built by detect_layering_violation and detect_transitive_layering_violation. Unlike the physical detectors that reopen on file edits, these coupling detectors reopen when a fresh IMPLEMENTS grounding changes the import graph, so their disclosure names that distinct trigger. Each layering detector inlines it while constructing its own record over a different evidence shape (direct vs transitive path), so the phrase is a co-located field of two independent detectors, not a contract to centralize.

### (floating)

**Date:** 2026-06-20T11:37:33.102776+00:00

The shared literal is the reopens_when value 'the file is modified after the ruling', set on the AdjudicatedSmell record built by detect_large_behavioral_symbol and detect_panic_marker_risk (and the oversized-file detector). These are the physical-plane detectors whose adjudications are invalidated by a content-hash change, so they all disclose the same reopen trigger. It is a per-detector field value assembled where each detector constructs its own record — there is no shared function to host it without passing the same constant back through every detector, and the wording is a deliberate uniform disclosure tied to the file-modification reopen rule these detectors share.

### (floating)

**Date:** 2026-06-20T11:37:13.485931+00:00

The repeat is the hypothesis not-found resolver error shared between resolve_hypothesis_with_db (the command-layer resolver that also matches name fragments) and the store-level resolve_hypothesis in reads.rs. Each is an independent failure surface at its own layer: the command resolver fails with the user's typed key after fragment matching, the store resolver fails on a missing id during a write. They are kept worded the same for a consistent CLI experience, but they are not one contract that must co-evolve — this matches the graph's existing ruling that per-resolver not-found messages are independent surfaces, not a centralizable business rule.

### (floating)

**Date:** 2026-06-20T11:37:13.443962+00:00

The shared string is the empty-queue line 'No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look.', emitted by run_review (the interactive next --mode review) and run_take_review (the bulk --take drain). Each renders it twice more as a JSON message and a text println. It is one user-facing sentence intentionally identical across the two review entry points so an agent sees the same closeout whether it pulls one item or drains the queue; REVIEW_CONFIDENCE is already the single source of the threshold. Consolidating the sentence would couple the interactive and bulk handlers for a copy string that is meant to read identically.

### (floating)

**Date:** 2026-06-20T11:37:13.406858+00:00

These repeats are the transactional/non-transactional variant pairs for relates_to operations: the RelatesTo column projection in get_relates_to_between_tx mirrors the public get_relates_to_between in reads.rs; the INSERT OR IGNORE INTO relates_to and the 'Cannot create edge: one or both intents not found' guard in get_or_create_relates_to_tx mirror get_or_create_relates_to in edge_writes.rs; the generic SELECT {from}, {to} FROM {table} mirrors the EDGE_SPECS walk; and PRAGMA table_info(table) in table_has_column mirrors the test introspector. The _tx halves exist so cascades (ripple/retire) can run inside a borrowed Transaction while the public halves run on the pooled connection. The SQL must match across each pair by construction — a const would split the query from the method that owns it without removing the two executors. Same projection, two connection contexts.

### (floating)

**Date:** 2026-06-20T11:37:13.373751+00:00

The repeated text is the per-edge staling statement UPDATE <table> SET inspection_status = 'needs_reverification' WHERE <natural key>, one per edge table (relates_to, targets, serves, governs). Each appears twice: once in its standalone flag_<edge>_needs_reverification method (which guards on the prior status and records a sync-flip transition note in its own write_tx) and once inline in ripple_intent_redefinition / retire_intent (which stale unconditionally inside the bulk cascade's already-open transaction). They are the same statement by necessity — staling any edge must set that exact sentinel on that table's key — but cannot be one function: one path owns its transaction and writes a note, the other borrows the caller's transaction and skips it. Deliberately consistent, not a drifting contract.

### (floating)

**Date:** 2026-06-20T11:33:37.113339+00:00

gs_of_with_disk is the disk-branch variant: it threads a caller-supplied disk_issues count through the fifth closure (move |_| Ok(disk_issues)) so a test can drive the audit-gate disk-reconciliation path, then unwraps. That closure still returns Ok, as do the other two stubs, so the Result is always Ok and the unwrap cannot panic — the parameter only varies the count fed into a successful build, never its fallibility.

### (floating)

**Date:** 2026-06-20T11:33:37.065547+00:00

gs_of is the baseline GraphState builder for the stats tests; it calls graph_state_from_snapshot_parts with three infallible stub closures (|_| Ok(0), || Ok(0), |_| Ok(0)) and unwraps the Result. Because every injected closure returns Ok, the assembled Result is unconditionally Ok and the unwrap is structurally unreachable. It keeps the per-test call sites to one line for the gate-cascade assertions.

### (floating)

**Date:** 2026-06-20T11:33:37.012561+00:00

sorted_json is a deterministic-ordering test utility: it serde_json::to_value(item).unwrap() and sorts by to_string(value).unwrap(). The inputs are the suite's own #[derive(Serialize)] fixtures, whose serialization is total — to_value cannot fail for these concrete types. An unwrap firing would mean a malformed test struct, a test-data defect surfaced immediately. No external or user input reaches it.

### (floating)

**Date:** 2026-06-20T11:33:36.962811+00:00

current_export loads the committed loom.graph.json fixture via expect("read committed export") and expect("parse committed export"). The file is a checked-in build invariant; if it is missing or malformed the regression suite MUST abort loudly because every parity test downstream depends on that fixture being present and valid. Failing fast here is the correct test-harness behaviour, not a runtime hazard.

### (floating)

**Date:** 2026-06-20T11:33:36.923574+00:00

table_columns is a unit-test introspection helper that runs PRAGMA table_info and collects column names; its expect("table_info")/expect("query")/expect("collect") guard a query against an in-memory test database the test itself just created. A failure means the schema under test is broken — which is exactly the assertion the test exists to make, so a panic IS the intended red signal. Production reads never call this fn.

### (floating)

**Date:** 2026-06-20T11:33:36.877295+00:00

The only panic marker here is .next().expect("one match") inside resolve_inbox_item, and it sits in the match arm where matches.len() == 1 has just been proven. The length is checked immediately above, so the iterator is guaranteed to yield Some; the expect documents that arm invariant and can never fire at runtime. Returning a Result there would force callers to handle an impossible None. Not a reachable panic.

### (floating)

**Date:** 2026-06-20T11:08:32.897960+00:00

Audited: edge_writes.rs is the edge-mutation submodule split from sqlite.rs — it collects the IMPLEMENTS/RELATES_TO/GOVERNS/VALIDATES/TARGETS write + sync-ripple methods (flag_*_needs_reverification, delete_calls_for_validation, set_targets_status_for_hypothesis, etc.) as one impl SqliteGraphStore block. The three co-owning intents (sync ripple indexed update path, computed graph population lane, hypothesis prove records TARGETS confidence) each drive a subset of these edge-write helpers; file-level ownership is the right granularity — every method here is an edge-table mutation on the single store, not a separable behavior. Broad ownership of this cohesive submodule is deliberate.

### (floating)

**Date:** 2026-06-20T10:40:39.414625+00:00

Audited: the repeats ('do not bulk-ground symbols without checking intent meaning', 'do not create intents for every private helper') are test-FIXTURE note text inside the smells test submodules (graph_tests/advisory_tests), constructed to drive detector unit tests. Test data, not product contracts; each test builds the fixture it asserts on, and real detection ignores is_test symbols.

### (floating)

**Date:** 2026-06-20T10:40:39.382693+00:00

Audited: after the run_with_sqlite decomposition, the repeats are the '── Next Work Item  [mode=discovery  priority=88.00] ─────────────────────────────

── Intent A ────────────────────────────────────────────────────────
  id:          45b8b4eb-048e-4beb-b8e0-e54566bc7dda
  name:        UI state coverage via aspect
  level:       feature
  domain:      cli
  layer:       presentation
  status:      proposed
  lifecycle:   implemented
  description: each screen component carries an aspect per UI state (populated, empty, loading, error) so happy_path_only flags a component that has a populated child but no empty or error sibling

── Intent B ────────────────────────────────────────────────────────
  id:          5f876b50-7945-4df0-8aab-da0017ed580a
  name:        command definitions and dispatch
  level:       feature
  domain:      cli
  status:      confirmed
  lifecycle:   implemented
  description: clap-derive CLI surface, dispatch to handlers, bare-invocation orientation

── Edge  [RELATES_TO] ──────────────────────────────────────────────
  id:                (not yet created — `loom edge explore` records it)
  from:              UI state coverage via aspect (45b8b4eb-048e-4beb-b8e0-e54566bc7dda)
  to:                command definitions and dispatch (5f876b50-7945-4df0-8aab-da0017ed580a)

  inspection_status: unexplored
  criterion:         (none)
  evidence:          (none)
  confidence:        0.00
  priority:          88.00
  last_inspected:    (never)
  inspected_by:      (none)
  notes:             discovery signal: same domain 'cli'; structural degree 17 + 68

── Related Code Files ──────────────────────────────────────────────
  src/db/queries/smells.rs  @ ASPECT_FAMILIES
  src/db/queries/stats.rs  @ ASPECT_FAMILIES

── Validations on Intent A ─────────────────────────────────────────
  ? UI state coverage via aspect — tests  [not_run]  cmd: cargo test -- ui_state_populated a_loading_only behavioral_happy
  ⊘ UI state coverage via aspect — acceptance  [blocked]  cmd: 

── Notes (7) ──────────────────────────────────────────────────────
  [decision] visibility ruled internal during alignment  (llm)
  [confirm] meaning re-affirmed  (llm)
  [decision] lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test  (llm)
  [decision] Audit decision: command definitions and dispatch deliberately spans cli.rs, main entry wiring, command module registration, and command docs/tests. This is one dispatch surface; splitting the intent would be taxonomy redesign, not required for this audit.  (llm:analyzer)
  [decision] Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.  (llm:analyzer)
  [decision] lifecycle → needs_change: newest_aspect_child compares child.created_at.as_str() > e (db/queries/smells.rs ~1813); nearby hypothesis code guards with RFC3339 parsing but this path does not  (llm)
  [decision] lifecycle → implemented: fixed: happy-path adjudication timestamps are compared only after RFC3339 parsing  (llm:fixer)

── Suggested Action ────────────────────────────────────────────────
No relationship is tracked yet between intent 'UI state coverage via aspect' and intent 'command definitions and dispatch'. (discovery signal: same domain 'cli'; structural degree 17 + 68) Inspect whether they interact, then record the result (this creates the edge):

  loom edge explore 45b8b4eb-048e-4beb-b8e0-e54566bc7dda 5f876b50-7945-4df0-8aab-da0017ed580a ground --criterion "<coexistence criterion>" --confidence 0.9
  loom edge explore 45b8b4eb-048e-4beb-b8e0-e54566bc7dda 5f876b50-7945-4df0-8aab-da0017ed580a issue  --criterion "<criterion>" --evidence "<problem>"
  loom edge explore 45b8b4eb-048e-4beb-b8e0-e54566bc7dda 5f876b50-7945-4df0-8aab-da0017ed580a independent --notes "<why unrelated>"

  Dispatch — this is analyzer work — fills criterion, evidence, confidence, inspection_status (the verdict). Whoever takes it declares `LOOM_AGENT=llm:analyzer` (or stay bare `llm` for solo); its queue is `loom next --mode discovery`. Same contract whether that's you now, a later pass, or a parallel agent.  [effort: high]
  solo · graph 'loom': 101 intents · 1792 edges (531 unresolved) · 3826 unexplored · 86 codefiles · synced 1m ago · vertical ✗ horizontal ○ · phase=ground
  360°: grounded 86/86 ✓ · realized 71/77 · explored 637/4950 · measured 2068/2068 ✓ · proven 56/77 (exec 7·assert 49) serves …' next-step hint and the 'the ONE falsifiable …' criterion help text, now appearing across the extracted per-subcommand handlers (handle_add/handle_confirm/etc.). They are user-facing hint/help strings echoed by sibling handlers of the same command — deliberately consistent guidance, not a contract centralizable without coupling the handlers.

### (floating)

**Date:** 2026-06-20T10:40:39.238380+00:00

Audited: the repeat is the resolver not-found error "No validation matches '{}' (by id, name, or fragment). Run loom validation list." across the validation resolver's call sites. It is one resolver's failure message reused where it resolves; an independent error surface per node-type resolver, not a shared business contract that must co-evolve.

### (floating)

**Date:** 2026-06-20T09:37:18.629834+00:00

Audited: the sync command and its flag engine are one pipeline — walk/hash files, then flip affected RELATES_TO/GOVERNS/VALIDATES/IMPLEMENTS to needs_reverification with graded ripple, compact transition churn to the cap. The owners (change detection + the flag engine + storage) are sequential stages of one operation; splitting the flagging from the detection that feeds it would sever the pipeline mid-flow.

### (floating)

**Date:** 2026-06-20T09:37:18.584728+00:00

Audited: the status orientation command — the compass/360 render plus the audit-pulse gate (should_compute_audit_pulse → smell scan) and the export-staleness/coverage reconciliation footer. All three compose ONE orientation screen from the same GraphState; the audit pulse and coverage lines are sections of status, not separable commands — they share the single snapshot status already loads.

### (floating)

**Date:** 2026-06-20T09:37:18.539617+00:00

Audited: the smells command surface — the run path that calls compute_smells_from_parts and the render path that formats findings/adjudications with teaching + remedy. The two owners are derived-signal computation and its rendering; they are producer and presenter of ONE report type (SmellReport). Splitting render out would hand the same report to a sibling file for no boundary gain.

### (floating)

**Date:** 2026-06-20T09:37:18.498856+00:00

Audited: the Saga node command — add (parse spec + bind steps to intents), run (execute + record), diagnose (triage failures), list, show. All operate on consumer-plane saga proofs; diagnose reads what run records. Considered splitting diagnose/run: wrong, the runner invokes diagnosis on failure and both share the saga spec model — one node family, one command.

### (floating)

**Date:** 2026-06-20T09:37:18.454614+00:00

Audited: the Intent node's lifecycle command — add, update, list, show, retire, confirm (freshness), and tag add/remove (vocab). All operate on Intent state in the semantic plane. Considered splitting tag/confirm from CRUD: wrong, they are operations on the same node read/written through the same store handle — one node type, one command surface.

### (floating)

**Date:** 2026-06-20T09:37:18.404241+00:00

Audited: the Hypothesis node's full lifecycle command — add (propose), prove (verdict), adopt/reject (disposition), target, list, show. Every arm transitions one Hypothesis through the pre-decision plane. The owners are that lifecycle plus the graph-write it does and the proof-spawn on adopt; splitting the verbs would scatter one node's state machine across files.

### (floating)

**Date:** 2026-06-20T09:37:18.344695+00:00

Audited: this file owns the graph↔artifact projections — export (graph→loom.graph.json), import (json→graph), and the --check freshness compare. All three are the same serialize/deserialize boundary for the portable graph. Considered moving import out: wrong, import and export must share one envelope shape or the round trip breaks — they are two directions of one format.

### (floating)

**Date:** 2026-06-20T09:37:18.276649+00:00

Audited: this is the CodeFile/Ignore node management surface — run dispatches add, remove, show, list, plus ignore add / ignore list and the coverage roll-up. Every path operates on the physical-plane registration of files (CodeFile + Ignore nodes). Considered splitting registration from coverage: wrong, coverage reads the very registrations the other arms write — one node family, one command.

### (floating)

**Date:** 2026-06-20T09:35:31.474740+00:00

Audited the strings: the repeats are TEST-FIXTURE strings inside #[cfg(test)] modules (e.g. 'do not bulk-ground symbols without checking intent meaning', 'do not create intents for every private helper') — sample note/intent text constructed to drive detector unit tests. They are test data, not product contracts; each test builds the fixture text it asserts on, and the detector explicitly ignores is_test symbols for real findings.

### (floating)

**Date:** 2026-06-20T09:35:31.429852+00:00

Audited the strings: the repeat is the freshness line "✓ {out} is up to date with the graph." shared by export --check and wiki --check. Both answer the same question (is the committed artifact current?) for two artifacts (loom.graph.json, loom.wiki.md); the {out} placeholder is what differs. Identical phrasing is correct — they report one condition — and lives in the two commands that own each artifact.

### (floating)

**Date:** 2026-06-20T09:35:31.390906+00:00

Audited the strings: the repeat is the rebuild instruction "re-export from the loom that wrote this graph, then loom init . && loom import loom.graph.json", shared between doctor's schema-mismatch path and migrate. Both hit the SAME recovery — a graph written by a newer/older binary — so they MUST give the identical procedure; duplicating the exact steps is safer than a shared const that could drift from one call site's context.

### (floating)

**Date:** 2026-06-20T09:35:31.355698+00:00

Audited the strings: the repeats are empty-state lines ("No verdicts below confidence {REVIEW_CONFIDENCE} — nothing needs a second look.", with and without the ✓ prefix) plus JSON-extraction idioms (].as_str().unwrap_or(). The empty-state pair is the same message in plain vs check-marked render of the review lane; the idioms are a parsing pattern, not a contract. Neither is product wording that drifts — one is a render variant, the other a Rust idiom the detector over-counts.

### (floating)

**Date:** 2026-06-20T09:35:31.314079+00:00

Audited the strings: the repeats are resolver error messages carrying a command example — "No intent matches '{}' … Run loom intent list", "No validation matches '{}' …", "'{}' is ambiguous — it matches: … narrow the fragment or loom find". Each resolver (intent/validation/edge) raises its own, naming its own list command. They are independent failure surfaces for distinct node types; a shared template would force one error to speak for three different lookups.

### (floating)

**Date:** 2026-06-20T09:35:31.273183+00:00

Audited the strings: the repeats are FIELD-HELP/placeholder texts for the verdict fields — e.g. 'the falsifiable coexistence criterion this edge was checked against', 'why these two intents have no meaningful relationship', 'what compliance looks like for this rule on this intent' — shared between the single-edge/gate commands and the batch entry point. They describe the SAME graph fields (criterion/evidence/notes), so reading identically is the point: batch and single-shot must teach one meaning per field.

### (floating)

**Date:** 2026-06-20T09:35:31.230630+00:00

Audited the strings: the repeats are the two hypothesis-resolution errors — "Hypothesis '{}' not found. Run loom hypothesis list." and "No hypothesis matches '{}' (by id, name, or fragment)" — across prove/adopt/reject/target/show. Each is a leaf failure path with its own context; they are not one contract that must co-evolve. Distinct call sites describing the same missing noun, deliberately consistent for the user, not centralizable without coupling unrelated handlers.

### (floating)

**Date:** 2026-06-20T09:35:31.192111+00:00

Audited the strings: the repeats are the "CodeFile '{}' not found (by id or path)" lookup error and the glob-quoting hint "Invalid glob '{}': … quote it: loom codefile add 'src/**/*.rs'". Both are per-handler error contexts in the codefile add/remove/show paths — each returns its own anyhow message with the user's input spliced in. They guide a failed lookup, not a shared data contract; matching wording is incidental to describing the same noun.

### (floating)

**Date:** 2026-06-20T09:34:09.067687+00:00

Audited the strings: the repeats are the LANDINGS menu entries — exact command shapes (e.g. loom inbox triage --take 20) the door offers as the way each utterance class becomes a graph noun. They match the same commands' own help/inbox routing because all three teach the one canonical invocation. The LANDINGS table is intentionally the total enumeration; its shapes are user instructions, deliberately verbatim with the commands they name.

### (floating)

**Date:** 2026-06-20T09:34:09.025549+00:00

Audited the strings: the repeats are QualityRule NAMES (webui-touch-target-size, webui-color-contrast, mobile-touch-target-size) appearing in the pack const tables and again in pack_rule_effort's annotation lookup. The name IS the rule's identity; the effort table keys on it by design. They are stable identifiers referenced from two const tables, not duplicated prose — the duplication is the join key between a rule and its effort.

### (floating)

**Date:** 2026-06-20T09:34:08.983480+00:00

Audited the strings: the repeat is the display format "  {rank}. {layer:<24} {n:>3} intent(s)" used to render the declared layer order in both the show and the post-set confirmation. It is one column layout reused so the two views align visually; it carries no semantics that could drift — a width tweak SHOULD change both, and they sit in one small file where that is obvious.

### (floating)

**Date:** 2026-06-20T09:34:08.943116+00:00

Audited the strings: the repeats are command-SHAPE teaching lines (loom rule add … / loom intent mark … --lifecycle needs_change / loom inbox normalize … --route …) emitted by inbox triage as the routing menu. They are the canonical invocation a user copies; the same shapes appear in door's LANDINGS because both surfaces teach the same next command. User instructions on independent teaching surfaces, intentionally identical.

### (floating)

**Date:** 2026-06-20T09:34:08.890929+00:00

Audited the strings: the repeats are interface-gap KIND keys — surface_without_calls, interface_from_sagas, call_without_validates, boundary_intent_without_calls — each appearing where the detector EMITS the gap and where the renderer/test keys on it. They are the stable wire-names of the gap kinds (enum-like identifiers), deliberately identical so producer and consumer agree; they are short keys, not prose, and changing one without the other is a compile-visible break, not silent drift.

### (floating)

**Date:** 2026-06-20T09:34:08.853884+00:00

Audited the strings: the repeat is "RELATES_TO edge '{}' not found." across the edge show/explore handlers. Each is a leaf error path returning its own anyhow context with the user's bad id interpolated; they are not a shared contract whose wording must change together — extracting a const would couple two independent failure messages. Independent error surfaces, kept verbatim by coincidence of the noun.

### (floating)

**Date:** 2026-06-20T09:24:44.900651+00:00

Audited the file: this is a design-proposal document (the SQLite multi-hop query proposal). It argues one connected change, so it necessarily references several intents — the migration cutover/rollback path, storage-responsibility vocabulary, and the SQLite-backed persistence migration. Those three owners are the topics the proposal connects, not separable code units; it is prose. A proposal that touches three intents is doing its job, not tangling code.

### (floating)

**Date:** 2026-06-20T09:24:44.868692+00:00

Audited the file: README.md is the project's entry-point documentation — it explains the five-plane / six-edge model, the mechanical done-condition + audit gate, the 360 coverage vector, and quick start. tangled_file flags it because three intents are doc-grounded here, but verifiable-delegated-coverage, storage-documentation, and migration-cutover are TOPICS the README explains to a reader, not code responsibilities. A README is organized by reader need; it is prose with no module to split, and it legitimately spans the intents it documents.

### (floating)

**Date:** 2026-06-20T09:24:22.077051+00:00

Audited the code: this is the command-module ROOT — it declares all 40+ command submodules and is the single dispatch point. dispatch() is the match that routes every Command variant to its handler; teach_unknown() turns unrecognized tokens into teaching (edit_distance for typos, synonym remap); orient() handles bare loom. The three owners are the routing itself (command-definitions/dispatch) plus families the match wires through (interface-inspection, concurrency-policy). A dispatcher is one hub by construction; splitting the match scatters routing without removing the coupling.

### (floating)

**Date:** 2026-06-20T09:23:34.874024+00:00

Audited the code: loom session is turn-zero, before any utterance. offers() is pure computation — live queue counts in, an ordered OFFER MENU with one recommended out, ordered by the scarcity of the user's presence (align drift, hypothesis rulings, blocked proofs first). The three owners are session-opener teaching (its product), the visual-confirm/align queue it surfaces, and storage (the count reads). Menu computation plus the read it needs; the offer logic is deliberately separated from the DB for testing, not into another file.

### (floating)

**Date:** 2026-06-20T09:23:34.835859+00:00

Audited the code: loom validate runs proofs. execute_and_record runs validation commands with the DB CLOSED on purpose — releasing the graph lock so a proof may itself invoke loom (found by loom validating itself) — then reopens to persist results + VALIDATES verdicts in one transaction; run_all drains every not_run proof after a sync flood. The three owners are validate-all, proof/bootstrap, and validation/saga storage-isolation — the last IS that close-during-exec lock discipline, not a file that could live elsewhere.

### (floating)

**Date:** 2026-06-20T09:23:34.804699+00:00

Audited the code: loom import rebuilds a graph from export output. run_with_sqlite reads the export JSON via the edge/label/prop schema vocabulary (graph travel format), optionally transform_as_planned strips groundings for the port path, then store.import_export_json writes it. The three owners are exactly those steps: travel-format (the JSON it parses), import/export parity (the round-trip it must preserve), graph-write (the restore mutation). The format and the bridge that restores it are one command — separable only by breaking the round trip.

### (floating)

**Date:** 2026-06-20T09:22:56.926326+00:00

Audited the code: the loom validation command for one node type — add, mark, update, delete, list, show. The adoption-outcome-proof owner is the mark path (prepare_mark_result + the result→edge-status mapping that records proof verdicts); graph-write is the add/mark/update/delete mutations; storage is the list/show read. One node type (Validation), one command file; the adoption-proof behavior lives in mark, not a separable concern.

### (floating)

**Date:** 2026-06-20T09:22:56.885555+00:00

Audited the code: this is the loom rule command plus its seedable pack DATA. Six const tables (ISO5055 baseline + mobile/webui/service/data/concurrency vantage packs) sit beside seed, verdict/check, list, show. The design-system owner is literally the WEBUI_PACK const; graph-write is seed/verdict; storage is list/show. The packs are data co-located with the only command that seeds them — moving them out separates a measuring-stick table from its sole consumer for no benefit.

### (floating)

**Date:** 2026-06-20T09:22:56.853133+00:00

Audited the code: this is the whole loom note command for one node type. run dispatches add (targets intent/edge/file/smell, with the --for handoff role), prune (transition-note compaction toward the cap), and list (filtered). Its three owners map to those: directed-handoff-notes is the --for path, graph-write is add/prune, storage is the list read. Considered separating handoff from CRUD — wrong: it would scatter Note handling across files when it is one node type with one command surface.

### (floating)

**Date:** 2026-06-20T09:22:14.419358+00:00

Audited the code: door.rs is the capture-first entrance. run_with_db persists the raw utterance as an InboxItem BEFORE any graph noun, then reads every plane through the query+storage layers to assemble routing context, and renders the LANDINGS menu + DOCTRINE that teach the turn-zero ask. The session-opener-teaching owner is its product; the query+storage owners are the read it must do to show what the graph already knows about an utterance. Splitting the read away would defeat the door's purpose — routing context IS a read.

### (floating)

**Date:** 2026-06-20T09:22:14.387347+00:00

Audited the code: find.rs is an ~80-line thin command. run_with_db does one thing — calls db.find_intents(query, limit) (the SQLite BM25/FTS search living in the query layer) and renders each hit with tree position, groundings, and freshness. Considered separating the query ownership: there is nothing to separate — the file is one delegating call plus rendering. The query+storage owners are the search implementation this command fronts; ask-the-map is the command itself.

### (floating)

**Date:** 2026-06-20T09:21:43.694710+00:00

Audited the code: loom report is one read-only roll-up. report_data_from_snapshot assembles nine distinct projections off ONE snapshot — status, centrality top-5, intents-without-validations, failing GOVERNS, recent passing, edge-status counts, completeness gaps, vertical completeness, blocked validations — into a single FullReport. Considered moving the projections to separate files: wrong, it would fragment one status view and reload the snapshot nine times. The query+storage owners are the shared read those projections draw from; the summary is report's product.

### (floating)

**Date:** 2026-06-20T09:21:43.663309+00:00

Audited the code: loom doctor is one read-only command. run() opens the store (storage boundary), run_with_db computes DoctorReport via the query layer (schema-version + integrity/completeness checks), and clean_orphaned_backends reaps the DEAD_BACKEND_RELICS allowlist. Considered splitting the read-layer ownership out — wrong here: an integrity checker that cannot read the graph it checks is not a smaller unit, it is a broken one. The two storage owners are the read path doctor consumes; completeness-checking is its only product.

### (floating)

**Date:** 2026-06-20T03:06:29.707979+00:00

Deliberate: 'is up to date with the graph' freshness message shared between export.rs and wiki.rs — both are graph projection commands that check freshness the same way. The string is a status message, not a business contract; both commands report the same condition.

### (floating)

**Date:** 2026-06-20T03:06:18.799299+00:00

Deliberate: repeated test fixture strings in smell detector tests — each test uses the same fixture pattern. Test fixtures, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:18.012561+00:00

Deliberate: SqliteGraphStore impl uses unwrap_or/unwrap_or_else for safe defaults on Optional query results (e.g. graph_meta returning None for new graphs). No bare unwrap() on fallible operations. The detector flags the impl block; all instances are guarded.

### (floating)

**Date:** 2026-06-20T03:05:17.974398+00:00

Deliberate: Printer impl uses unwrap_or_default and unwrap_or on Option/Result with safe defaults — no bare unwrap() or expect() that can panic. The panic_marker_risk detector flags impl blocks containing any unwrap-like call; these are all guarded.

### (floating)

**Date:** 2026-06-20T03:05:17.932159+00:00

Deliberate: types.rs defines all shared graph types (Intent, CodeFile, RelatesTo, etc.) — all are the type system. Cohesive: one types module.

### (floating)

**Date:** 2026-06-20T03:05:17.897950+00:00

Deliberate: schema.rs is the single source of truth for graph vocabulary (node labels, edge types, properties). Cohesive: one schema declaration.

### (floating)

**Date:** 2026-06-20T03:05:17.856776+00:00

Deliberate: snapshot.rs defines QuerySnapshot + DiscoverySnapshot — the shared read projections. Cohesive: one snapshot module.

### (floating)

**Date:** 2026-06-20T03:05:17.814232+00:00

Deliberate: db/mod.rs declares the GraphReadRepository trait + path helpers + store open — all are the storage boundary surface. Cohesive: one module boundary.

### (floating)

**Date:** 2026-06-20T03:05:17.777694+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:17.737882+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:17.697311+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:17.656976+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:17.614550+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:05:17.583490+00:00

Deliberate: repeated error/example strings across command handlers — each handler needs its own error context or command example. Error messages and user instructions, not business contracts.

### (floating)

**Date:** 2026-06-20T03:04:37.021639+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.982127+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.944061+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.899210+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.858387+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.818793+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.780965+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.736909+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.692279+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.653417+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.609289+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.568369+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.527232+00:00

Deliberate: this file serves 3+ intents because it handles multiple subcommands/aspects of one command surface. Each intent is a distinct subcommand or documentation section sharing the same module. Cohesive: one command/documentation file.

### (floating)

**Date:** 2026-06-20T03:04:36.484025+00:00

Deliberate: 'boundary' string repeated in batch validation messages — each validation context needs its own boundary description. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T03:04:36.453907+00:00

Deliberate: repeated loom command example strings in error messages — each handler provides its own command example. User instruction, not a business contract.

### (floating)

**Date:** 2026-06-20T03:04:19.838600+00:00

Deliberate: 'architecture_ver...' string repeated in smells.rs test fixtures and detector code — the string is a test constant shared across detector tests. Test fixture, not a business contract.

### (floating)

**Date:** 2026-06-20T03:04:19.798389+00:00

Deliberate: SQLite-backed graph persistence migration is a completed migration — the 'happy' aspect (migration succeeded) has no 'sad' sibling because the migration is done. There's no ongoing fallback path to test; the old grafeo backend is removed.

### (floating)

**Date:** 2026-06-20T03:04:19.760236+00:00

Deliberate: sync.rs handles the sync command + flag engine — both are the change-detection surface. Cohesive: one sync pipeline.

### (floating)

**Date:** 2026-06-20T03:04:19.716811+00:00

Deliberate: status.rs handles the status command + audit pulse + coverage reconciliation — all are the status surface. Cohesive: one orientation command.

### (floating)

**Date:** 2026-06-20T03:04:19.668446+00:00

Deliberate: smells.rs handles the smells command + render — both are the smell inspection surface. Cohesive: one command, one rendering.

### (floating)

**Date:** 2026-06-20T03:04:19.628311+00:00

Deliberate: hypothesis.rs handles 5+ subcommands (add, prove, adopt, list, show) — all operate on the Hypothesis node type. Cohesive: one node type, one command file.

### (floating)

**Date:** 2026-06-20T03:04:19.588054+00:00

Deliberate: export.rs handles export + import + wiki — all are graph projection commands sharing the same read-and-serialize pattern. Cohesive: one projection module.

### (floating)

**Date:** 2026-06-20T03:04:19.546126+00:00

Deliberate: 'schema vocabulary and repository boundary' grounds in 4 files (db/schema.rs, types.rs, commands/migrate.rs, db/mod.rs) — these are the schema declaration, shared types, the migrate command, and the read boundary trait. The spread is the schema surface, not fragmentation.

### (floating)

**Date:** 2026-06-20T03:04:19.505730+00:00

Deliberate: 'saga failure diagnosis' grounds in 4 files (saga/diagnose.rs, commands/saga.rs, cli.rs, commands/mod.rs) — diagnosis is invoked from the CLI, dispatched through mod.rs, and implemented in diagnose.rs. The spread is the command chain, not fragmentation.

### (floating)

**Date:** 2026-06-20T03:04:19.468374+00:00

Deliberate: 'interface plane gap detection' grounds in 4 files — gap detection touches populate.rs, interface.rs, saga.rs, and status.rs because gaps are detected from multiple surfaces. The spread reflects the detection sources.

### (floating)

**Date:** 2026-06-20T03:04:19.432205+00:00

Deliberate: 'derived problem signals' grounds in 4 files — smells detection spans smells.rs, scoring.rs, stats.rs, and commands/smells.rs because detectors + scoring + rendering are separate concerns. The spread reflects the detection pipeline.

### (floating)

**Date:** 2026-06-20T03:04:19.392736+00:00

Deliberate: 'computed graph population lane' grounds in 4 files — population touches populate.rs, interface.rs, saga.rs, and next.rs because it backfills from multiple sources. The spread reflects the population pipeline, not fragmentation.

### (floating)

**Date:** 2026-06-20T03:04:19.344873+00:00

Deliberate: 'command definitions and dispatch' grounds in 4 files (cli.rs, main.rs, commands/mod.rs, build.rs) — all are the CLI entry surface. The spread is the dispatch chain, not fragmentation.

### (floating)

**Date:** 2026-06-20T03:03:47.685414+00:00

Deliberate: 'is ambiguous' error message in batch resolution — shared with resolve.rs for the same lookup pattern. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T03:03:47.649286+00:00

Deliberate: 'No verdicts/what compliance looks like' messages repeated across mode renderers — each mode renders its own empty-state message. UI text, not business contracts.

### (floating)

**Date:** 2026-06-20T03:03:47.608451+00:00

Deliberate: 're-export from the loom that wrote this graph' message shared with migrate.rs — both describe the same rebuild procedure. User instruction, not a business contract.

### (floating)

**Date:** 2026-06-20T03:03:47.570116+00:00

Deliberate: 'Hypothesis not found' and 'No hypothesis matches' error messages repeated across hypothesis handlers — each needs its own error context. Error messages, not business contracts.

### (floating)

**Date:** 2026-06-20T03:03:47.530126+00:00

Deliberate: 'No validation/intent/hypothesis matches' error messages repeated across lookup helpers — each needs its own error context. Error messages, not business contracts.

### (floating)

**Date:** 2026-06-20T03:03:47.491722+00:00

Deliberate: 'CodeFile not found' error message repeated across handlers — each needs its own error context. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T03:00:08.180361+00:00

Deliberate: regression test suite tests one store (SqliteGraphStore) with one harness — 5 intents are test groups (store, coverage, doctor, smells, interface) sharing the test infrastructure. Cohesive: one test harness.

### (floating)

**Date:** 2026-06-20T03:00:08.139696+00:00

Deliberate: stats.rs contains the graph-state compass + coverage computations + 360° vector — 5 intents share the same QuerySnapshot computation. Cohesive: one graph-state computation module.

### (floating)

**Date:** 2026-06-20T03:00:08.101741+00:00

Deliberate: saga.rs handles 5 subcommands (add, run, diagnose, list, show) — all operate on the Saga node type. Cohesive: one node type, one command file.

### (floating)

**Date:** 2026-06-20T03:00:08.059581+00:00

Deliberate: intent.rs handles 5+ subcommands (add, update, list, show, retire, confirm, tag) — all operate on the Intent node type. Cohesive: one node type, one command file.

### (floating)

**Date:** 2026-06-20T03:00:08.018989+00:00

Deliberate: cli.rs is the clap-derive CLI surface — 5 intents (dispatch, hypothesis lifecycle, TARGETS confidence, interface inspection, saga diagnosis) are clap enum variants, not code logic. The 'tangle' is the CLI declaration itself; each variant routes to its own handler. Cohesive: one declarative CLI surface.

### (floating)

**Date:** 2026-06-20T03:00:01.370049+00:00

Deliberate: 'storage documentation and guide refresh' touches 5 files because documentation spans README, docs/COMMANDS.md, docs/CONTRIBUTING.md, the guide command, and the wiki — each is a distinct documentation surface that needs the same refresh. The spread reflects documentation channels, not fragmented code ownership.

### (floating)

**Date:** 2026-06-20T03:00:01.338866+00:00

Deliberate: 're-export from the loom that wrote this graph' message repeated in doctor and migrate — both describe the same rebuild procedure. The string is a user instruction, not a business contract; duplication ensures each command's message is self-contained.

### (floating)

**Date:** 2026-06-20T03:00:01.295627+00:00

Deliberate: 'Hypothesis not found' error message repeated across hypothesis handlers — each needs its own error context. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T03:00:01.254530+00:00

Deliberate: 'No validation matches' error message repeated across lookup helpers — each needs its own error context. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T03:00:01.212020+00:00

Deliberate: 'CodeFile not found' error message repeated across handlers — each needs its own error context. Error message, not a business contract.

### (floating)

**Date:** 2026-06-20T02:57:54.442395+00:00

Deliberate: the 'No validation matches' error message appears in resolve_validation_with_db, resolve_validation_from_list, and SqliteGraphStore — each lookup helper needs its own error context. The string is an error message, not a business contract.

### (floating)

**Date:** 2026-06-20T02:57:54.392678+00:00

Deliberate: the 'CodeFile not found' error message appears in run_remove, run_show, and resolve_codefile_id — each handler needs its own error context. The string is an error message, not a business contract; extracting it to a shared constant would obscure the per-handler context.

### (floating)

**Date:** 2026-06-20T02:57:39.758811+00:00

Known: saga diagnosis imports interface surface inspection for interface-call resolution. Foundation import (diagnose resolves interface calls), not a specific coupling. Awaits discovery.

### (floating)

**Date:** 2026-06-20T02:57:39.717481+00:00

Known: CLI dispatch imports saga diagnosis for the saga subcommand routing. Foundation import (the dispatch routes to saga commands), not a specific coupling. Awaits discovery.

### (floating)

**Date:** 2026-06-20T02:57:39.679418+00:00

Known: graph-write handlers import saga diagnosis for error rendering. This is a foundation import (error types), not a specific coupling. The RELATES_TO grid is optional and these pairs await discovery — they don't gate green.

### (floating)

**Date:** 2026-06-20T02:57:39.640072+00:00

Deliberate: the 'No validation matches' error message is repeated across validation lookup helpers — each helper needs its own error context. The string is an error message, not a business contract.

### (floating)

**Date:** 2026-06-20T02:57:39.611721+00:00

Deliberate: the 'CodeFile not found' error message is repeated across handlers that look up codefiles by id/path — each handler needs its own error context. The string is an error message, not a business contract; extracting it to a shared constant would obscure the per-handler context.

### (floating)

**Date:** 2026-06-20T02:57:26.157343+00:00

Deliberate: repo.rs implements filesystem introspection — 6 intents (file walking, content hashing, import extraction, symbol extraction, git co-change, path confinement) are all filesystem operations sharing the same tree-sitter + walk infrastructure. Cohesive: one repo introspection module.

### (floating)

**Date:** 2026-06-20T02:57:26.114552+00:00

Deliberate: scoring.rs implements queue scoring for 6 lanes (discovery, fix, build, validate, quality, review) — each lane has its own scoring function but they share centrality and ripple-bump infrastructure. Cohesive: one scoring module, shared graph centrality.

### (floating)

**Date:** 2026-06-20T02:57:04.227137+00:00

Deliberate: 78 coded intents are untagged because the vocabulary tagging pass is incomplete — the tag detector requires tags to detect duplicated responsibility, but tagging is a gradual process. The blind spot is known and accepted during active development; tagging will be completed as the codebase stabilizes.

### (floating)

**Date:** 2026-06-20T02:57:04.190176+00:00

Deliberate: loom is a CLI tool, not a web service — there are no HTTP endpoints to exercise via saga. The 'consumer surface' is the command line, proven by the 122 validations that run loom commands. A saga journey would test an HTTP API that doesn't exist.

### (floating)

**Date:** 2026-06-20T02:57:04.144787+00:00

Deliberate: loom CLI has no runtime 'sad path' in the aspect sense — errors propagate via Result and are rendered as error messages. There's no separate fallback/intent for error handling because Rust's Result type is the error channel. A 'sad' aspect intent would be artificial.

### (floating)

**Date:** 2026-06-20T02:57:04.105204+00:00

Deliberate: the 'SQLite query and search implementation' intent covers all query modules under db/queries/ — 14 files are the query layer's modules (stats, scoring, integrity, smells, etc.). Each module is a cohesive slice of the query layer; the spread reflects module decomposition, not fragmentation.

### (floating)

**Date:** 2026-06-20T02:57:04.076453+00:00

Deliberate: the 'backend-neutral storage boundary' intent covers the GraphReadRepository trait — every command handler that opens a store grounds here because they all consume the read boundary. The 28 files are consumers of the trait, not fragmented ownership. The intent is cross-cutting by design (it's the universal read interface).

### (floating)

**Date:** 2026-06-20T02:56:48.741441+00:00

Deliberate: codefile.rs handles 7 subcommands (add, remove, show, list, ignore add, ignore list, coverage) — all operate on the CodeFile node type. Cohesive: one node type, one command file.

### (floating)

**Date:** 2026-06-20T02:56:48.703344+00:00

Deliberate: smells.rs implements all smell detectors — 8 intents are detector groups (tangled_file, large_symbol, overlapping_ownership, string_contract, undeclared_coupling, cycle_island, shotgun, proof_locality) sharing the same SmellInputs + QuerySnapshot. Cohesive: one analysis pass, one report type.

### (floating)

**Date:** 2026-06-20T02:56:48.663307+00:00

Deliberate: next.rs implements the loom next command — all 9 intents are mode handlers (discovery, fix, build, validate, quality, review, align, --all) sharing the queue-scoring infrastructure. Cohesive: one command, shared scoring.

### (floating)

**Date:** 2026-06-20T02:56:48.620940+00:00

Deliberate: guide.rs renders the loom guide — all 9 intents are guide sections (golden rules, orchestration, lifecycle, modes, etc.) rendered from one data structure. Cohesive: one command, one output format.

### (floating)

**Date:** 2026-06-20T02:56:48.575660+00:00

Deliberate: sqlite.rs is the single SQLite storage backend — all 13 intents grounded here (queries, mutations, schema, search, snapshot, integrity) share one SqliteGraphStore impl, one connection, one schema. The intents are related (all operate on the same store), not unrelated. This is a cohesive module, not a godfile.

### repo introspection

**Date:** 2026-06-18T07:53:27.687917+00:00

Fixer scoped repo.rs extract_imports and extract_symbols to #[cfg(test)] instead of #[allow(dead_code)]; cargo test passed 196/196 and cargo build plus RUSTFLAGS='-D warnings' cargo build emitted no warnings. Please re-verify iso5055-main-no-dead-or-duplicate-code.

### repo introspection

**Date:** 2026-06-18T07:42:21.694328+00:00

GOVERNS failing (iso5055-main-no-dead-or-duplicate-code on repo introspection): repo.rs extract_imports (src/repo.rs:455) and extract_symbols (src/repo.rs:462) are #[allow(dead_code)] pub fn used only by the #[cfg(test)] module (src/repo.rs:1221-1434) — unused public functions in the production build. Fix: scope both to #[cfg(test)] (their only callers are tests) or remove them. extract_imports_heuristic is the documented universal fallback — do NOT touch it. After the fix, quality re-verifies the GOVERNS edge.

### backend-neutral storage boundary

**Date:** 2026-06-18T06:55:42.865948+00:00

Resolved by validator rerun: the three previously failing backend-neutral storage boundary proofs now pass after the test regressions were fixed (cargo check, cargo test, target/debug/loom status --json, and next --all --json all exited 0). No fixer action remains for the 2026-06-18 validation failure note.

### migration cutover and rollback path

**Date:** 2026-06-18T06:54:34.305166+00:00

Validator follow-up: the local SQLite dogfood gate before global install passed in this run via cargo test plus target/debug/loom status --json and next --all --json exiting 0; global install was intentionally not promoted during validation.

### inbox intake boundary

**Date:** 2026-06-18T05:27:57.194550+00:00

lifecycle → implemented: fixed: inbox listing and prefix resolution push filters into SQL WHERE clauses

### intent-island reachability finding

**Date:** 2026-06-18T05:27:57.157871+00:00

lifecycle → implemented: fixed: twin-intent and duplicated-responsibility pair checks are bucketed by abstraction level before pairwise comparison

### smells propose hypotheses

**Date:** 2026-06-18T05:27:57.121621+00:00

lifecycle → implemented: fixed: cochange and shotgun suggestions leave final limiting to the command layer

### multi-hop audit layer

**Date:** 2026-06-18T05:26:59.568926+00:00

lifecycle → implemented: fixed: batch RELATES_TO and GOVERNS line operations now combine create/update/transition-note mutations in one transaction

### multi-hop audit layer

**Date:** 2026-06-18T05:23:06.736342+00:00

lifecycle → implemented: fixed: batch rule_verdict now inserts/updates GOVERNS verdicts atomically in one SQLite transaction

### shared symbol locator matcher

**Date:** 2026-06-18T05:22:35.413599+00:00

lifecycle → implemented: fixed: symbol-accountability resolved_pct is clamped to 0..100

### saga failure diagnosis

**Date:** 2026-06-18T05:22:35.379268+00:00

lifecycle → implemented: fixed: JWT diagnosis now distinguishes malformed bearer tokens and sufficient-scope 403s

### saga runner halt-on-failure semantics

**Date:** 2026-06-18T05:22:35.344921+00:00

lifecycle → implemented: fixed: EnvRedactor now redacts every referenced env value, including short secrets

### intent-island reachability finding

**Date:** 2026-06-18T05:21:19.257514+00:00

lifecycle → implemented: fixed: twin-intent and duplicated-responsibility scans pre-bucket active intents by abstraction level

### inbox intake boundary

**Date:** 2026-06-18T05:20:03.270817+00:00

lifecycle → implemented: fixed: inbox list and id resolution predicates are pushed into SQL WHERE clauses

### smells propose hypotheses

**Date:** 2026-06-18T05:18:34.311411+00:00

lifecycle → implemented: fixed: cochange and shotgun advisory builders no longer truncate before command --limit is applied

### discovery queue ranks pairs by plausibility

**Date:** 2026-06-18T05:17:26.995286+00:00

lifecycle → implemented: fixed: discovery smell pair generation dedups per import and explored-pair totals no longer mask hierarchy anomalies

### UI state coverage via aspect

**Date:** 2026-06-18T05:15:50.233817+00:00

lifecycle → implemented: fixed: happy-path adjudication timestamps are compared only after RFC3339 parsing

### repo introspection

**Date:** 2026-06-18T05:14:46.374404+00:00

lifecycle → implemented: fixed: confine now resolves symlinked path components before allowing repo-relative access; relative dot-only Python imports already emit the parent directory

### no silent fallbacks in the query layer

**Date:** 2026-06-18T05:13:35.887373+00:00

lifecycle → implemented: fixed: intent row JSON parsing errors now include the offending intent id

### tiered review queue

**Date:** 2026-06-18T05:12:47.498167+00:00

lifecycle → implemented: fixed: relates dispatch effort is derived per item from structural centrality/signals instead of hardcoded

### SQLite import export parity bridge

**Date:** 2026-06-18T05:11:33.458148+00:00

lifecycle → implemented: fixed: export paths are confined and dynamic SQL identifiers/literals are validated before interpolation

### directed handoff notes

**Date:** 2026-06-18T05:09:01.696158+00:00

lifecycle → implemented: fixed: note target and kind lookups now use indexed SQL WHERE clauses

### advisory buckets honor decision adjudication

**Date:** 2026-06-18T05:01:37.986933+00:00

cochange denom fallback to count is defensive only: repo::record_cochange_event increments individual for every file before emitting pairs, so individual always contains every paired file in production; the fallback never triggers.

### saga runner halt-on-failure semantics

**Date:** 2026-06-18T05:01:37.968909+00:00

No connect_timeout added: reqwest Client::builder().timeout(...) is a total request bound (saga/runner.rs:105-108), so an unreachable target cannot hang beyond spec.timeout_secs; a separate connect_timeout is optional hardening, not a defect.

### intent-island reachability finding

**Date:** 2026-06-18T05:01:31.677924+00:00

lifecycle → needs_change: twin-intent/duplicated-responsibility iterate all intent pairs then check abstraction level (db/queries/smells.rs ~614,~657); pre-bucketing by abstraction level avoids cross-level comparisons

### discovery queue ranks pairs by plausibility

**Date:** 2026-06-18T05:01:31.663836+00:00

lifecycle → needs_change: explored_pairs total uses (C(n,2)-hier_pairs).max(0) (db/queries/stats.rs:330-333); hierarchy anomaly masked as total=0 making coverage axis untrustworthy; undeclared-coupling nests over codefiles/imports/owners_a/owners_b and dedups after (db/queries/smells.rs ~1274-1291); dedup inside the inner loop and index intents_on_file to avoid the product

### shared symbol locator matcher

**Date:** 2026-06-18T05:01:31.649719+00:00

lifecycle → needs_change: resolved_pct computed without clamp (db/queries/symbol_accountability.rs:224-228); an invariant break could silently exceed 100

### inbox intake boundary

**Date:** 2026-06-18T05:01:31.636801+00:00

lifecycle → needs_change: list_inbox_items (~2622) and resolve_inbox_item (~2667) select all rows then filter in Rust (db/sqlite.rs); list_intents filters client-side too

### no silent fallbacks in the query layer

**Date:** 2026-06-18T05:01:31.623706+00:00

lifecycle → needs_change: list_intents_matching collects rows then map_err(Into::into) (db/sqlite.rs:1299-1318); a corrupted source_refs/tags column fails the whole query with no hint which intent

### smells propose hypotheses

**Date:** 2026-06-18T05:01:31.608333+00:00

lifecycle → needs_change: cochange_suggestions/shotgun_surgery_suggestions truncate to 15/10 (db/queries/smells.rs ~2549,~2690) before commands/smells.rs --limit; --limit>MAX never shows more

### UI state coverage via aspect

**Date:** 2026-06-18T05:01:31.595025+00:00

lifecycle → needs_change: newest_aspect_child compares child.created_at.as_str() > e (db/queries/smells.rs ~1813); nearby hypothesis code guards with RFC3339 parsing but this path does not

### tiered review queue

**Date:** 2026-06-18T05:01:31.580191+00:00

lifecycle → needs_change: next discovery/fix dispatch is hardcoded analyzer/mid and fixer/high (commands/next.rs ~208-212) despite effort being documented as structure-derived; quality already derives from rule effort

### multi-hop audit layer

**Date:** 2026-06-18T05:01:31.568535+00:00

lifecycle → needs_change: batch loops apply_line_sqlite per line (commands/batch.rs:79-97); insert_governs runs outside a transaction before update_governs_verdict opens its own, so mid-op failure can leave an edge without a verdict

### SQLite import export parity bridge

**Date:** 2026-06-18T05:01:31.556179+00:00

lifecycle → needs_change: export --check reads fs::read_to_string(root.join(out)) with no repo::confine guard (commands/export.rs:41-44); can read outside repo root; format! identifier interpolation at db/sqlite.rs:1291,1391,1406,4783/4809; identifiers from hardcoded specs today but where_clause arg is free-form

### repo introspection

**Date:** 2026-06-18T05:01:31.543158+00:00

lifecycle → needs_change: confine returns after lexical strip_prefix before canonicalizing (repo.rs ~211-244); symlinked component under root can escape undetected on macOS; fallback Python import parsing turns from .. import X into empty module (repo.rs ~625-642); tree-sitter path emits parent dir (ts_imports.rs ~243-244) but fallback does not

### saga failure diagnosis

**Date:** 2026-06-18T05:01:31.528014+00:00

lifecycle → needs_change: decode_jwt_payload returns None for any base64/JSON failure (saga/diagnose.rs:434-444); malformed bearer falls through to generic 401 instead of token-not-a-valid-JWT; jwt_scope_mismatch returns None when required scopes are satisfied (saga/diagnose.rs:230-254); resource-permission 403 misreported as scope problem

### directed handoff notes

**Date:** 2026-06-18T05:01:31.514540+00:00

lifecycle → needs_change: notes_for_target/notes_by_kind call list_all_notes then retain (db/sqlite.rs:1546-1556); runs per work-item, behind the 12032-notes read-path cost

### saga runner halt-on-failure semantics

**Date:** 2026-06-18T05:01:31.495472+00:00

lifecycle → needs_change: EnvRedactor only records env values with len>=4; short secret values leak into StepOutcome url/detail (saga/runner.rs:81 vs redaction contract at :37)

### (floating)

**Date:** 2026-06-17T15:11:24.119450+00:00

The literal loom inbox triage --take 20 appears as both the next step for a new card and the command surface for intake queue work. Keeping the exact command visible in both places is deliberate operator guidance, not separate behavior.

### (floating)

**Date:** 2026-06-17T15:11:24.039436+00:00

Door and Inbox intentionally repeat the same route-menu vocabulary for complaint and redesign cases so capture-first and triage surfaces teach identical landing choices. The phrases are shared concept language, not independent contracts; if the route taxonomy changes, update both surfaces together.

### loom: living intent graph CLI

**Date:** 2026-06-17T15:11:23.926127+00:00

Duplicate-detection blind spot accepted for this dogfood fix: 77 historical coded intents remain untagged, but the new inbox intake boundary is tagged with schema. Exhaustive vocabulary tagging is a separate graph-hygiene pass and should not block the Inbox intake/global binary validation.

### storage documentation and guide refresh

**Date:** 2026-06-17T15:10:34.145764+00:00

Documentation spread is deliberate for the storage/guide refresh intent: README, COMMANDS, CONTRIBUTING, and related docs are separate reader entry points that must carry the same SQLite/current-command story. Splitting the docs intent would hide the consistency contract this feature is meant to preserve.

### inbox intake boundary

**Date:** 2026-06-17T15:09:07.197292+00:00

visibility ruled internal during alignment

### (floating)

**Date:** 2026-06-17T15:03:08.262864+00:00

Large-symbol audit after final helper extraction: run_with_sqlite remains a single intent command dispatcher with many subcommand branches and shared gate/store setup. The actual repeated-string issues were extracted into output helpers; splitting this dispatcher safely should be a dedicated command-architecture refactor, not part of the Inbox intake boundary change.

### (floating)

**Date:** 2026-06-17T14:57:58.571168+00:00

Large-symbol audit: create_table_batch stays large for now because it is the centralized import/export table loader that keeps table clearing, property projection, list serialization, and batched insert ordering in one deterministic schema path. Splitting it safely needs a storage-import refactor with parity tests, not this Inbox intake change.

### (floating)

**Date:** 2026-06-17T14:57:58.491650+00:00

Large-symbol audit: run_relates_with_repo, render_all, and run_align stay large for now because they are rich queue/render coordinators that assemble one work item or one closeout/agenda surface from many graph read models. The current change only added Inbox intake counts and string helpers; a deeper split should be a dedicated next-command decomposition, not incidental to Inbox.

### (floating)

**Date:** 2026-06-17T14:57:58.409784+00:00

Large-symbol audit: run_with_sqlite stays as the intent command dispatcher for now because it owns one CLI enum match with many subcommand branches and shared gate/store setup. The low-risk cleanup extracted repeated user-facing strings; splitting the dispatcher is a separate command-architecture refactor, not required for the Inbox intake change.

### (floating)

**Date:** 2026-06-17T14:06:34.760432+00:00

Repeated CLI/API strings shared with next.rs are deliberate presentation contracts: the stale export warning, populate next command, and required_human_gated_debt JSON key must read identically in status/next surfaces. Keep them literal until a dedicated public-output constants pass covers both commands.

### (floating)

**Date:** 2026-06-17T14:06:34.689246+00:00

Repeated CLI/API strings shared with status.rs are deliberate presentation contracts: the stale export warning, populate next command, and required_human_gated_debt JSON key must read identically in next/status surfaces. Keep them literal until a dedicated public-output constants pass covers both commands.

### (floating)

**Date:** 2026-06-17T14:01:34.811006+00:00

required_human_gated_debt and related completion labels are stable status/next API keys. The current duplication is intentional cross-command presentation parity; extract shared constants during a schema/constants pass rather than hiding the public JSON contract in this status taxonomy change.

### (floating)

**Date:** 2026-06-17T14:01:34.739405+00:00

Repeated CLI strings in next.rs are intentionally mirrored across JSON/plain output and mode-specific guidance where exact operator wording matters: drift messages, note commands, batch template headers, export warnings, quality green-gate text, and populate commands. Extract during a focused CLI rendering constants pass.

### (floating)

**Date:** 2026-06-17T14:01:34.667922+00:00

render_all, run_align, and run_relates_with_repo are known large CLI coordinators. This change moved blocked-validation gate classification into stats.rs and only consumes the summary here; splitting the remaining queue rendering and align/discovery flows should be a focused next-command decomposition, not incidental status taxonomy work.

### (floating)

**Date:** 2026-06-17T14:01:34.596079+00:00

graph_state_from_snapshot_parts remains a single compass coordinator for now because it derives one ordered graph-state pulse from shared counts and phase gates. This change kept the new blocked-validation taxonomy in separate helpers instead of growing that coordinator; cargo test and the loom completeness validation pass.

### interface plane gap detection

**Date:** 2026-06-17T12:46:01.175540+00:00

Shotgun advisory inspected: interface plane gap detection legitimately touches next-mode routing and sqlite regression tests because the gap lane is both status-visible work and a persisted interface/CALLS audit. Remaining broad co-change is coordinator/test history, not a refactor target after the shared output helper was extracted.

### smells propose hypotheses

**Date:** 2026-06-17T12:46:01.166537+00:00

Shotgun advisory inspected after post-commit cochange shifted. smells propose hypotheses lives in the central smells detector because redesign-shaped findings must emit proof-first remedies with the detector evidence. Broad co-change reflects detector expansion history; split only through a proven smells-module hypothesis with remedy parity tests.

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-17T12:39:36.332516+00:00

Shotgun advisory inspected: dependency-cycle smell lives inside the central smells detector and naturally co-changed with many graph-intent features during audit expansion. The real centrality relationship is now grounded; remaining broad history is detector-coordinator churn, not a split/refactor target without a proven smells-module hypothesis.

### advisory buckets honor decision adjudication

**Date:** 2026-06-17T12:39:01.927533+00:00

Shotgun advisory inspected after resolving concrete advisory-bucket regressions. This intent deliberately spans src/commands/smells.rs and src/db/queries/smells.rs because advisory rendering, adjudication, and detector accounting must agree. Broad co-change is expected detector/coordinator history; split only through a proven smells-modularization hypothesis with summary/json parity tests.

### (floating)

**Date:** 2026-06-17T12:38:36.608724+00:00

populate derived graph structure is the shared purpose string for the custody/lane gate around populate writes. It intentionally repeats the lane name so errors and audit notes teach the same operation.

### (floating)

**Date:** 2026-06-17T12:38:36.539247+00:00

The repeated HTTP endpoint called by saga text is intentional interface-plane description vocabulary: saga-derived HTTP surfaces should render the same provenance wording wherever the surface is planned or created.

### (floating)

**Date:** 2026-06-17T12:38:36.470005+00:00

Remaining repeated populate strings are deliberate machine-facing vocabulary: boundary_intent_without_calls is both the internal gap-kind key and the JSON field name; changing only one would break driver/read-surface parity, and a constant alone does not remove the contract.

### (floating)

**Date:** 2026-06-17T12:36:02.811170+00:00

src/cli.rs intentionally co-locates clap command enums for the whole command surface. Splitting derive definitions by intent would add dispatch indirection and risk help/example drift; keep until a proven CLI-modularization hypothesis shows maintenance benefit.

### (floating)

**Date:** 2026-06-17T12:36:02.742258+00:00

src/db/queries/smells.rs deliberately co-locates detector implementations, teaching text, and adjudication checks so a smell's evidence/remedy/suppression semantics stay in one reviewable module. A module split should be proposed as a hypothesis with parity tests, not done only to silence tangle size.

### (floating)

**Date:** 2026-06-17T12:36:02.674444+00:00

create_table_batch is a single SQLite schema DDL literal by design: table constraints and indexes must be reviewed as one schema contract. Splitting this string would make migration drift harder to inspect without reducing runtime complexity.

### (floating)

**Date:** 2026-06-17T12:36:02.604956+00:00

teaching_for is intentionally centralized pattern text keyed by smell kind. The large match keeps remediation language discoverable and avoids drift between summary/json/human teaching; split only if a generated/table-driven form preserves exact command guidance.

### (floating)

**Date:** 2026-06-17T12:36:02.536419+00:00

compute_smells_from_parts remains a deliberately linear detector pipeline for now: it assembles heterogeneous smell families over one snapshot so summary/open/adjudicated accounting stays coherent. Split only through a proven hypothesis with parity tests for every smell family.

### loom: living intent graph CLI

**Date:** 2026-06-17T12:36:02.445397+00:00

Duplicate-detection blind spot accepted for this audit pass: 76 coded intents remain untagged, but current cleanup targeted concrete audit debt and helper duplication. Vocabulary backfill is useful future graph-taxonomy work, not a correctness blocker for the implemented code changes.

### triage queue for hypotheses

**Date:** 2026-06-17T12:27:59.661094+00:00

Shotgun advisory inspected: hypothesis triage is another next-mode surface sharing the same router/output envelope as build, quality, align, and validate. Co-change with many intents reflects central queue orchestration history; split/reown only after a supported hypothesis proves next.rs is causing maintenance pain.

### bulk quality read grouped by intent

**Date:** 2026-06-17T12:27:59.621278+00:00

Shotgun advisory inspected: bulk quality read lives in src/commands/next.rs because --take batching is part of the shared next-command envelope and batch-template contract. The broad co-change comes from queue-router evolution across modes, not from a hidden quality-specific responsibility that should be split now.

### align queue ranks user-intent drift suspicion

**Date:** 2026-06-17T12:27:59.607437+00:00

Shotgun advisory inspected after draining cochange pairs. Align ranking is intentionally owned by src/commands/next.rs plus src/db/queries/scoring.rs because it is one mode of the shared work router; real sync-to-align coupling is now grounded, and unrelated router/history churn is incidental until a proven hypothesis shows a cleaner split.

### (floating)

**Date:** 2026-06-17T12:09:13.171589+00:00

graph_state_from_snapshot_parts deliberately remains a single linear compass pipeline: it computes shared edge/status/coverage facts once, then applies one ordered phase decision table whose priority order is behaviorally significant. The function has local helper boundaries for reusable calculations (coverage/scoring/snapshot helpers and blocked-validation accounting) and regression tests pin the phase, coverage, and blocked-proof behavior. Splitting the decision table now would obscure the ordering without reducing current risk; reopen on the next edit if a cohesive helper boundary emerges.

### seed guide teaches the user interview

**Date:** 2026-06-17T12:08:51.512814+00:00

visibility ruled user_visible during alignment

### interface surface inspection commands

**Date:** 2026-06-17T12:08:51.500635+00:00

visibility ruled user_visible during alignment

### visual-confirm user-gated queue

**Date:** 2026-06-17T12:00:48.915579+00:00

visibility ruled user_visible during alignment

### loom: living intent graph CLI

**Date:** 2026-06-17T12:00:48.905202+00:00

visibility ruled user_visible during alignment

### saga steps resolve interface calls

**Date:** 2026-06-17T12:00:48.895240+00:00

visibility ruled user_visible during alignment

### reaction-driven mockup loop

**Date:** 2026-06-17T12:00:48.885440+00:00

visibility ruled user_visible during alignment

### saga consumer plane

**Date:** 2026-06-17T12:00:48.874322+00:00

visibility ruled user_visible during alignment

### hypothesis plane

**Date:** 2026-06-17T12:00:48.863405+00:00

visibility ruled user_visible during alignment

### smells propose hypotheses

**Date:** 2026-06-17T12:00:48.853362+00:00

visibility ruled user_visible during alignment

### interface surface inspection commands

**Date:** 2026-06-17T12:00:48.843027+00:00

visibility ruled user_visible during alignment

### session opener teaches the turn-zero ask

**Date:** 2026-06-17T12:00:48.830981+00:00

visibility ruled user_visible during alignment

### validate --all drains pending proofs

**Date:** 2026-06-17T12:00:48.819788+00:00

visibility ruled user_visible during alignment

### ask-the-map keyword search

**Date:** 2026-06-17T12:00:48.808628+00:00

visibility ruled user_visible during alignment

### saga run stamps the graph

**Date:** 2026-06-17T12:00:48.798440+00:00

visibility ruled user_visible during alignment

### self-teaching surface

**Date:** 2026-06-17T12:00:48.788300+00:00

visibility ruled user_visible during alignment

### seed guide teaches the user interview

**Date:** 2026-06-17T12:00:48.778108+00:00

visibility ruled user_visible during alignment

### endpoint-constrained edge storage

**Date:** 2026-06-17T12:00:48.767950+00:00

visibility ruled internal during alignment

### saga spec with first-class intent binding

**Date:** 2026-06-17T12:00:48.757633+00:00

visibility ruled internal during alignment

### SQLite graph persistence

**Date:** 2026-06-17T12:00:48.747591+00:00

visibility ruled internal during alignment

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-17T12:00:48.737345+00:00

visibility ruled internal during alignment

### CLI surface and dispatch

**Date:** 2026-06-17T12:00:48.720855+00:00

visibility ruled internal during alignment

### directed handoff notes

**Date:** 2026-06-17T12:00:48.710250+00:00

visibility ruled internal during alignment

### no silent fallbacks in the query layer

**Date:** 2026-06-17T12:00:48.699931+00:00

visibility ruled internal during alignment

### shared graph query snapshot layer

**Date:** 2026-06-17T12:00:48.689782+00:00

visibility ruled internal during alignment

### verifiable delegated coverage

**Date:** 2026-06-17T12:00:48.678650+00:00

visibility ruled internal during alignment

### stale hypothesis evidence ripples on sync

**Date:** 2026-06-17T12:00:48.667883+00:00

visibility ruled internal during alignment

### bounded tag vocabulary

**Date:** 2026-06-17T12:00:48.656299+00:00

visibility ruled internal during alignment

### SQLite direct concurrency policy

**Date:** 2026-06-17T12:00:48.646327+00:00

visibility ruled internal during alignment

### bulk quality read grouped by intent

**Date:** 2026-06-17T12:00:48.635650+00:00

visibility ruled internal during alignment

### adoption spawns outcome proof

**Date:** 2026-06-17T12:00:48.625164+00:00

visibility ruled internal during alignment

### align queue ranks user-intent drift suspicion

**Date:** 2026-06-17T12:00:48.613917+00:00

visibility ruled internal during alignment

### intent meaning evolves in place with semantic ripple

**Date:** 2026-06-17T12:00:48.603388+00:00

visibility ruled internal during alignment

### confirmation stamps freshness for drift ranking

**Date:** 2026-06-17T12:00:48.592608+00:00

visibility ruled internal during alignment

### snapshot analysis and annotation helpers

**Date:** 2026-06-17T12:00:48.581638+00:00

visibility ruled internal during alignment

### schema vocabulary and repository boundary

**Date:** 2026-06-17T12:00:48.566594+00:00

visibility ruled internal during alignment

### advisory buckets honor decision adjudication

**Date:** 2026-06-17T11:56:57.271989+00:00

Audit advisory decision: this wide co-change is accepted as incidental audit-surface history. Advisory adjudication spans smells command rendering and smell query detectors, which naturally co-change with many graph signals; no immediate split is required before continuing debt work.

### align queue ranks user-intent drift suspicion

**Date:** 2026-06-17T11:56:57.261980+00:00

Audit advisory decision: this wide co-change is accepted as intentional queue-surface churn. Align ranking shares next/scoring surfaces with many closeout and command flows; the align --take agenda change is a read-only operator surface, not evidence that align owns those unrelated behaviors.

### betweenness centrality in priority scoring

**Date:** 2026-06-17T11:56:57.246682+00:00

Audit advisory decision: this wide co-change is accepted as incidental centrality history. Betweenness/priority scoring lives in shared scoring/snapshot query files that naturally move with many queue and command surfaces; this commit did not reveal a hidden single responsibility to split before continuing debt work.

### migration cutover and rollback path

**Date:** 2026-06-17T11:55:27.664925+00:00

Audit advisory decision: migration cutover and rollback path is documentation-grounded, so its honest proof is the integration/CLI regression suite that exercises import/export, status, next --all, and SQLite read surfaces. A README-local unit test would be artificial; existing nonlocal proof covers the documented migration contract until the docs or migration behavior changes.

### (floating)

**Date:** 2026-06-17T11:55:17.485702+00:00

Audit reopen after adding validation list --result: the repeated validation resolver error string remains intentional. resolve.rs and validation.rs both expose the same user-facing not-found/ambiguous validation wording so direct resolver reuse and validation command UX stay byte-consistent.

### (floating)

**Date:** 2026-06-17T11:55:17.416476+00:00

Audit reopen after adding align --take agenda and blocked-validation closeout filter: render_all, run_align, and run_relates_with_repo remain deliberately centralized command/queue renderers for now; splitting them is separate behavior-preserving cleanup, not part of this operator-surface fix. Repeated closeout, batch-template, note-list, populate, and quality-green strings are intentional CLI affordances mirrored across JSON/human/single/bulk surfaces so operators see the same command at each point of use.

### (floating)

**Date:** 2026-06-17T11:33:21.991989+00:00

Audit reopen after blocked-validation closeout fix: render_all, run_align, and run_relates_with_repo remain deliberately centralized command-rendering/queue orchestration functions for now; splitting them is separate behavior-preserving cleanup, not required for this discrepancy fix. The repeated strings are intentional CLI affordances repeated across human and JSON/queue surfaces so the same commands and warnings stay visible at each point of use.

### (floating)

**Date:** 2026-06-17T11:29:09.288985+00:00

Audit reopen after deterministic co-change ordering edit: compute_smells_from_parts and teaching_for remain deliberately centralized detector/teaching tables for now because splitting them would be a separate behavior-preserving refactor, not required for the audit drain. The repeated symbol-accountability teaching strings are intentionally mirrored between smells.rs and symbol_accountability.rs so both audit surfaces teach the same anti-overmapping rule at the point of failure.

### (floating)

**Date:** 2026-06-17T11:14:57.704365+00:00

Audit advisory decision: the short pub fn run shims in doctor/report/status are deliberately independent command entry wrappers with coincidental identical shape. They should remain local to each command module; no shared abstraction is required for the audit lane.

### migration cutover and rollback path

**Date:** 2026-06-17T11:14:37.438306+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### schema vocabulary and repository boundary

**Date:** 2026-06-17T11:14:37.427940+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### graph travel format

**Date:** 2026-06-17T11:14:37.417797+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### (floating)

**Date:** 2026-06-17T11:14:36.520512+00:00

Audit decision: repeated status/next command strings are intentional user-facing affordances kept local to queue/status renderers. Centralizing this copy is optional UX cleanup, not required for the audit lane.

### (floating)

**Date:** 2026-06-17T11:14:36.452898+00:00

Audit decision: smells::render deliberately remains a single report renderer that assembles summary/detail/adjudicated/advisory sections in output order. Splitting this renderer is presentation refactor work, not required for the audit lane.

### seed guide teaches the user interview

**Date:** 2026-06-17T10:36:13.907133+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### staged want-to-physical seed ladder

**Date:** 2026-06-17T10:36:13.895883+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### auto-stub the acceptance validation at the contract stage

**Date:** 2026-06-17T10:36:13.883711+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### bulk quality read grouped by intent

**Date:** 2026-06-17T10:36:13.873947+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### transitive layering-violation detection

**Date:** 2026-06-17T10:36:13.863334+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### UI state coverage via aspect

**Date:** 2026-06-17T10:36:13.852050+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### intent-island reachability finding

**Date:** 2026-06-17T10:36:13.842650+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### mockup is contract not realization

**Date:** 2026-06-17T10:36:13.832670+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-17T10:36:13.822484+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### visibility captured at seed time

**Date:** 2026-06-17T10:36:13.813090+00:00

Audit advisory decision: recurring wide co-change is accepted as incidental history for this coordinator/audit teaching surface. The broad changes came from repo-wide graph/schema/guide/audit evolution, not a hidden single responsibility that should be split before leaving the audit lane.

### saga spec with first-class intent binding

**Date:** 2026-06-17T10:36:07.840321+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### migration cutover and rollback path

**Date:** 2026-06-17T10:36:07.830233+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### graph travel format

**Date:** 2026-06-17T10:36:07.819946+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### saga runner halt-on-failure semantics

**Date:** 2026-06-17T10:36:07.810082+00:00

Audit advisory decision: existing proof is accepted for this intent even though the linked validation resolves outside the grounded module. The current validation exercises the behavior at command/query or integration altitude, and adding module-local proof is optional future hardening rather than an audit blocker.

### storage documentation and guide refresh

**Date:** 2026-06-17T10:35:31.173334+00:00

Audit decision: storage documentation and guide refresh intentionally spans README, docs, and guide command text because it is the operator-facing documentation slice for the storage migration. The spread is deliberate for this audit.

### schema vocabulary and repository boundary

**Date:** 2026-06-17T10:35:31.163596+00:00

Audit decision: schema vocabulary and repository boundary is intentionally cross-cutting across schema constants, typed graph records, repository open/read helpers, and command consumers. Splitting it is future taxonomy work, not an audit blocker.

### interface plane gap detection

**Date:** 2026-06-17T10:35:31.154043+00:00

Audit decision: interface plane gap detection deliberately spans the inspect command, populate/status surfacing, and regression tests. The files are one detection/reporting workflow, not fragmented ownership for this audit.

### computed graph population lane

**Date:** 2026-06-17T10:35:31.142829+00:00

Audit decision: computed graph population lane deliberately spans command entry points, repository/schema storage helpers, and regression tests because the behavior is an end-to-end lifecycle. The spread is expected for the populate lane and should re-open only on new groundings.

### command definitions and dispatch

**Date:** 2026-06-17T10:35:31.132378+00:00

Audit decision: command definitions and dispatch deliberately spans cli.rs, main entry wiring, command module registration, and command docs/tests. This is one dispatch surface; splitting the intent would be taxonomy redesign, not required for this audit.

### loom: living intent graph CLI

**Date:** 2026-06-17T10:35:31.122010+00:00

no consumer surface: this repo's current user-visible surface is a local CLI/LLM operator loop, not an HTTP product journey exercised by loom saga. CLI behavior is covered through command tests, validations, and graph verdicts; require saga coverage only when a real consumer HTTP/API journey exists.

### loom: living intent graph CLI

**Date:** 2026-06-17T10:35:31.109430+00:00

Audit decision: duplicate-detection tag coverage is intentionally not backfilled across all coded intents in this audit. The registered vocabulary remains storage/migration scoped; adding broad product/workflow tags is a taxonomy lane and blindly tagging legacy intents would create misleading duplicate signals.

### (floating)

**Date:** 2026-06-17T10:34:38.147651+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:38.057709+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.991684+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.926921+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.860830+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.794515+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.728526+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.662301+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.595627+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.527972+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.462760+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.394704+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.327499+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.262204+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.195306+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.130894+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:37.062695+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.996837+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.932335+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.864380+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.798882+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.733129+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.666148+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.599543+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.533989+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.466056+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.395625+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.324997+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.257691+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.187620+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.123391+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:36.058205+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.992064+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.925945+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.860485+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.795321+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.721034+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.654406+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.588481+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.522395+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### (floating)

**Date:** 2026-06-17T10:34:35.456007+00:00

Audit decision: current file-level smell findings are accepted for this audit. This file deliberately acts as a command/query/schema/documentation coordinator or carries local user-facing contract text; extracting helpers or splitting ownership would be redesign work, not required to finish the audit lane. Re-open on the next edit to this file.

### shared symbol locator matcher

**Date:** 2026-06-17T10:12:10.288754+00:00

boundary changed: 'inbound' -> '<internal>' (resolve interface-plane gap: shared symbol matching is internal graph-analysis implementation, not an externally callable surface, so the boundary facet was misleading)

### shared nonempty string push helper

**Date:** 2026-06-17T10:12:05.715374+00:00

boundary changed: 'inbound' -> '<internal>' (resolve interface-plane gap: this is an internal static-analysis utility helper, not an inbound endpoint or outbound client call)

### shared command entity resolvers

**Date:** 2026-06-17T10:12:01.887009+00:00

boundary changed: 'inbound' -> '<internal>' (resolve interface-plane gap: shared resolver helpers are internal command implementation plumbing, not callable external surfaces, so they should not require CALLS edges)

### saga runner halt-on-failure semantics

**Date:** 2026-06-17T10:11:36.674115+00:00

boundary changed: 'outbound' -> '<internal>' (resolve interface-plane gap: the saga runner performs outbound HTTP during execution, but this intent is the internal executor semantics for ordering and failure reporting; individual saga steps provide CALLS bindings, not this runner implementation intent)

### advisory buckets honor decision adjudication

**Date:** 2026-06-17T10:11:33.018537+00:00

boundary changed: 'inbound' -> '<internal>' (resolve interface-plane gap: this is an internal audit/rendering behavior under derived problem signals, not an externally callable endpoint or client call, so it should not require a CALLS binding)

### CLI surface and dispatch

**Date:** 2026-06-17T10:11:27.165920+00:00

boundary changed: 'inbound' -> '<internal>' (resolve interface-plane gap: this component describes the CLI grammar/dispatch as product contract, but the current CALLS plane is populated from saga HTTP steps and cannot truthfully bind a broad CLI component; concrete callable CLI behavior should be modeled by command-specific proofs rather than a fake CALLS edge)

### advisory buckets honor decision adjudication

**Date:** 2026-06-17T02:23:58.023022+00:00

lifecycle → implemented: advisory buckets now honor current decision notes and report suppressed advisories under adjudicated output

### advisory buckets honor decision adjudication

**Date:** 2026-06-17T02:23:57.891092+00:00

spawned from hypothesis 'clones carry an adjudicated disposition' (44f0e229-2a0b-4a0e-bd52-2401e3fd4ac2) - predicted outcome (acceptance contract): loom smells reports each code_clone's disposition plus a roll-up like 'N clones — D deliberate, H tracked, O open'; a decision note anchored to a clone's shape_hash moves it out of open and re-opens when sync changes that hash; the raw code_clones_total never drops below the number of duplications physically present on disk

### (floating)

**Date:** 2026-06-17T02:23:57.891092+00:00

adopted: spawned 'advisory buckets honor decision adjudication' (936cf0e6-342e-4599-8d6c-92fb4e3901c8) - User ruled that informational advisory buckets should be suppressible through note adjudication; implemented the supported clone-disposition concern across all advisory buckets.

### shared nonempty string push helper

**Date:** 2026-06-17T01:47:34.479814+00:00

lifecycle → implemented: centralized duplicated push_unique helper in vec_utils and replaced repo/ts_imports local copies

### (floating)

**Date:** 2026-06-17T01:46:44.872523+00:00

adopted: spawned 'shared nonempty string push helper' (82336903-25ba-4f43-95b6-88fb70cfc3c1) - Supported proof found identical push_unique helpers in repo.rs and ts_imports.rs; adopt as a small shared utility refactor.

### shared nonempty string push helper

**Date:** 2026-06-17T01:46:44.872523+00:00

spawned from hypothesis 'push_unique duplicated across import modules' (fb4bdd5c-88db-4d27-9598-06e1d3af6666) - predicted outcome (acceptance contract): either a single definition imported by both modules and the code_clone group collapses to one file, or a recorded deliberate-copies disposition that stops it reading as an open clone

### shared symbol locator matcher

**Date:** 2026-06-17T01:44:57.701616+00:00

lifecycle → implemented: centralized symbol locator matching and updated coverage plus symbol-accountability callers

### shared symbol locator matcher

**Date:** 2026-06-17T01:44:01.594066+00:00

spawned from hypothesis 'symbol-identifier helpers duplicated across layers' (3bfa2d6f-8234-4316-aa81-362431bbe3a1) - predicted outcome (acceptance contract): one definition of each helper shared by both call sites; the two code_clone groups for these symbols collapse to one file; coverage's symbol-accountability output is unchanged

### (floating)

**Date:** 2026-06-17T01:44:01.594066+00:00

adopted: spawned 'shared symbol locator matcher' (cfcbfb63-c60e-4ff3-b9c1-0d2058d921e8) - Supported proof found duplicated symbol identifier matching across coverage and symbol-accountability query code; adopt as a shared matcher refactor.

### (floating)

**Date:** 2026-06-17T01:43:24.543387+00:00

rejected: Refuted against current code: mutating commands do not write loom.graph.json; export freshness is checked and explicit loom export is required.

### shared command entity resolvers

**Date:** 2026-06-17T01:39:17.640951+00:00

lifecycle → implemented: centralized duplicated command entity resolver helpers and replaced local copies in edge/rule/intent/persona/note command modules

### (floating)

**Date:** 2026-06-17T01:37:32.409898+00:00

adopted: spawned 'shared command entity resolvers' (616970c1-9a3a-4a92-bbf2-50b3bae2d9fd) - Supported proof found duplicated command resolver contracts across intent, note, rule, edge, and persona command modules; adopt as a focused shared-resolver refactor.

### shared command entity resolvers

**Date:** 2026-06-17T01:37:32.409898+00:00

spawned from hypothesis 'command resolvers copy-pasted across modules' (1f673868-272c-48cf-8afd-240aa812414c) - predicted outcome (acceptance contract): one definition of each resolver; the code_clone groups for resolve_intent_with_db and resolve_validation_with_db collapse to a single location each; ambiguity and not-found CLI messages are byte-identical to today (existing command tests stay green)

### hypothesis prove records TARGETS confidence

**Date:** 2026-06-17T01:35:16.234037+00:00

lifecycle → implemented: hypothesis prove now requires explicit confidence and stamps that value onto TARGETS rows; CLI/help/tests cover the new contract

### (floating)

**Date:** 2026-06-17T01:24:00.460548+00:00

adopted: spawned 'hypothesis prove records TARGETS confidence' (f3a922b5-11ba-4bd8-80c9-9f1762e96b54) - Supported proof showed current prove path leaves passing TARGETS edges at confidence 0.0, which violates doctor integrity; adopt as a focused integrity fix.

### hypothesis prove records TARGETS confidence

**Date:** 2026-06-17T01:24:00.460548+00:00

spawned from hypothesis 'prove stamps TARGETS verdicts without confidence' (a11b9aec-b2f2-4584-a01e-5b3910628111) - predicted outcome (acceptance contract): a freshly added then proven hypothesis has its TARGETS edges at the prover's stated confidence, never 0.0; loom doctor stays healthy after any prove; no future loom export trips the sqlite_imported_export_read_surface regression on leaked-default TARGETS edges

### external interface surface plane

**Date:** 2026-06-17T00:42:20.849832+00:00

lifecycle → implemented: all planned feature leaves for schema, saga registration, and inspection commands are implemented

### interface surface inspection commands

**Date:** 2026-06-17T00:42:20.834466+00:00

lifecycle → implemented: added loom interface list/show and CLI dispatch/schema help for interface surface inspection

### saga steps resolve interface calls

**Date:** 2026-06-17T00:42:20.823307+00:00

lifecycle → implemented: saga add now resolves each HTTP step to an interface surface and records ordered CALLS edges

### interface surface schema vocabulary

**Date:** 2026-06-17T00:42:20.808569+00:00

lifecycle → implemented: implemented InterfaceSurface/CALLS schema, typed models, SQLite persistence, export/import, and round-trip test

### smells propose hypotheses

**Date:** 2026-06-16T20:26:33.059803+00:00

v3: the code_clone ADVISORY remedy + teaching now also emit 'loom hypothesis add' as the third disposition (real dup, deferred → tracked refactor proposal), extending v2's gating-smell coverage to the advisory tier. Unlike the gating smells, code_clone is recomputed from shape_hash and reads no notes, so the routing only files the work — honoring a deliberate-copies ruling on a clone (anchor on shape_hash, annotate-don't-suppress) is tracked separately as hypothesis 'clones carry an adjudicated disposition' (44f0e229).

### (floating)

**Date:** 2026-06-16T20:20:56.696854+00:00

adopted: spawned 'external interface surface plane' (fcf2f089-6dbe-46f0-8296-d50512420ff8), 'interface surface schema vocabulary' (bf39f175-67f8-4800-844a-68d7b7b34bdc), 'saga steps resolve interface calls' (f05e3b28-0cfb-4294-b5ad-739cb2dfdfc4), 'interface surface inspection commands' (ebb7441b-bb63-4908-80bf-9c8cefe2c198) - The supported claim shows the current saga-only model cannot answer interface-level ownership, journey coverage, or quality-rule questions; adopt as a planned interface surface plane with schema, saga-binding, and inspection work.

### external interface surface plane

**Date:** 2026-06-16T20:20:56.696854+00:00

spawned from hypothesis 'first-class interface surface plane' (cb1a109a-5b4d-44ce-98e5-b5cd35ecf783) - predicted outcome (acceptance contract): The schema/model exposes an interface surface node and CALLS-style edge; saga add registers or resolves HTTP surfaces from each step; saga list/show or a dedicated query can report every surface, its implementing/owning intent, and journey coverage; service quality rules can be verdicted against surfaces without treating a full saga as the endpoint.

### interface surface inspection commands

**Date:** 2026-06-16T20:20:56.696854+00:00

spawned from hypothesis 'first-class interface surface plane' (cb1a109a-5b4d-44ce-98e5-b5cd35ecf783) - predicted outcome (acceptance contract): The schema/model exposes an interface surface node and CALLS-style edge; saga add registers or resolves HTTP surfaces from each step; saga list/show or a dedicated query can report every surface, its implementing/owning intent, and journey coverage; service quality rules can be verdicted against surfaces without treating a full saga as the endpoint.

### interface surface schema vocabulary

**Date:** 2026-06-16T20:20:56.696854+00:00

spawned from hypothesis 'first-class interface surface plane' (cb1a109a-5b4d-44ce-98e5-b5cd35ecf783) - predicted outcome (acceptance contract): The schema/model exposes an interface surface node and CALLS-style edge; saga add registers or resolves HTTP surfaces from each step; saga list/show or a dedicated query can report every surface, its implementing/owning intent, and journey coverage; service quality rules can be verdicted against surfaces without treating a full saga as the endpoint.

### saga steps resolve interface calls

**Date:** 2026-06-16T20:20:56.696854+00:00

spawned from hypothesis 'first-class interface surface plane' (cb1a109a-5b4d-44ce-98e5-b5cd35ecf783) - predicted outcome (acceptance contract): The schema/model exposes an interface surface node and CALLS-style edge; saga add registers or resolves HTTP surfaces from each step; saga list/show or a dedicated query can report every surface, its implementing/owning intent, and journey coverage; service quality rules can be verdicted against surfaces without treating a full saga as the endpoint.

### multi-language static analysis coverage

**Date:** 2026-06-16T13:17:58.188954+00:00

lifecycle → implemented: All child language-support intents are implemented and grounded to repo/codefile static-analysis helpers; roll-up now represents the delivered multi-language static-analysis coverage component.

### Swift static analysis support

**Date:** 2026-06-16T13:17:27.428169+00:00

lifecycle → implemented: Implemented via dependency-light repo static-analysis heuristics and covered by the multi-language heuristic static analysis validation.

### Svelte and Bun project support

**Date:** 2026-06-16T13:17:27.419220+00:00

lifecycle → implemented: Implemented via dependency-light repo static-analysis heuristics and covered by the multi-language heuristic static analysis validation.

### Kotlin static analysis support

**Date:** 2026-06-16T13:17:27.409607+00:00

lifecycle → implemented: Implemented via dependency-light repo static-analysis heuristics and covered by the multi-language heuristic static analysis validation.

### Go static analysis support

**Date:** 2026-06-16T13:17:27.400452+00:00

lifecycle → implemented: Implemented via dependency-light repo static-analysis heuristics and covered by the multi-language heuristic static analysis validation.

### Dart and Flutter static analysis support

**Date:** 2026-06-16T13:17:27.390759+00:00

lifecycle → implemented: Implemented via dependency-light repo static-analysis heuristics and covered by the multi-language heuristic static analysis validation.

### multi-hop audit layer

**Date:** 2026-06-16T05:53:01.983565+00:00

lifecycle → implemented: Rollup: all active children built (betweenness, reciprocal-dependency, intent-island, transitive layering); structure_version cache consciously deferred (not pending).

### structure_version cache for topology-keyed metrics

**Date:** 2026-06-16T05:53:01.964214+00:00

Safe future design when this is built: derive structure_version as a CONTENT HASH of the structural facts (node ids + edge endpoints + layer + lifecycle, EXCLUDING inspection_status) so cache invalidation is automatic and the 'never bump on inspection flip' property is free — no manual bump-site that could be missed and stale the audit. Cache the metrics in the DB keyed by that hash.

### structure_version cache for topology-keyed metrics

**Date:** 2026-06-16T05:53:01.941777+00:00

lifecycle → deferred: Deferred (not superseded): premature optimization for current scale. Topology metrics (cycles/islands/centrality) compute in microseconds at ~90 intents, and the snapshot load — paid before the version key can even be computed — dominates. Revisit when large/federated scale makes the metrics dominate.

### (floating)

**Date:** 2026-06-16T04:37:25.979426+00:00

Deliberate-for-now coordinator: smells.rs is the single derived-signal engine — each detector is a numbered section over ONE shared snapshot + adjudication helper + SmellReport, so they cohabit by design. The file is now ~3.2k lines; a real future option is extracting detector sections into submodules (track via a hypothesis if it impedes change). Accepted as cohabitation for now.

### (floating)

**Date:** 2026-06-16T04:37:25.946422+00:00

Deliberate coordinator module: guide.rs hosts every driving-mode playbook (greenfield/brownfield/refactor/port/seed) as one self-teaching surface sharing the render contract and ratchet tests. The seed mode legitimately gained several owners (the seed-flow + visual-register guidance leaves); cohabitation is by design. Split a mode out only if its playbook grows independently complex.

### UI/UX visual-register seed flow

**Date:** 2026-06-16T04:35:17.762325+00:00

lifecycle → implemented: Rollup: all children (reaction loop, mockup-contract, UI-state, design-system, visual-confirm) built+validated

### intent-spectrum seed-flow guidance

**Date:** 2026-06-16T04:35:17.748360+00:00

lifecycle → implemented: Rollup: all children (staged ladder, visibility-at-seed, auto-stub) built+validated

### transitive layering-violation detection

**Date:** 2026-06-16T04:34:46.026359+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### design-system standards via QualityRule packs

**Date:** 2026-06-16T04:34:45.983880+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### UI state coverage via aspect

**Date:** 2026-06-16T04:34:45.941987+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### visual-confirm user-gated queue

**Date:** 2026-06-16T04:34:45.897582+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### mockup is contract not realization

**Date:** 2026-06-16T04:34:45.849755+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### reaction-driven mockup loop

**Date:** 2026-06-16T04:34:45.801978+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### auto-stub the acceptance validation at the contract stage

**Date:** 2026-06-16T04:34:45.754852+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### visibility captured at seed time

**Date:** 2026-06-16T04:34:45.704297+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### staged want-to-physical seed ladder

**Date:** 2026-06-16T04:34:45.635522+00:00

lifecycle → implemented: Built+tested this turn; grounded to code, proven by the named cargo test

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-15T17:39:50.381056+00:00

Reframed directed-SCC -> reciprocal-pair (both directions grounded). SCC over non-canonicalized RELATES_TO produced a non-actionable 39-intent blob on loom's own graph; the reciprocal grounded pair is the only honest, actionable circular signal. Uninspected saga path edges are excluded so legitimate round-trip journeys never false-gate (adversarial-review finding). DEFERRED: a genuine saga-flow cycle detector needs a stored saga-edge provenance marker (saga add writes plain uninspected edges today); build that increment if/when sagas are used. RETIREMENT: if RELATES_TO storage is ever canonicalized at insert (sort from<to), reciprocal pairs become impossible and this detector should be retired, not left as a zero-yield gate.

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-15T17:37:55.231988+00:00

redefined: Reframed from directed-SCC to reciprocal-pair: an SCC over non-canonicalized RELATES_TO produced a non-actionable 39-intent blob; reciprocal grounded pairs are the only honestly-useful actionable signal, and excluding uninspected saga edges prevents false-gating legitimate round-trip journeys
was: loom smells reports one finding per strongly-connected component of size greater than one in the RELATES_TO graph (a circular intent dependency), naming its members; an acyclic grid reports none

### intent-island reachability finding

**Date:** 2026-06-15T17:01:37.351663+00:00

lifecycle → implemented: Built and tested: graph_algo.rs algorithms with unit tests + scoring/smells integration tests; cargo build/clippy/fmt/test clean on default and --no-default-features

### dependency-cycle smell over RELATES_TO

**Date:** 2026-06-15T17:01:37.338848+00:00

lifecycle → implemented: Built and tested: graph_algo.rs algorithms with unit tests + scoring/smells integration tests; cargo build/clippy/fmt/test clean on default and --no-default-features

### graded decaying sync ripple beyond one hop

**Date:** 2026-06-15T17:01:37.327866+00:00

lifecycle → implemented: Built and tested: graph_algo.rs algorithms with unit tests + scoring/smells integration tests; cargo build/clippy/fmt/test clean on default and --no-default-features

### betweenness centrality in priority scoring

**Date:** 2026-06-15T17:01:37.315876+00:00

lifecycle → implemented: Built and tested: graph_algo.rs algorithms with unit tests + scoring/smells integration tests; cargo build/clippy/fmt/test clean on default and --no-default-features

### seed guide teaches the user interview

**Date:** 2026-06-15T11:42:56.805837+00:00

Accepted direction as a coherent proposal set: SQLite typed-table substrate enables bounded multi-hop audit; intent-spectrum stays on existing planes/facets with seed-flow guidance rather than a new Intent subtype; UI/UX is the visual-register specialization of that seed flow. Amendment: an HTML mockup is a repo-native CodeFile and contract/source/proof target, but it must not satisfy production IMPLEMENTS for a screen unless the intent is explicitly a prototype/mockup/Storybook artifact; production screen intents remain planned until real app code grounds them.

### self-teaching surface

**Date:** 2026-06-15T11:19:54.413834+00:00

reworded: self-teaching audit made lifecycle coverage explicit
was: guide/schema/orientation embed the full driving protocol in the binary so a cold LLM needs no external docs

### (floating)

**Date:** 2026-06-15T10:46:45.403148+00:00

src/commands/find.rs intentionally keeps the repository-backed find call and its JSON/human renderer in one small command module. The render helper is accepted under the find surface for this audit; no storage-backend debt remains in the file.

### (floating)

**Date:** 2026-06-15T10:46:45.364278+00:00

src/commands/door.rs intentionally keeps landing constants, doctrine copy, repository-backed search, and rendering together as one user-facing door surface. The backend boundary only owns run_with_db access; broad constants/render ownership is accepted here rather than split into artificial micro-intents.

### (floating)

**Date:** 2026-06-15T10:46:45.329665+00:00

src/commands/saga.rs intentionally keeps saga add/run/list, SQLite persistence, validation stamping, and step-resolution helpers together because they form one command workflow. Broad symbol ownership is accepted for this audit; split only if a proven redesign shows the workflow is fighting maintenance.

### (floating)

**Date:** 2026-06-15T10:46:45.295146+00:00

src/db/sqlite.rs intentionally co-locates typed SQLite schema, import/export, mutation helpers, read snapshots, and constraint tests while the backend stabilizes. This is accepted as the single storage module for the replacement audit; a later modular split should be hypothesis/proof driven.

### SQLite query and search implementation

**Date:** 2026-06-15T10:46:45.241522+00:00

SQLite query and search implementation intentionally spans command entry points plus shared snapshot/query helpers for this audit. The responsibility is coherent as the read-surface cutover contract; finer child intents are useful taxonomy debt, not a blocker for replacing the storage backend.

### backend-neutral storage boundary

**Date:** 2026-06-15T10:46:45.230263+00:00

The backend-neutral storage boundary is deliberately broad for the replacement audit: it records the command-to-repository contract across read/list surfaces after the SQLite cutover. Split it into narrower read-command/write-command/storage children in a later graph-design pass; broad grounding is accepted here because the source already routes through concrete symbols and all validations pass.

### loom: living intent graph CLI

**Date:** 2026-06-15T10:46:45.214663+00:00

SQLite replacement audit accepts the current duplicate-detection blind spot as non-blocking: the registered vocabulary is deliberately migration/storage-scoped, and backfilling legacy product/workflow tags would create misleading duplicate signals. A separate taxonomy pass should add broader terms before claiming a whole-repo duplicate audit is clean.

### (floating)

**Date:** 2026-06-15T10:33:09.888232+00:00

retired: superseded by direct embedded SQLite; automatic daemon routing and socket management were removed, while serve now fails with a retirement message - replaced by intent 6cc267b6-6052-48bd-b914-674fabe48eee

### SQLite direct concurrency policy

**Date:** 2026-06-15T10:33:09.265849+00:00

redefined: SQLite fully replaced the daemon-backed Grafeo runtime; the graph must describe direct SQLite concurrency, not an automatic daemon layer
was: SQLite WAL/busy-timeout behavior, direct CLI access, daemon routing, stale-daemon replacement, and hard-kill durability are specified and tested so the daemon remains a performance layer rather than a correctness crutch.

### SQLite direct concurrency policy

**Date:** 2026-06-15T10:33:09.265849+00:00

renamed: 'SQLite concurrency and daemon policy' -> 'SQLite direct concurrency policy' (SQLite fully replaced the daemon-backed Grafeo runtime; the graph must describe direct SQLite concurrency, not an automatic daemon layer)

### endpoint-constrained edge storage

**Date:** 2026-06-15T10:28:42.293507+00:00

renamed: 'endpoint-matched edge queries' -> 'endpoint-constrained edge storage' (SQLite cutover replaced query-workaround semantics with endpoint-constrained relational storage)

### endpoint-constrained edge storage

**Date:** 2026-06-15T10:28:42.293507+00:00

redefined: SQLite cutover replaced query-workaround semantics with endpoint-constrained relational storage
was: RELATES_TO/HIERARCHY/IMPLEMENTS/GOVERNS/VALIDATES queries — edges keyed by endpoint nodes, never by their own properties (grafeo 0.5.x reliability rule)

### snapshot analysis and annotation helpers

**Date:** 2026-06-15T10:28:42.278085+00:00

redefined: SQLite cutover removed CRUD query modules; the remaining query layer is snapshot analysis, not storage CRUD
was: CRUD for Intent/CodeFile/QualityRule/Validation/Note/Ignore/Meta nodes plus shared row extraction; reliable node-anchored paths only

### snapshot analysis and annotation helpers

**Date:** 2026-06-15T10:28:42.278085+00:00

renamed: 'node and annotation queries' -> 'snapshot analysis and annotation helpers' (SQLite cutover removed CRUD query modules; the remaining query layer is snapshot analysis, not storage CRUD)

### schema vocabulary and repository boundary

**Date:** 2026-06-15T10:28:42.256582+00:00

redefined: SQLite cutover removed the old session trait; the intent now covers vocabulary plus the typed repository boundary
was: single-source schema declarations (labels/edges/props/owners/version), GQL escaping, the LoomDb trait and long-lived grafeo session, shared type structs

### schema vocabulary and repository boundary

**Date:** 2026-06-15T10:28:42.256582+00:00

renamed: 'schema vocabulary and session' -> 'schema vocabulary and repository boundary' (SQLite cutover removed the old session trait; the intent now covers vocabulary plus the typed repository boundary)

### SQLite graph persistence

**Date:** 2026-06-15T10:28:42.232197+00:00

redefined: SQLite fully replaced the legacy graph backend, so the storage component meaning must match the active implementation
was: embedded grafeo DB behind a LoomDb trait; one long-lived session; all edge queries endpoint-matched or scan+filter because grafeo 0.5.x cannot match relationships by their own property

### SQLite graph persistence

**Date:** 2026-06-15T10:28:42.232197+00:00

renamed: 'graph persistence on grafeo' -> 'SQLite graph persistence' (SQLite fully replaced the legacy graph backend, so the storage component meaning must match the active implementation)

### SQLite-backed graph persistence migration

**Date:** 2026-06-15T10:22:44.330361+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### validation and saga storage isolation

**Date:** 2026-06-15T10:22:44.320202+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### sync ripple indexed update path

**Date:** 2026-06-15T10:22:44.309946+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### storage responsibility vocabulary coverage

**Date:** 2026-06-15T10:22:44.299470+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### storage documentation and guide refresh

**Date:** 2026-06-15T10:22:44.287932+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### storage contract regression suite

**Date:** 2026-06-15T10:22:44.276008+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### migration cutover and rollback path

**Date:** 2026-06-15T10:22:44.265020+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### backend-neutral storage boundary

**Date:** 2026-06-15T10:22:44.249699+00:00

lifecycle → implemented: SQLite source cutover completed: Grafeo dependency and legacy query files removed, active commands use SQLite/read snapshots, docs updated, cargo check and cargo test pass, and target/debug/loom successfully synced/routed the live SQLite graph. Global binary remains untouched until explicit promotion.

### sync ripple indexed update path

**Date:** 2026-06-15T10:21:49.001438+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: sync invalidates IMPLEMENTS, RELATES_TO, GOVERNS, TARGETS, and VALIDATES from indexed storage state with deterministic cause notes and without per-edge GQL fan-out inherited from Grafeo.

### storage documentation and guide refresh

**Date:** 2026-06-15T10:21:48.991238+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: README, guide, command docs, daemon design, and build/install guidance describe SQLite storage accurately and remove obsolete Grafeo-only lock/query caveats once cutover is complete.

### storage contract regression suite

**Date:** 2026-06-15T10:21:48.981264+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: Backend tests prove storage semantics independent of Grafeo: transactions, endpoint uniqueness, list round trips, free-text binding, constraints, FTS/search behavior, import rejection, and snapshot reads.

### migration cutover and rollback path

**Date:** 2026-06-15T10:21:48.970967+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: Users can keep using the global Grafeo-backed Loom graph while building the SQLite binary, create scratch graphs from export, back up live state, migrate deliberately, and roll back through committed loom.graph.json if parity fails.

### backend-neutral storage boundary

**Date:** 2026-06-15T10:21:48.960383+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: Command handlers and query consumers depend on typed Loom storage/repository operations and transaction helpers, not on GrafeoDb, grafeo::QueryResult, grafeo::Value, or backend query strings.

### SQLite-backed graph persistence migration

**Date:** 2026-06-15T10:21:48.946791+00:00

reworded: SQLite cutover terminology clarified after removing the legacy backend
was: Replace Loom's Grafeo-shaped live graph persistence with typed SQLite storage while preserving graph semantics, deterministic export/import, global-vs-local command parity, validation/saga honesty, daemon/performance guarantees, and auditability through Loom itself.

### (floating)

**Date:** 2026-06-15T10:20:51.011817+00:00

retired: superseded after full SQLite source cutover: the legacy global-vs-local parity harness and deleted sqlite_parity test no longer describe the active gate; SQLite regression tests plus the local dogfood validation now prove the replacement - replaced by intent 5567df9f-375b-4615-9cbc-499d36ea61a0

### migration cutover and rollback path

**Date:** 2026-06-15T08:18:27.283139+00:00

Global loom promotion is gated: keep using target/debug/loom against .loom/graph.sqlite until the 'local SQLite dogfood gate before global install' validation passes after the current cleanup slice. Do not run cargo install --path . as the global replacement before this gate is green.

### backend-neutral storage boundary

**Date:** 2026-06-15T05:13:33.197476+00:00

The backend-neutral storage boundary is intentionally broad during the SQLite spike because it tracks one cross-command migration contract: commands and query consumers should move behind typed repository operations while Grafeo remains the control backend. Split this into read-command, write-command, storage-schema, and daemon-cutover child intents only when the default backend cutover is approved; until then the wide grounding is a planning umbrella, not a code ownership claim that each file must be moved now.

### loom: living intent graph CLI

**Date:** 2026-06-15T05:13:24.296821+00:00

The current intent tag vocabulary is deliberately scoped to the SQLite/storage migration spike: storage, query, schema, sqlite, parity, migration, export, and daemon. Legacy coded intents outside that vocabulary should not be backfilled with misleading storage tags; add broader product/workflow terms in a separate taxonomy pass before using tags for whole-repo duplicate detection.

### (floating)

**Date:** 2026-06-15T04:46:14.385937+00:00

The scale benchmark harness intentionally has one file-level owner for its synthetic graph edge helper functions add_hier and add_impl; both are internal helpers for generating benchmark exports, so separate product intents would be artificial.

### SQLite direct concurrency policy

**Date:** 2026-06-14T19:58:11.340542+00:00

lifecycle → implemented: Daemon policy and SQLite WAL concurrency are grounded and covered by targeted daemon/WAL validations; daemon remains a performance layer with direct/fallback paths preserving correctness.

### SQLite query and search implementation

**Date:** 2026-06-14T19:56:24.457022+00:00

lifecycle → implemented: All stated SQLite read command surfaces are wired through the SQLite backend, parity-covered against the installed global Grafeo binary, and release-benchmarked equal-or-faster across the full next/read surface.

### (floating)

**Date:** 2026-06-14T10:54:35.776845+00:00

lifecycle → implemented: tests/sqlite_parity.rs compares installed global loom with target/debug/loom on isolated scratch graphs for stable read JSON and deterministic mutation/export parity.

### SQLite import export parity bridge

**Date:** 2026-06-14T10:54:35.705947+00:00

lifecycle → implemented: SqliteGraphStore imports committed loom.graph.json and exports semantically equivalent JSON; validated by sqlite_import_export_parity.

### typed SQLite graph schema

**Date:** 2026-06-14T10:54:35.629114+00:00

lifecycle → implemented: src/db/sqlite.rs now creates typed node/edge tables with FK, uniqueness, CHECK constraints, JSON-list columns, indexes, and FTS; validated by sqlite_schema_contract.

### (floating)

**Date:** 2026-06-14T10:54:23.821989+00:00

reworded: avoid claiming SQLite-backed scratch parity before the live SQLite command path exists
was: The installed Grafeo-backed loom remains the planning/control binary while target/debug/loom is compared on scratch SQLite graphs for structured JSON reads, mutation sequences, and export diffs before any live cutover.

### SQLite graph persistence

**Date:** 2026-06-14T10:16:37.082801+00:00

Keep this implemented Grafeo component as the baseline/control harness during migration. Retire or replace it only after the SQLite planned component has passed parity and cutover validations; marking it needs_change now would make the global control graph less honest.

### SQLite-backed graph persistence migration

**Date:** 2026-06-14T10:16:37.048959+00:00

Do not flatten Loom into an opaque generic node/edge blob store. The planned SQLite model remains graph-shaped but typed: separate node tables and typed edge tables with FK, endpoint uniqueness, CHECK status constraints, queue indexes, JSON-list fields, and FTS where search needs it.

### SQLite-backed graph persistence migration

**Date:** 2026-06-14T10:16:37.014270+00:00

Migration operating rule: installed/global Grafeo-backed loom remains the planning and control graph. The local target/debug/loom SQLite candidate works only on scratch graphs imported from the same loom.graph.json until read parity, mutation parity, export parity, and rollback/cutover validations pass.

### sync ripple indexed update path

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### typed SQLite graph schema

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### SQLite query and search implementation

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### storage responsibility vocabulary coverage

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### migration cutover and rollback path

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### storage documentation and guide refresh

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### (floating)

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### SQLite-backed graph persistence migration

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### (floating)

**Date:** 2026-06-14T10:16:34.353053+00:00

adopted: spawned 'SQLite-backed graph persistence migration' (c2f6bca0-2ccd-48b7-af47-1a74fba61441), 'backend-neutral storage boundary' (6a41dcbc-4035-4d08-8315-0d77803ca862), 'typed SQLite graph schema' (829c54ad-de0c-4505-b0c4-55116fcfd621), 'SQLite import export parity bridge' (4d16b78b-0ace-4827-b041-f6d5031e16f4), 'global local command parity harness' (6f4dc39e-0179-4ed9-9e8c-5eff49545022), 'storage contract regression suite' (5567df9f-375b-4615-9cbc-499d36ea61a0), 'SQLite concurrency and daemon policy' (6cc267b6-6052-48bd-b914-674fabe48eee), 'validation and saga storage isolation' (95a32c01-480b-4d59-8633-fc249cd9a526), 'SQLite query and search implementation' (2bd1db29-7898-4800-a450-3dc084409197), 'sync ripple indexed update path' (87d4fe6a-2331-4376-bb9f-172b24eefda7), 'migration cutover and rollback path' (63b49d1c-a2fa-4598-aa3d-07ebd761ccdd), 'storage documentation and guide refresh' (0401f1f2-443f-42c0-b833-b113f1633299), 'storage responsibility vocabulary coverage' (667e3b6c-cea2-4ab1-83cb-e0911c80efa9) — Supported by storage-boundary audit and SQLite spike; convert into a planned migration spine with explicit parity and cutover gates.

### SQLite import export parity bridge

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### validation and saga storage isolation

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### backend-neutral storage boundary

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### storage contract regression suite

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### SQLite direct concurrency policy

**Date:** 2026-06-14T10:16:34.353053+00:00

spawned from hypothesis 'sqlite-backed graph persistence' (b834c7ab-1140-4c85-b2d9-b4f2e137d7dc) — predicted outcome (acceptance contract): A local SQLite loom binary imports the current export, matches the global Grafeo-backed binary on structured read and mutation parity over scratch graphs, enforces FK/unique/status constraints in storage tests, and removes Grafeo-specific query/lock workarounds from command/query code.

### (floating)

**Date:** 2026-06-14T09:28:49.307220+00:00

rejected: Superseded by the implemented automatic JSON daemon path: servable JSON automation no longer uses default direct cross-process opens, while explicit direct opt-out remains available for debugging and can be handled later as a lower-priority teaching polish.

### (floating)

**Date:** 2026-06-14T09:26:53.997239+00:00

lifecycle → implemented: Implemented automatic daemon use for servable JSON graph commands with explicit LOOM_DAEMON opt-out.

### saga runner halt-on-failure semantics

**Date:** 2026-06-14T03:24:44.417106+00:00

boundary changed: '<internal>' → 'outbound' (the executor issues HTTP requests to the external service under test via reqwest (rustls) — loom acts as a client crossing into the outside world. The system's outbound edge.)

### CLI surface and dispatch

**Date:** 2026-06-14T03:24:43.389402+00:00

boundary changed: '<internal>' → 'inbound' (loom's published contract to its LLM driver: the CLI grammar IS the API the outside world calls, and every output is the agent's next prompt. The system's inbound edge.)

### hostile import rejected loudly

**Date:** 2026-06-13T10:00:17.517562+00:00

visibility ruled internal during alignment

### repo introspection

**Date:** 2026-06-13T10:00:07.887552+00:00

visibility ruled internal during alignment

### triage queue for hypotheses

**Date:** 2026-06-13T09:59:58.215281+00:00

visibility ruled internal during alignment

### intent retirement contract

**Date:** 2026-06-13T09:59:48.567986+00:00

visibility ruled internal during alignment

### hypothesis lifecycle commands

**Date:** 2026-06-13T09:59:38.954511+00:00

visibility ruled internal during alignment

### tiered review queue

**Date:** 2026-06-13T09:59:29.349460+00:00

visibility ruled internal during alignment

### intents addressable by name, not only uuid

**Date:** 2026-06-13T09:59:19.728792+00:00

visibility ruled internal during alignment

### porting mode: import --as-planned

**Date:** 2026-06-13T09:59:09.727793+00:00

visibility ruled internal during alignment

### proof and bootstrap handlers

**Date:** 2026-06-13T09:59:00.087592+00:00

visibility ruled internal during alignment

### self-teaching surface

**Date:** 2026-06-13T09:58:50.277978+00:00

visibility ruled user_visible during alignment

### role lanes and evidence gates

**Date:** 2026-06-13T09:58:40.708769+00:00

visibility ruled internal during alignment

### scale: hot commands bounded on large graphs

**Date:** 2026-06-13T09:58:30.871314+00:00

visibility ruled internal during alignment

### bulk grounding via glob

**Date:** 2026-06-13T09:58:21.270010+00:00

visibility ruled internal during alignment

### endpoint-constrained edge storage

**Date:** 2026-06-13T09:58:11.603977+00:00

visibility ruled internal during alignment

### derived problem signals

**Date:** 2026-06-13T09:58:02.062383+00:00

visibility ruled internal during alignment

### graph travel format

**Date:** 2026-06-13T09:57:52.349710+00:00

visibility ruled internal during alignment

### priority-scored work queues

**Date:** 2026-06-13T09:57:41.727013+00:00

visibility ruled internal during alignment

### discovery queue ranks pairs by plausibility

**Date:** 2026-06-13T09:57:32.110690+00:00

visibility ruled internal during alignment

### graph-write command handlers

**Date:** 2026-06-13T09:57:22.526151+00:00

visibility ruled internal during alignment

### sync flag engine

**Date:** 2026-06-13T09:57:12.835778+00:00

visibility ruled internal during alignment

### hypothesis node and TARGETS edge

**Date:** 2026-06-13T09:57:03.241407+00:00

visibility ruled internal during alignment

### dual-mode output

**Date:** 2026-06-13T09:56:53.637233+00:00

visibility ruled internal during alignment

### completeness and integrity checking

**Date:** 2026-06-13T09:56:44.050716+00:00

visibility ruled internal during alignment

### graph targeting pin

**Date:** 2026-06-13T09:56:34.400496+00:00

visibility ruled internal during alignment

### command definitions and dispatch

**Date:** 2026-06-13T09:56:24.798185+00:00

visibility ruled internal during alignment

### schema vocabulary and repository boundary

**Date:** 2026-06-13T09:56:15.121776+00:00

visibility ruled internal during alignment

### snapshot analysis and annotation helpers

**Date:** 2026-06-13T09:56:05.163370+00:00

visibility ruled internal during alignment

### (floating)

**Date:** 2026-06-12T05:37:48.271979+00:00

Stats queries intentionally co-locate completeness pulse, scale-sensitive counts, and seed-guide status because they share graph-state aggregation and coverage-vector calculations; this is a query snapshot concern, not separate feature logic.

### (floating)

**Date:** 2026-06-12T05:37:48.177461+00:00

Guide command intentionally carries seed, porting, and self-teaching protocol text together so the binary emits one coherent operator playbook; splitting prose by intent would make the user-facing guide harder to audit.

### (floating)

**Date:** 2026-06-12T05:37:48.086675+00:00

Intent query helpers deliberately co-locate confirmation freshness, meaning updates, name resolution, and fallback-free resolver behavior because they operate on the same Intent node vocabulary and ripple rules.

### (floating)

**Date:** 2026-06-12T05:37:47.995711+00:00

Next queue handling deliberately co-locates align, quality bulk reads, review, and triage because they share queue rendering, graph-state context, and candidate ranking output; the file is a queue dispatcher rather than unrelated business logic.

### (floating)

**Date:** 2026-06-12T05:37:47.896396+00:00

Intent command handling deliberately co-locates confirm, meaning update, retirement, and write-handler glue because they share one CLI enum, one resolver path, and one semantic-ripple transaction surface; splitting now would add dispatch churn without reducing coupling.

### schema vocabulary and repository boundary

**Date:** 2026-06-12T05:37:47.795968+00:00

Schema vocabulary deliberately spans src/db/schema.rs, src/types.rs, src/db/mod.rs, and src/commands/migrate.rs: declarations, shared Rust types, session boundary, and live-schema upgrade are one contract and splitting the intent would hide the migration dependency.

### stale hypothesis evidence ripples on sync

**Date:** 2026-06-11T07:54:37.389258+00:00

Gap closed post-11d2ad4: sync flipped TARGETS edges but nothing ROUTED them — stale support rotted silently. triage_candidates now also serves supported hypotheses with needs_reverification TARGETS as RE-PROVE items; re-proving re-stamps the edges. Regression: stale_target_support_routes_back_to_triage.

### (floating)

**Date:** 2026-06-11T07:36:23.569534+00:00

retired: The narrower conversion-only work item is superseded by the broader adopted flow that also creates and verifies the outcome proof. — replaced by intent 809761fa-9de9-4b00-b3e4-eda65efb65ab

### (floating)

**Date:** 2026-06-11T07:36:23.489583+00:00

retired: Transient coupling-refactor work completed; the enduring implementation now lives in the shared graph query snapshot layer and derived problem signals. — replaced by intent be7990bd-7f82-40d9-8064-c72c52c0a04c

### (floating)

**Date:** 2026-06-11T07:36:23.408156+00:00

retired: Transient ranking refactor completed; the enduring implementation now lives in the shared graph query snapshot layer and discovery queue behavior. — replaced by intent be7990bd-7f82-40d9-8064-c72c52c0a04c

### (floating)

**Date:** 2026-06-11T07:36:23.328300+00:00

retired: Transient threading work completed; the stable architectural outcome is the shared graph query snapshot layer. — replaced by intent be7990bd-7f82-40d9-8064-c72c52c0a04c

### (floating)

**Date:** 2026-06-11T07:36:23.247261+00:00

retired: Transient performance refactor completed; the enduring behavior now lives inside the stable sync flag engine component. — replaced by intent 29799603-3704-4dfa-9ba4-387a7c1942f8

### shared graph query snapshot layer

**Date:** 2026-06-11T07:11:37.505494+00:00

lifecycle → implemented: QuerySnapshot and DiscoverySnapshot now serve status/report, next scoring helpers, smells, and doctor's inspectable-edge audit through one bulk-loaded read surface.

### (floating)

**Date:** 2026-06-11T07:09:58.144937+00:00

lifecycle → implemented: derived problem signals now reuse a discovery snapshot backed by sync-refreshed codefile imports

### (floating)

**Date:** 2026-06-11T07:09:57.928974+00:00

lifecycle → implemented: unexplored pair ranking now reuses a shared discovery snapshot for token, ownership, and import inputs

### (floating)

**Date:** 2026-06-11T07:09:57.720183+00:00

lifecycle → implemented: status/report orientation work now threads one QuerySnapshot through graph_state, validate_selection, and normative_coverage

### (floating)

**Date:** 2026-06-11T07:09:57.446579+00:00

lifecycle → implemented: sync now drives ripple invalidation from bulk indexes and deduplicates affected edges and validations per run

### shared graph query snapshot layer

**Date:** 2026-06-11T06:55:38.142435+00:00

spawned from hypothesis 'unified graph query snapshot layer' (a098fe93-96db-470a-9592-dd0c66acf9da) — predicted outcome (acceptance contract): read-heavy commands share one bulk graph projection per invocation family, making further performance work simpler and reducing repeated table scans across command handlers.

### (floating)

**Date:** 2026-06-11T06:55:38.142435+00:00

adopted: spawned 'shared graph query snapshot layer' (be7990bd-7f82-40d9-8064-c72c52c0a04c) — supported claim maps to a planned read-heavy query-layer refactor across command surfaces

### (floating)

**Date:** 2026-06-11T06:55:37.946054+00:00

spawned from hypothesis 'incremental coupling index' (58e226d1-82c5-4a47-9edd-3394c4473071) — predicted outcome (acceptance contract): smells and related coupling checks reuse sync-produced adjacency data and large-graph smell latency improves without weakening findings.

### (floating)

**Date:** 2026-06-11T06:55:37.946054+00:00

adopted: spawned 'incremental coupling snapshot' (2ddcf18d-9c6c-4205-bf98-81cf7fa7fad6) — supported claim maps to a planned sync-maintained coupling snapshot for smell computation

### (floating)

**Date:** 2026-06-11T06:53:07.474654+00:00

adopted: spawned 'discovery ranking snapshot' (0961299d-781d-4860-9064-128c6aa60bd0) — supported claim maps to a planned reusable ranking-snapshot refactor for optional discovery

### (floating)

**Date:** 2026-06-11T06:53:07.474654+00:00

spawned from hypothesis 'snapshot discovery ranking inputs' (ac503cbb-0de1-496b-87e6-b0de766b0fa9) — predicted outcome (acceptance contract): discovery commands rank unexplored pairs from precomputed inputs and large-graph discovery latency drops materially without changing ranking semantics.

### (floating)

**Date:** 2026-06-11T06:53:07.272045+00:00

adopted: spawned 'bulk-indexed sync ripple' (344b406f-c0db-466f-b987-e30bf44322ab) — supported claim maps to a planned sync refactor that bulk-indexes remaining ripple classes

### (floating)

**Date:** 2026-06-11T06:53:07.272045+00:00

spawned from hypothesis 'bulk-index sync ripple pass' (592c2ff5-db8c-4406-8664-93c93d5984e6) — predicted outcome (acceptance contract): sync touches each relevant ripple edge or validation at most once per run and large multi-file syncs avoid per-intent query fan-out.

### (floating)

**Date:** 2026-06-11T06:53:07.062089+00:00

spawned from hypothesis 'thread graph-state snapshots' (b4d474e7-4a79-4405-ba43-ebd8d1c1ee1f) — predicted outcome (acceptance contract): status/next/report load each bulk graph slice once per command and the 2000-intent orientation benchmark improves without changing output semantics.

### (floating)

**Date:** 2026-06-11T06:53:07.062089+00:00

adopted: spawned 'graph-state snapshot threading' (4c656525-4851-4f2f-bdcd-25a3c08522d4) — supported claim maps to a planned read-only snapshot refactor spanning graph_state-oriented helpers

### stale hypothesis evidence ripples on sync

**Date:** 2026-06-11T04:51:12.813360+00:00

lifecycle → implemented: Sync now stales passing TARGETS edges for changed target-intent code and reports the count.

### adoption spawns outcome proof

**Date:** 2026-06-11T04:51:12.630555+00:00

lifecycle → implemented: Adoption now creates a not_run manual validation from predicted_outcome, links it to spawned intents, and validation mark passed confirms the hypothesis.

### smells propose hypotheses

**Date:** 2026-06-11T03:16:16.457096+00:00

lifecycle → implemented: v2 shipped: recurrent_trouble, tangled_file, twin_intents (merge case), and scattered_intent (code-change case) remedies emit loom hypothesis add

### triage queue for hypotheses

**Date:** 2026-06-11T03:16:16.420938+00:00

lifecycle → implemented: v2 shipped: next --mode triage serves proposed hypotheses by combined target centrality (analyzer, effort high, optional); next --all lists the queue flagged optional

### hypothesis plane

**Date:** 2026-06-11T02:01:48.046088+00:00

lifecycle → implemented: v1 core loop shipped (propose/prove/decide); v2 routing and v3 outcome-proof slices remain planned children

### (floating)

**Date:** 2026-06-11T02:01:48.011250+00:00

lifecycle → implemented: v1 shipped: adopt requires supported status + spawned intents or a conversion reason; lineage decision notes on both ends carry the predicted outcome as acceptance contract

### hypothesis lifecycle commands

**Date:** 2026-06-11T02:01:47.975862+00:00

lifecycle → implemented: v1 shipped: loom hypothesis add/target/prove/adopt/reject/list/show with evidence gates, analyzer/builder lanes, proposer-not-prover, sequence gating, transition notes

### hypothesis node and TARGETS edge

**Date:** 2026-06-11T02:01:47.931283+00:00

lifecycle → implemented: v1 shipped: Hypothesis label + TARGETS edge in schema (additive, stays v3), queries with round-trip tests, export/import tolerance for older exports, doctor audit

### hypothesis plane

**Date:** 2026-06-11T01:43:55.904232+00:00

Design decision: Hypothesis is a dedicated 5th node type, not Intent.status=proposed with a confirm gate. The dual-proof loop justifies it: the problem is proven against code BEFORE adoption (analyzer, triage queue), and the predicted_outcome is proven AFTER implementation (validator, via a spawned Validation). Boundary: hypothesis = pre-decision speculation, intent = decided work; adoption is the conversion point and speculation never counts in coverage/completeness. Slices: v1 core loop (schema v4, lifecycle commands, adoption-as-conversion), v2 routing (triage queue, smells emit hypothesis remedies), v3 closing the loop (outcome proof spawning, TARGETS sync ripple).

### porting mode: import --as-planned

**Date:** 2026-06-10T16:26:51.161695+00:00

Hardening campaign C7: porting = the semantic plane travels, the physical plane is rebuilt — criteria written for the old code become the acceptance contract for the new; do after A-items

### scale: hot commands bounded on large graphs

**Date:** 2026-06-10T16:26:51.136080+00:00

Hardening campaign (June 10 2026), phase A2 — sequenced after A1 (coherence-by-construction, shipped) and B5 (loom batch, shipped); Meridian repos will hit graph-size limits first

### priority-scored work queues

**Date:** 2026-06-10T15:33:38.164895+00:00

recurrent_trouble (2 past regressions) reviewed and refuted as actionable: the prior regressions were earlier dogfood rounds — notably the O(N^2) discovery-ranking hotspot, which was already resolved by decomposing the ranking into the child intent 'discovery queue ranks pairs by plausibility'. Current state: intent confirmed/implemented, all 5 RELATES_TO passing, all GOVERNS green (perf bounded-work + no-redundant-work just re-verified passing this sweep), validation 'build-altitude and queue tests' passed. The design is sound at present; no redesign needed. The signal is historical memory, not a live defect.

### graph travel format

**Date:** 2026-06-10T13:56:34.195776+00:00

lifecycle → implemented: import_graph now validates export nodes/edges shape and field types explicitly instead of defaulting malformed JSON; regression test added.

### graph travel format

**Date:** 2026-06-10T13:55:15.307991+00:00

lifecycle → needs_change: import_graph silently defaults malformed export JSON fields to empty strings or zeroes, allowing corrupt graph travel files to import without error.

### repo introspection

**Date:** 2026-06-10T13:41:23.724960+00:00

lifecycle → implemented: repo::walk_files and repo::detect now return Result and propagate filesystem walk errors through detect, guide, and coverage callers; cargo test passes.

### repo introspection

**Date:** 2026-06-10T13:40:14.888196+00:00

lifecycle → needs_change: walk_files silently drops ignore::WalkBuilder filesystem errors, so detect/coverage can underreport repo evidence without surfacing incomplete introspection.

### derived problem signals

**Date:** 2026-06-10T13:32:41.627227+00:00

lifecycle → implemented: compute_smells now reports malformed CodeFile.imports JSON with context instead of suppressing undeclared-coupling findings; regression test added.

### derived problem signals

**Date:** 2026-06-10T13:32:07.897831+00:00

lifecycle → needs_change: compute_smells silently treats malformed CodeFile.imports JSON as an empty import list, suppressing undeclared-coupling findings instead of reporting graph evidence corruption.

### priority-scored work queues

**Date:** 2026-06-10T13:28:36.609408+00:00

lifecycle → implemented: build_candidates now reuses the already-fetched intents vector instead of issuing a second identical list_intents query; cargo test passes.

### priority-scored work queues

**Date:** 2026-06-10T13:28:07.869303+00:00

lifecycle → needs_change: build_candidates repeats the same list_intents query within one queue computation instead of reusing the intents vector it already fetched.

### priority-scored work queues

**Date:** 2026-06-10T13:25:27.509883+00:00

lifecycle → implemented: Discovery scoring now reports malformed CodeFile.imports JSON with contextual errors instead of defaulting to an empty import set; regression test added.

### priority-scored work queues

**Date:** 2026-06-10T13:24:38.434803+00:00

lifecycle → needs_change: Discovery scoring silently treats malformed CodeFile.imports JSON as an empty import list, which can hide coupling evidence and alter queue priority instead of reporting graph-data corruption.

### sync flag engine

**Date:** 2026-06-10T13:20:59.497715+00:00

lifecycle → implemented: Sync now reuses the bytes read for hashing to derive UTF-8 text content and loads IMPLEMENTS edges once for locator checks, removing duplicated file reads and repeated list scans.

### sync flag engine

**Date:** 2026-06-10T13:20:02.212790+00:00

lifecycle → needs_change: Sync duplicates full-file I/O by reading each codefile once as bytes for hashing and again as text for import/locator checks; unreadable-text handling can also repeat list_all_implements.

### snapshot analysis and annotation helpers

**Date:** 2026-06-10T13:03:25.432585+00:00

lifecycle → implemented: Fixed source_refs parsing to propagate malformed JSON errors and added regression coverage.

### snapshot analysis and annotation helpers

**Date:** 2026-06-10T13:01:32.887239+00:00

lifecycle → needs_change: Quality verdict iso5055-rel-no-unchecked-failure: source_refs JSON parse errors are swallowed with unwrap_or_default in add/remove source ref paths.

### bulk grounding via glob

**Date:** 2026-06-09T20:17:03.721582+00:00

lifecycle → implemented: edge implement/unimplement accept globs over registered paths; used to drive the component decompositions

### CLI surface and dispatch

**Date:** 2026-06-09T20:14:29.811400+00:00

lifecycle → implemented: decomposed into 3 cohesive children (definitions+dispatch, write handlers, proof+bootstrap); groundings moved down

### SQLite graph persistence

**Date:** 2026-06-09T20:14:29.742501+00:00

lifecycle → implemented: decomposed into 4 cohesive children (schema/session, node queries, edge queries, travel format); groundings moved down

### completeness and integrity checking

**Date:** 2026-06-09T20:08:18.910368+00:00

lifecycle → implemented: count_unexplored_pairs: arithmetic C(n,2) minus linked unordered pairs; graph_state no longer pays O(N^2) scoring per pulse; cross-check test pins count == enumeration

### completeness and integrity checking

**Date:** 2026-06-09T20:07:12.079615+00:00

lifecycle → needs_change: iso5055-perf-no-redundant-work failing: graph_state pays full O(N^2) scoring on every pulse just to count unexplored pairs — add a cheap arithmetic counter in scoring.rs and use it in stats.rs

### intents addressable by name, not only uuid

**Date:** 2026-06-09T20:03:14.765638+00:00

lifecycle → implemented: resolve_intent/resolve_rule: exact id → exact name → unique fragment; wired into every intent/rule-taking command; ambiguity errors list candidates

### discovery queue ranks pairs by plausibility

**Date:** 2026-06-09T19:57:05.034661+00:00

lifecycle → implemented: shipped in 5c98e98/32f3c44: suspicion = import links (5x) + shared files (3x) + description overlap + domain; why displayed in the work item

### CLI surface and dispatch

**Date:** 2026-06-09T19:31:07.825735+00:00

lifecycle → needs_change: loom smells: scattered across 12 files — decompose into child intents per command family

### SQLite graph persistence

**Date:** 2026-06-09T19:31:07.796416+00:00

lifecycle → needs_change: loom smells: scattered across 17 files — decompose into child intents per query concern (node queries / edge queries / scoring / health)

### CLI surface and dispatch

**Date:** 2026-06-09T19:11:27.457007+00:00

lifecycle → implemented: validate.rs now releases the DB session while commands run (read → drop → run → reopen → persist)

### CLI surface and dispatch

**Date:** 2026-06-09T19:10:33.851381+00:00

lifecycle → needs_change: loom validate holds the grafeo DB lock (one long-lived session) while running validation commands as subprocesses; any validation that itself invokes loom (e.g. 'loom status --json') fails with GRAFEO-X001 lock error. Fix: read everything first, RELEASE the connection, run commands, reopen to persist results.


<!-- loom:prose-start -->









<!-- loom:prose-end -->
