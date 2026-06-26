---
type: reference
title: "Components & code"
tags:
  - analysis
  - audit
  - cli
  - concurrency
  - core
  - db
  - developer-experience
  - docs
  - graph-integrity
  - health
  - navigation
  - operations
  - repo
  - static-analysis
  - sync
  - teaching
  - testing
  - trust
  - unknown
  - validation
  - workflow
---

## Components & code

Intents grouped by domain, with where each is grounded in code.

### analysis

- **Dart and Flutter static analysis support** — loom extracts Dart imports and declarations and recognizes Flutter conventions such as pubspec.yaml, lib, test, integration_test, and generated Dart files so Flutter apps get useful static-analysis signals  `src/commands/codefile.rs`, `src/repo.rs`
- **Go static analysis support** — loom extracts Go imports, packages, top-level declarations, test files, and module paths so Go repositories get coverage, coupling, symbol-accountability, sync-narrowing, clone, and smell signals  `src/commands/codefile.rs`, `src/repo.rs`
- **Kotlin static analysis support** — loom extracts Kotlin package imports, declarations, test conventions, and Android or JVM layout signals so Kotlin repositories receive language-aware coverage, coupling, and smell analysis  `src/commands/codefile.rs`, `src/repo.rs`
- **Svelte and Bun project support** — loom recognizes Bun projects and extracts useful Svelte component module/script symbols and imports so Svelte and Bun repositories get coverage, coupling, and smell signals instead of degrading to file-only tracking  `src/commands/codefile.rs`, `src/repo.rs`
- **Swift static analysis support** — loom extracts Swift imports, declarations, test conventions, and Swift package or Apple app layout signals so iOS, macOS, and Swift package repositories receive language-aware static-analysis coverage  `src/commands/codefile.rs`, `src/repo.rs`
- **betweenness centrality in priority scoring** — loom next ranking incorporates bridge (betweenness) centrality so a low-degree chokepoint intent can outrank a high-degree leaf, computed in Rust over the snapshot rather than via SQL recursion  `src/db/queries/scoring.rs`, `src/db/queries/snapshot.rs`
- **dependency-cycle smell over RELATES_TO** — loom smells reports one finding per RECIPROCAL RELATES_TO pair: two active intents where BOTH directed rows (a->b and b->a) carry a grounded verdict (not uninspected, not independent). RELATES_TO is semantically undirected and its stored direction is not canonicalized, so a long directed cycle is typing-order noise (the old SCC reading produced a vacuous 39-intent blob on loom's own graph); a reciprocal grounded pair is the one honest signal - the same relationship stored twice, double-counting in centrality and able to carry disagreeing verdicts. Mechanically-created uninspected saga path edges are excluded so a round-trip journey never false-gates. A graph with no reciprocal grounded pair reports none.  `src/db/queries/smells/consumer.rs`, `src/db/queries/smells/graph_tests.inc`
- **graded decaying sync ripple beyond one hop** — loom sync flips only direct one-hop RELATES_TO neighbors of a changed file's intents to needs_reverification, while two and three hop neighbors receive a decaying priority bump that does not change their inspection_status  `src/commands/sync.rs`, `src/db/queries/scoring.rs`
- **intent-island reachability finding** — loom smells reports any intent subgraph with no path to a system-level root through HIERARCHY and RELATES_TO as an island; a fully connected graph reports none  `src/db/queries/smells/consumer.rs`
- **multi-hop audit layer** — Derived audit signals computed over arbitrary-depth traversals of the intent and import graph (RELATES_TO cycles, transitive layering violations, intent islands, bridge centrality), surfaced via smells and graph_state without putting heavy traversal on the per-turn hot path  `src/db/queries/graph_algo.rs`
- **multi-language static analysis coverage** — loom extends language-aware import, symbol, test, generated-file, and project detection beyond Rust, TypeScript, JavaScript, and Python so popular stacks get first-class static analysis while the graph remains language-agnostic
- **structure_version cache for topology-keyed metrics** — topology-keyed heavy metrics (cycles, islands, centrality) are cached and stamped with a structure_version that bumps on node or edge add and remove and on layer or lifecycle change, but never on an inspection_status flip
- **transitive layering-violation detection** — the layering check flags an up-the-order dependency that is clean at every single hop but forms an illegal direction across multiple hops, going beyond the direct-import check  `src/db/queries/smells.rs`, `src/db/queries/smells/coupling.rs`

### audit

- **advisory buckets honor decision adjudication** — Advisory smell buckets move current decision-note rulings out of open advisory counts and into adjudicated output, with reopen anchors on the relevant intent or file.  `src/commands/smells.rs`, `src/db/queries/smells.rs`, `src/db/queries/smells/advisory_tests.inc`
- **storage responsibility vocabulary coverage** — The migration seeds and applies a small bounded vocabulary for storage, query, export, daemon, schema, parity, and migration work so duplicated responsibility and ownership drift are detectable while the backend is being rewritten.  `docs/sqlite-multihop-proposal.md`, `src/commands/vocab.rs`

### cli

- **CLI surface and dispatch** — clap-derive command definitions and dispatch to command handlers; bare invocation prints orientation
- **UI state coverage via aspect** — each screen component carries an aspect per UI state (populated, empty, loading, error) so happy_path_only flags a component that has a populated child but no empty or error sibling  `src/db/queries/smells.rs`, `src/db/queries/stats.rs`
- **UI/UX visual-register seed flow** — The seed ladder specialised for the visual register: reaction-driven with an HTML mockup as the reaction surface and contract, mockup-as-CodeFile that never satisfies production IMPLEMENTS, and machine-first verification with a human visual-confirm queue  `docs/ui-ux-flow-proposal.md`
- **align queue ranks user-intent drift suspicion** — loom next --mode align serves active intents ranked by (1+churn-since-confirm) x ln(1+degree) + age/90 where churn counts sync-flip transition notes on the intent's edges newer than its last confirm stamp; the item presents the meaning for a user interview with exactly four outcome commands (confirm/update/retire/add); validator lane, effort mid, dispatch names the align queue  `src/commands/next/align.rs`, `src/db/queries/scoring.rs`
- **auto-stub the acceptance validation at the contract stage** — the contract stage writes a not_run manual_check Validation VALIDATES-linked to the want intent so the want is falsifiable by default, mirroring the hypothesis-adoption predicted-outcome stub  `src/commands/guide.rs`
- **bulk grounding via glob** — edge implement <intent> 'src/db/**' should ground every matching registered file in one call, mirroring codefile add's glob support; dogfood finding: 48 files needed 48 invocations  `src/commands/edge.rs`
- **bulk quality read grouped by intent** — loom next --mode quality --take N returns compact GOVERNS work items in one call, grouped by intent so one code-neighborhood read pays for every rule held against it, with prefilled rule_verdict batch-template lines (existing criterion kept, placeholder evidence forced through the gates) and per-item effort from the rule's annotation; capped at 50; symmetric with loom batch's rule_verdict write  `src/commands/next/quality.rs`
- **command definitions and dispatch** — clap-derive CLI surface, dispatch to handlers, bare-invocation orientation  `build.rs`, `src/cli.rs`, `src/commands/mod.rs`, `src/main.rs`
- **computed graph population lane** — Loom computes brownfield and schema-upgrade graph-population work, then backfills interface surfaces and CALLS from existing saga specs without changing product-code lifecycle.  `src/commands/next.rs`, `src/commands/populate.rs`, `src/db/sqlite/edge_writes.rs`, `tests/sqlite_regression.rs`
- **confirmation stamps freshness for drift ranking** — loom intent confirm ratifies status AND appends a kind=confirm note (the freshness event); last_confirmed_at returns the newest stamp or None; events are append-only so alignment history travels in the export with no schema field  `src/commands/intent.rs`, `src/db/queries/scoring.rs`
- **design-system standards via QualityRule packs** — UI standards such as accessibility, contrast, touch targets and responsive breakpoints are seeded from rule seed web-ui and mobile and GOVERN the screen and component intents rather than being invented per screen  `src/commands/rule.rs`
- **dual-mode output** — every command renders human-readable text or --json including a graph_state pulse  `src/commands/tour.rs`, `src/commands/wiki.rs`, `src/output.rs`
- **graph-write command handlers** — handlers that mutate the graph: intent/edge/codefile/validation/note/rule/ignore plus export-import restoration; all lane-gated  `src/commands/batch.rs`, `src/commands/codefile.rs`, `src/commands/export.rs`, `src/commands/ignore.rs`, `src/commands/import.rs`, `src/commands/intent.rs`, `src/commands/note.rs`, `src/commands/rule.rs`, `src/commands/validation.rs`
- **intent meaning evolves in place with semantic ripple** — loom intent update rewrites an intent's name/description in place (same node, same history); a description change ripples one hop like sync but for meaning: passing/independent RELATES_TO and GOVERNS, passing IMPLEMENTS and TARGETS go needs_reverification and linked proofs go not_run (blocked keeps its reason); the old wording is preserved in a decision note; a name-only change ripples nothing; deprecated intents are rejected  `src/commands/intent.rs`, `src/db/sqlite/writes.rs`
- **intent-spectrum seed-flow guidance** — loom guide --mode seed stages a want-to-contract-to-logic-to-physical ladder so a driver populates the intent register implicitly through question order, with visibility captured at seed time, rather than via a new Intent subtype  `src/commands/seed.rs`
- **intents addressable by name, not only uuid** — every command that takes an intent id should also accept its unique name (or unambiguous prefix); dogfood finding: the driver had to maintain an external name-to-id map across the whole session  `src/db/queries/intent.rs`
- **interface plane gap detection** — Loom detects gaps in the already-populated InterfaceSurface/CALLS plane, including uncalled surfaces, boundary intents without CALLS bindings, and CALLS edges missing matching VALIDATES edges.  `src/commands/interface.rs`, `src/commands/next/render.rs`, `src/commands/populate.rs`, `tests/sqlite_regression.rs`
- **interface surface inspection commands** — Expose CLI reads that list and inspect interface surfaces, showing method/path or equivalent identity, owning intent or implementation grounding, saga callers, validation state, and quality-rule verdict coverage.  `src/cli.rs`, `src/commands/interface.rs`, `src/commands/mod.rs`
- **mockup is contract not realization** — a production screen intent source_refs its HTML mockup and stays lifecycle=planned; the mockup never creates an IMPLEMENTS edge for it, and only an explicit prototype or Storybook intent may IMPLEMENTS a mockup  `src/commands/guide.rs`
- **proof and bootstrap handlers** — loom validate (runs proofs with the session released) and loom init (idempotent bootstrap)  `src/commands/init.rs`, `src/commands/validate.rs`
- **reaction-driven mockup loop** — for a user_visible screen, loom guidance generates an HTML mockup as the reaction surface and converts each human reaction into graph deltas, looping until reactions stop changing structure  `src/commands/guide.rs`
- **seed guide teaches the user interview** — loom guide --mode seed is explicit-only (never auto-detected) and teaches both loops: elicit (altitude calibrated to user fluency, one question per landing, recommended answers, terminate on enumerable gaps not exhaustion) and align (drive loom next --mode align outcomes); an empty graph's compass routes phase=seed pointing at this guide  `docs/intent-spectrum-proposal.md`, `src/commands/guide.rs`, `src/db/queries/stats.rs`
- **shared command entity resolvers** — Shared resolver helpers provide one id/name/fragment lookup contract for command modules instead of copy-pasted local resolve_intent_with_db and resolve_validation_with_db bodies.  `src/commands/resolve.rs`
- **staged want-to-physical seed ladder** — loom guide --mode seed elicits in four ordered stages — want (a user_visible intent plus persona), contract (a validation), logic (internal child intents), physical (implements to code) — one stage and one question at a time  `src/commands/guide.rs`
- **status surfaces populate gaps** — Loom status exposes computed populate work, including interface-plane gaps and the next populate command, so an LLM starting from status can see graph population gaps that need resolution.  `src/commands/status.rs`, `tests/sqlite_regression.rs`
- **validate --all drains pending proofs** — loom validate --all runs every validation whose last_result is not_run (never run or sync-invalidated) in one verb with the same three-phase lock discipline as per-intent validate (resolve, run with DB closed, persist in one transaction); settled passed/failed verdicts are not re-run and blocked proofs keep their recorded reason  `src/commands/validate.rs`
- **visibility captured at seed time** — every intent created through the seed ladder is born with visibility set — user_visible from the want stage, internal from the logic stage — instead of left unset and triaged later by the align interview  `src/commands/guide.rs`
- **visual-confirm user-gated queue** — subjective aesthetic residue surfaces as manual_check validations with inspected_by set to human, batched into a user-gated lane in loom session prioritised by user presence like align, rulings and blocked  `src/commands/session.rs`

### concurrency

- **SQLite direct concurrency policy** — SQLite is the active embedded graph store: every command opens .loom/graph.sqlite directly with foreign keys, busy timeout, WAL/NORMAL pragmas, and transaction boundaries; loom serve is a retired stub rather than a correctness or performance layer.  `docs/daemon-design.md`, `src/commands/mod.rs`, `src/db/sqlite.rs`

### core

- **loom: living intent graph CLI** — Builds and maintains a falsifiable intent graph of a codebase (semantic/physical/normative planes) that any LLM drives via structured CLI commands; the graph is durable memory, the context window is the working set  `Cargo.toml`

### db

- **SQLite graph persistence** — embedded SQLite graph store behind typed command/repository APIs; schema vocabulary in Rust, physical tables and constraints in SQLite, derived endpoint edge keys, deterministic JSON import/export, and pure snapshot analysis for graph computations
- **SQLite import export parity bridge** — A fresh SQLite graph can import the current deterministic loom.graph.json and export the same semantic nodes, edges, ids, criteria, notes, list fields, and layer order without adopting old backend-private state.  `src/commands/export.rs`, `src/commands/import.rs`, `src/db/sqlite/import_export.rs`
- **SQLite query and search implementation** — Read-heavy commands use typed SQLite queries or a shared graph projection for status, next, report, doctor, smells, find, and door without preserving Grafeo-specific scan/filter workarounds unless parity requires them.  `src/commands/doctor.rs`, `src/commands/door.rs`, `src/commands/find.rs`, `src/commands/next.rs`, `src/commands/report.rs`, `src/commands/smells.rs`, `src/commands/status.rs`, `src/db/queries/find.rs`, `src/db/queries/integrity.rs`, `src/db/queries/scoring.rs`, `src/db/queries/smells.rs`, `src/db/queries/snapshot.rs`, `src/db/queries/stats.rs`, `src/db/sqlite/reads.rs`, `src/db/sqlite/search.rs`
- **SQLite-backed graph persistence migration** — Replace Loom's legacy live graph persistence with typed SQLite storage while preserving graph semantics, deterministic export/import, validation/saga honesty, direct CLI operation, and auditability through Loom itself.  `docs/sqlite-multihop-proposal.md`
- **backend-neutral storage boundary** — Command handlers and query consumers depend on typed SQLite graph storage/repository operations and pure snapshot analysis, not backend-specific query strings or legacy storage types.  `src/commands/cluster.rs`, `src/commands/coverage.rs`, `src/commands/delegate.rs`, `src/commands/doctor.rs`, `src/commands/door.rs`, `src/commands/hotspots.rs`, `src/commands/impact.rs`, `src/commands/layer.rs`, `src/commands/next/relates.rs`, `src/commands/note.rs`, `src/commands/persona.rs`, `src/commands/report.rs`, `src/commands/rule.rs`, `src/commands/saga.rs`, `src/commands/smells.rs`, `src/commands/status.rs`, `src/commands/validation.rs`, `src/commands/vocab.rs`, `src/db/mod.rs`, `src/db/sqlite.rs`, `src/output.rs`
- **endpoint-constrained edge storage** — RELATES_TO, HIERARCHY, IMPLEMENTS, GOVERNS, VALIDATES, TARGETS, SERVES, and JOURNEYS are keyed by endpoint ids with derived stable edge ids and SQLite uniqueness/foreign-key constraints, not stored edge uuids  `src/db/queries/relates_to.rs`
- **graph travel format** — deterministic JSON export and restore-into-fresh-init import so the graph travels with the repo and diffs in PRs  `src/commands/export.rs`, `src/commands/import.rs`
- **interface surface schema vocabulary** — Add the schema/type/export/import vocabulary for a generic interface surface node and a stable call edge, with identity fields that support HTTP endpoints first and leave room for CLI commands, RPC methods, and event topics.  `src/db/schema.rs`, `src/db/sqlite.rs`, `src/types.rs`
- **schema vocabulary and repository boundary** — single-source graph vocabulary declarations (labels, edges, properties, owners, version), shared type structs, graph root targeting helpers, and the GraphReadRepository/GraphReadHandle boundary backed by SQLite  `src/commands/migrate.rs`, `src/db/mod.rs`, `src/db/schema.rs`, `src/types.rs`
- **snapshot analysis and annotation helpers** — pure Rust analysis helpers over typed QuerySnapshot data plus annotation-oriented note/vocabulary/meta helpers; SQLite owns storage and mutation while this layer derives queues, integrity, stats, search, smells, and coverage signals  `src/commands/explain.rs`, `src/db/queries/meta.rs`, `src/db/queries/mod.rs`
- **typed SQLite graph schema** — The SQLite store models Loom's graph as typed node and edge tables with foreign keys, unique endpoint constraints, status CHECKs, queue indexes, JSON-list columns for list facts, and FTS indexes where search uses them.  `src/db/sqlite/schema_ddl.rs`

### developer-experience

- **source corpus coverage** — enumerates structured requirement-like IDs (US-, E-, REQ-, NFR-, INV-, ADR-) in documentation files and reconciles them against the intent graph, surfacing documented-but-unmodeled requirements for inbox triage  `src/commands/corpus.rs`, `src/db/queries/corpus.rs`

### docs

- **storage documentation and guide refresh** — README, guide, command docs, retired serve notice, and build/install guidance describe SQLite storage accurately and omit obsolete legacy lock/query caveats.  `README.md`, `docs/COMMANDS.md`, `docs/CONTRIBUTING.md`, `docs/daemon-design.md`, `src/commands/guide.rs`

### graph-integrity

- **hypothesis prove records TARGETS confidence** — When a hypothesis is proven, the TARGETS verdicts it stamps carry the prover's explicit confidence instead of leaving the uninspected 0.0 default behind.  `src/cli.rs`, `src/commands/hypothesis.rs`, `src/db/sqlite/edge_writes.rs`

### health

- **completeness and integrity checking** — vertical spine (tree shape + leaf realization + file reach), coverage reconciliation against disk, doctor audit of schema conformance and provenance lanes  `docs/maturity-ladder-proposal.md`, `src/commands/complete.rs`, `src/commands/coverage.rs`, `src/commands/doctor.rs`, `src/commands/hotspots.rs`, `src/commands/impact.rs`, `src/commands/report.rs`, `src/commands/status.rs`, `src/db/queries/completeness.rs`, `src/db/queries/comprehensiveness.rs`, `src/db/queries/integrity.rs`, `src/db/queries/maturity.rs`, `src/db/queries/maturity/tests.rs`, `src/db/queries/stats.rs`, `src/db/queries/stats/tests.rs`, `src/db/queries/symbol_accountability.rs`
- **derived problem signals** — loom smells: twins, overlapping ownership, scatter, tangles, undeclared coupling, recurrence, normative gaps — computed from graph structure, each with a remedy  `src/commands/domain.rs`, `src/commands/layer.rs`, `src/commands/smells.rs`, `src/db/queries/smells.rs`, `src/db/queries/smells/normative.rs`, `src/db/queries/smells/physical.rs`, `src/db/queries/smells/semantic.rs`, `src/db/queries/smells/source_fact_tests.inc`
- **verifiable delegated coverage** — Coverage treats files under a declared subtree as covered by a child loom.graph.json export, and reports a missing export as an explicit delegation-target gap.  `README.md`, `src/commands/delegate.rs`

### navigation

- **ask-the-map keyword search** — loom find: BM25 keyword search over active intent names+descriptions, ranked; hits carry hierarchy chain, IMPLEMENTS groundings with locators, and a stale-edge freshness count; scoring runs in Rust because grafeo's text-index CALL returns internal node ids unjoinable to properties through GQL; a miss points at loom coverage to distinguish unmapped from nonexistent  `src/commands/find.rs`, `src/db/queries/find.rs`

### operations

- **migration cutover and rollback path** — Users can dogfood target/debug/loom against the live SQLite graph, keep committed loom.graph.json as rollback/export state, and promote the local binary only after tests, local graph routing, and export checks pass.  `src/commands/export.rs`, `src/commands/import.rs`

### repo

- **repo introspection** — gitignore-aware file walk, stack detection, greenfield/brownfield suggestion; runs before init  `src/commands/detect.rs`, `src/repo.rs`, `src/ts_imports.rs`

### static-analysis

- **shared nonempty string push helper** — Import-analysis modules use one helper for pushing non-empty unique string specifiers into vectors.  `src/vec_utils.rs`
- **shared symbol locator matcher** — Coverage and symbol-accountability queries use one shared identifier-word matcher for deciding whether IMPLEMENTS locators name a symbol.  `src/db/queries/symbol_match.rs`

### sync

- **sync ripple indexed update path** — sync invalidates IMPLEMENTS, RELATES_TO, GOVERNS, TARGETS, and VALIDATES from indexed SQLite state with deterministic cause notes and without per-edge query fan-out.  `src/commands/sync.rs`, `src/db/sqlite/edge_writes.rs`

### teaching

- **opt-in lane-skill install** — loom skill list|show|install emits the binary-served lane-skills as SKILL.md files for a harness that wants to PIN them; never required — the binary serves each lane JIT via loom guide --role X. The SKILL.md body delegates the live charge back to the binary so a pinned copy can't drift.  `src/commands/skill.rs`
- **self-teaching surface** — guide/schema/orientation embed the full driving protocol, including lifecycle transitions and status-family separation, so a cold LLM needs no external docs  `docs/COMMANDS.md`, `src/commands/guide.rs`, `src/commands/schema.rs`, `src/commands/tour.rs`

### testing

- **storage contract regression suite** — Backend tests prove SQLite storage semantics: transactions, endpoint uniqueness, list round trips, free-text binding, constraints, search behavior, import/export shape, and snapshot reads.  `src/db/sqlite.rs`, `src/db/sqlite/tests.rs`, `tests/sqlite_regression.rs`

### trust

- **role lanes and evidence gates** — a declared LOOM_AGENT role is held to its lane at the command boundary; criterion/evidence/notes must be substantive; confidence bounded to [0,1]; solo mode passes all lanes  `src/agent.rs`, `src/gate.rs`

### unknown

- **directed handoff notes** — Notes carry an optional audience lane (--for builder|analyzer|fixer|validator|quality); loom note list --for is the lane inbox; loom next sorts notes addressed to the work item's owner role first; quality rules carry inspection_effort (low|mid|high) annotated in the packs and inherited by quality work items  `src/commands/next.rs`, `src/commands/note.rs`, `src/db/sqlite/writes.rs`
- **graph targeting pin** — Every command resolves its target graph via --graph flag > LOOM_GRAPH env > cwd, through a single resolve_root() helper; a mutating command with a pinned graph ignores cwd entirely, so the cd-fallback incident class (failed cd + mutating script hitting whatever graph cwd lands in) is dead; proven by a validation that mutates from a foreign cwd and finds the write in the pinned graph  `src/db/mod.rs`
- **hostile import rejected loudly** — loom import of a malformed, truncated, wrong-typed, or non-UTF-8 loom.graph.json fails with an error naming the offending field and leaves NO partial graph behind; proven by a fuzz-style validation feeding corrupted exports  `.claude/skills/run-loom/fuzz_import.sh`
- **inbox intake boundary** — loom inbox and door provide the single capture-first boundary for free-form human or LLM language: raw text is stored as an InboxItem, triage supplies context and route proposals, normalize records a proposed graph command or answer, and mark closes the card after the real graph command runs separately so intake candidates never masquerade as graph truth or required completion debt.  `src/commands/inbox.rs`, `src/commands/seed.rs`
- **intent retirement contract** — loom intent retire marks superseded design deprecated with reason and successor recorded as notes; retired intents are INVISIBLE TO COMPUTATION (queues, coverage axes, centrality, the grid, completeness, sync ripple) and VISIBLE TO HISTORY (node, edges, notes remain); the command reports triggered fallout: orphaned children, files losing their only owner, dangling proofs; independent edges add nothing to centrality  `src/commands/intent.rs`
- **no silent fallbacks in the query layer** — No query-layer read path swallows a parse or extraction failure via unwrap_or_default: malformed stored JSON (imports, source_refs) and missing columns surface as contextual errors, completing the hardening sweep started in codefile/intent/portability/sync  `src/db/sqlite.rs`
- **porting mode: import --as-planned** — loom import --as-planned adopts a source graph's intents, hierarchy, criteria, and validations-as-specs into a fresh target-repo graph, drops all groundings (IMPLEMENTS), marks every leaf planned, and loom guide gains a port mode teaching the re-realization loop; the semantic plane travels, the physical plane is rebuilt in the new language  `src/commands/guide.rs`
- **scale: hot commands bounded on large graphs** — On a synthetic graph of >=500 intents and >=1000 edges, loom status / next / smells / next --all each complete in under 2 seconds; the O(N^2) paths (discovery pair enumeration, twin/overlap smells) are bounded or restructured; proven by a benchmark validation against the synthetic graph  `src/db/queries/stats.rs`
- **session opener teaches the turn-zero ask** — loom session serves turn zero (the user invoked loom with no stated goal): a directive to ask ONE question in the user's language plus a state-aware offer menu where each offer is backed by a live queue and its count and exactly one is recommended; user-gated queues (align drift, hypothesis rulings, blocked proofs) outrank everything an agent can drain alone; works before loom init (import > map > interview) and on an empty graph (interview vs map by source on disk); synonym verbs (start/begin/hello/mode/talk/chat/interview) teach the command  `src/commands/door.rs`, `src/commands/session.rs`
- **source corpus coverage sad path** — When source docs carry no structured requirement IDs, corpus coverage reports completeness as UNKNOWN and routes to loom seed --inbox for LLM triage, never silently claiming full coverage from zero IDs.  `src/commands/corpus.rs`, `src/db/queries/corpus.rs`
- **tiered review queue** — Verdicts recorded with confidence below 0.7 surface in loom next --mode review, ranked (1-confidence) x centrality; re-recording at/above the threshold or overturning resolves the item; every work item carries effort low|mid|high about the WORK (never a model); the fix queue dispatches needs_reverification to the analyzer and failing to the fixer  `src/commands/next/review.rs`, `src/commands/next/scoring.rs`
- **whoami identity report** — loom whoami reports the acting $LOOM_AGENT identity, the resolved role, and whether lane enforcement is on (a role is set) or off (solo)  `src/commands/whoami.rs`

### validation

- **external interface surface plane** — Represent externally callable surfaces as first-class graph nodes so ownership, journey coverage, quality rules, and implementation grounding can address an interface independently of the saga YAML that calls it.
- **saga consumer plane** — External-consumer proofs: a saga is an ordered chain of endpoint invocations (captures thread one response into the next request) whose run stamps RUNTIME evidence into the graph — the execution complement to read-evidence grounding.  `src/commands/persona.rs`, `src/saga/mod.rs`
- **saga failure diagnosis** — Diagnose failed saga runs into structured, repo-agnostic root-cause categories and actionable next steps without stamping graph verdicts.  `src/cli.rs`, `src/commands/saga.rs`, `src/saga/diagnose.rs`, `tests/sqlite_regression.rs`
- **saga run stamps the graph** — loom saga add declares the proof (Validation type=saga + VALIDATES edges + uninspected RELATES_TO path + spec as CodeFile); loom saga run translates outcomes into verdicts: passed consecutive pairs stamp passing RELATES_TO with runtime evidence, the failing boundary stamps failing with the broken expectation, unreached pairs stay untouched, and existing edge criteria are preserved. Exits non-zero on failure so loom validate/CI read it.  `src/commands/saga.rs`, `tests/cold_saga_endpoint_warning.rs`
- **saga runner halt-on-failure semantics** — The executor runs steps eagerly and in order, halts at the first failure, and reports honest per-step outcomes: steps before the failure passed, the failing step carries every broken expectation, steps after it produce NO outcome (never reached is not failing). All target-observed failures (refusal, timeout, bad JSON, empty capture) are outcomes, not process errors.  `src/saga/runner.rs`
- **saga spec with first-class intent binding** — The YAML saga format: every step names the intent it proves; specs are validated at load (method/JSONPath/json-xor-body) and {{ var }}/{{ env.X }} interpolation resolves vars from initial vars and earlier captures, failing hard on unknown names.  `src/saga/spec.rs`
- **saga steps resolve interface calls** — During saga add, normalize each step request into an interface surface, resolve or create that surface, and record the ordered call relationship while keeping the step intent binding as the semantic behavior under proof.  `src/commands/saga.rs`
- **validation and saga storage isolation** — validate and saga run continue to execute external commands or HTTP with no live graph handle held, then reopen and record results atomically under the new storage backend.  `src/commands/saga.rs`, `src/commands/validate.rs`

### workflow

- **adoption spawns outcome proof** — Adopting a hypothesis writes its predicted_outcome as a not_run Validation attached to the spawned intents; when the validator lane passes it, the hypothesis derives confirmed. Every adopted improvement gets checked for whether it actually delivered.  `src/commands/hypothesis.rs`, `src/commands/validation.rs`
- **bounded tag vocabulary** — loom vocab maintains a small normalized tag registry for intent tags; write-time validation inlines the registry and drift remedies merge near-duplicate keys so discovery and smells get deliberate collisions.  `src/commands/vocab.rs`, `src/db/queries/vocab.rs`
- **discovery queue ranks pairs by plausibility** — unexplored RELATES_TO pairs should be ranked beyond raw centrality (e.g. shared files, shared domain, co-change) so a driver is not pointed at 31 equally-scored pairs on a 10-intent graph  `src/db/queries/scoring.rs`
- **hypothesis lifecycle commands** — loom hypothesis add/list/show/prove/adopt/reject drives the state machine with gates: claim and predicted_outcome must be substantive and falsifiable, the prover's provenance must differ from the proposer's, only a proposed hypothesis can be proven, and only a supported one adopted. Every transition is recorded as an append-only note.  `src/cli.rs`, `src/commands/hypothesis.rs`
- **hypothesis node and TARGETS edge** — Schema v4 adds a Hypothesis node (claim, proposal, predicted_outcome, status: proposed/supported/refuted/adopted/rejected, provenance) and a TARGETS edge (Hypothesis to Intent) carrying the standard inspectable meta. Both persist through the deterministic export/import round-trip with two-phase validation, and loom doctor audits their required properties and value vocabularies.  `src/db/schema.rs`, `src/types.rs`
- **hypothesis plane** — The pre-decision plane: improvement hypotheses that any lane can propose, an analyzer proves against current code, and a builder adopts into planned intents. Speculation stays invisible to coverage and completeness until adoption converts it into the existing lifecycle.
- **priority-scored work queues** — loom next: one queue per agent role (discovery/fix/build/validate/quality), scored by centrality + urgency - staleness; returns one item with full context so no second lookup is needed  `src/commands/cluster.rs`
- **scale benchmark harness** — The run-loom benchmark scripts generate a synthetic graph and fail when status, next, smells, or next --all exceed the hot-command budget.  `.claude/skills/run-loom/bench.sh`, `.claude/skills/run-loom/gen_synth_graph.py`
- **shared graph query snapshot layer** — A read-only graph projection bulk-loads shared node and edge slices once and serves status, next, report, doctor, and smells through a common query surface.  `src/db/queries/snapshot.rs`
- **smells propose hypotheses** — Structural findings that call for redesign rather than a patch (recurrent trouble, scattered intents, twin intents) emit loom hypothesis add as their remedy command, so the graph's own signals feed the proposal plane instead of dying in notes.  `src/db/queries/smells.rs`, `src/db/queries/smells/lifecycle.rs`
- **stale hypothesis evidence ripples on sync** — TARGETS edges on supported hypotheses flip to needs_reverification when a target intent's grounded code changes, with a transition note naming the file. Support earned against old code must be re-earned, exactly like RELATES_TO and GOVERNS claims.  `src/commands/sync.rs`
- **sync flag engine** — mtime-delta detection propagating one hop: RELATES_TO neighbours and passing GOVERNS go needs_reverification, linked validations go not_run; files missing on disk are reported  `src/commands/sync.rs`
- **triage queue for hypotheses** — loom next --mode triage serves proposed hypotheses ranked by target-intent centrality as optional work items - like discovery and review, triage never blocks phase=complete. loom next --all shows the triage count flagged optional.  `src/commands/next/modes.rs`


<!-- loom:prose-start -->
## The responsibility map

Each component intent owns a family of feature intents and is grounded in a
concrete set of source files. This page is the index from a component name to
the files that implement it; the skeleton above lists the deterministic
projection, and the prose below explains the *why* behind each component.

### CLI and dispatch

[CLI surface and dispatch](intent:a1a8eb10-bc4c-43d7-a4ec-b7d1fb8d26ae) — clap-derive definitions in
[`src/cli.rs`](../src/cli.rs) plus the dispatch table in
[`src/commands/mod.rs`](../src/commands/mod.rs). Owns argument parsing, the orientation
printout for bare `loom`, and the synonym/typo teacher for unrecognized verbs.

[graph-write command handlers](intent:988027a3-d33a-40c8-8f32-c5140b0d1937) — the mutating command modules
under [`src/commands/`](../src/commands/). Each noun has one file (`intent.rs`,
`edge.rs`, `codefile.rs`, `validation.rs`, `note.rs`, `rule.rs`, `ignore.rs`);
every transition is lane-gated by the
[role lanes and evidence gates](intent:20bf582e-df31-4f96-8b37-6171c38e3478).

[dual-mode output](intent:bb8ee237-2d84-46ee-b254-c8bc39c16fc1) — every handler renders both
human text and `--json`, with a shared graph_state pulse, through
[`src/output.rs`](../src/output.rs). The dual-mode contract is what makes loom
scriptable without a separate API.

[self-teaching surface](intent:f2995090-ba95-48d3-bb6d-21b4044c32dc) — `loom guide`, `loom door`,
`loom complete`, and the orient printout. The teaching axis is a first-class
component, not documentation: it reads the graph and emits the next concrete
invocation.

### Persistence and edges

[SQLite graph persistence](intent:01783338-7f02-4f4b-8d15-5f396ef7d47d) — embedded SQLite store in
[`src/db/sqlite.rs`](../src/db/sqlite.rs) behind the typed query layer in
[`src/db/queries/`](../src/db/queries/). Schema vocabulary in Rust; tables and
constraints in SQLite.

[SQLite-backed graph persistence migration](intent:c2f6bca0-2ccd-48b7-af47-1a74fba61441) — the intent that tracks the
migration from the legacy live-graph backend. Proven by the parity harness in
[`tests/sqlite_regression.rs`](../tests/sqlite_regression.rs) and the round-trip checks in
[`src/db/schema.rs`](../src/db/schema.rs).

[endpoint-constrained edge storage](intent:29288a6c-3f0b-4762-a34d-ad4a714b5390) — every edge kind (RELATES_TO,
HIERARCHY, IMPLEMENTS, GOVERNS, VALIDATES, TARGETS, SERVES, JOURNEYS) is keyed
by endpoint ids with a derived stable edge id, so an edge survives re-import
and is identity-stable. Implemented in [`src/db/schema.rs`](../src/db/schema.rs) and
[`src/db/queries/`](../src/db/queries/).

### Work selection and audit

[priority-scored work queues](intent:47c9182c-f7a8-4a50-9281-6d05507e646c) — `loom next --mode <lane>`
ranks the next item for every lane (build, fix, quality, discovery, align,
validate, populate). Scoring in [`src/db/queries/scoring.rs`](../src/db/queries/scoring.rs).

[completeness and integrity checking](intent:ab4ac603-a14d-4ae4-b68b-a4bf9dce0cb2) — `loom doctor`, `loom smells`,
`loom coverage`. The deterministic integrity axis that catches graph/code
drift. Implementations in [`src/commands/doctor.rs`](../src/commands/doctor.rs),
[`src/commands/smells.rs`](../src/commands/smells.rs), [`src/commands/coverage.rs`](../src/commands/coverage.rs).

[multi-hop audit layer](intent:c61f66f5-a396-4494-9b48-c00d5203bcb3) — multi-hop graph reads via the
shared snapshot in [`src/db/queries/snapshot.rs`](../src/db/queries/snapshot.rs). Backs
`loom explain`, `loom impact`, and `loom cluster`.

[snapshot analysis and annotation helpers](intent:e2d64ed4-b10f-48fd-995b-f533a6250a18) — `loom find`, `loom smells`,
`loom coverage` share a single read-only snapshot per invocation.
Implementations in [`src/db/queries/stats.rs`](../src/db/queries/stats.rs) and
[`src/db/queries/scoring.rs`](../src/db/queries/scoring.rs).

### Sync and analysis

[sync flag engine](intent:29799603-3704-4dfa-9ba4-387a7c1942f8) — `loom sync` re-hashes touched
files and flips only one-hop RELATES_TO neighbours to `needs_reverification`,
with a decaying ripple beyond. Implemented in
[`src/commands/sync.rs`](../src/commands/sync.rs) and [`src/repo.rs`](../src/repo.rs).

[repo introspection](intent:382c288a-4846-46f6-93be-eb9e1f40faf3) — `loom detect` and the
stack/file evidence behind quality-pack recommendations. Implementation in
[`src/repo.rs`](../src/repo.rs).

[multi-language static analysis coverage](intent:ea9c7e3e-9f95-4d58-ade9-d771fcf50cc3) — multi-language static analysis
(Rust, Go, Dart, Kotlin, Swift, Svelte/Bun) extracting imports, declarations,
and layout signals. Implementations in [`src/ts_imports.rs`](../src/ts_imports.rs) and
language modules under analysis.

[source corpus coverage](intent:dda91659-329a-479e-9d33-42a41b5fa9b1) — the source corpus that
`loom sync` walks; coverage of which files are registered and which are
untracked. Backed by [`src/repo.rs`](../src/repo.rs).

### Governance and speculation

[role lanes and evidence gates](intent:20bf582e-df31-4f96-8b37-6171c38e3478) — the lane gates (builder,
validator, analyst, registrar, reviewer, scout, architect, fixer, quality) and
the evidence requirements behind every transition. Implementations in
[`src/gate.rs`](../src/gate.rs) and [`src/agent.rs`](../src/agent.rs).

[hypothesis plane](intent:32b42fd0-6b6c-46a6-a9a8-97be595bddf3) — the pre-decision plane:
improvement hypotheses any lane can propose, an analyzer proves against
current code, and a builder adopts into planned intents. Speculation stays
invisible to coverage until adoption. Implementations in
[`src/commands/hypothesis.rs`](../src/commands/hypothesis.rs) and [`src/db/queries/`](../src/db/queries/).

[external interface surface plane](intent:fcf2f089-6dbe-46f0-8296-d50512420ff8) — externally callable surfaces
modelled as first-class graph nodes so ownership, journey coverage, quality
rules, and implementation grounding can address an interface independently of
the saga YAML that calls it. Implementations in
[`src/commands/interface.rs`](../src/commands/interface.rs) and [`src/db/queries/`](../src/db/queries/).

[saga consumer plane](intent:4c752ad2-e332-4148-87e2-88340991e2a5) — the saga consumer plane:
`loom saga` validates YAML-defined call sequences against the interface
surface so a journey is provable, not asserted. Implementation in
[`src/commands/saga.rs`](../src/commands/saga.rs).

### Seed flows

[UI/UX visual-register seed flow](intent:c6b2b5d9-387a-4fa3-936d-e4e780021067) — the visual-register seed
ladder: reaction-driven, with an HTML mockup as the reaction surface and
contract, mockup-as-CodeFile that never satisfies production IMPLEMENTS, and
machine-first verification with a human visual-confirm queue. Implementation
in [`src/commands/seed.rs`](../src/commands/seed.rs).

[intent-spectrum seed-flow guidance](intent:e21d2c64-f9fa-46c0-aaec-1985efbf8152) — the intent-spectrum seed
guidance that points a driver at the right seed flow for a given intent class.
Implementation in [`src/commands/seed.rs`](../src/commands/seed.rs) and
[`src/commands/guide.rs`](../src/commands/guide.rs).

<!-- loom:prose-end -->
