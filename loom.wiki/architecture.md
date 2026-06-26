---
type: architecture
title: "Architecture"
tags:
  - analysis
  - cli
  - core
  - db
  - developer-experience
  - docs
  - health
  - repo
  - teaching
  - trust
  - validation
  - workflow
---

## Architecture

The intent hierarchy — what the system is, decomposed top-down.

- **loom: living intent graph CLI** — Builds and maintains a falsifiable intent graph of a codebase (semantic/physical/normative planes) that any LLM drives via structured CLI commands; the graph is durable memory, the context window is the working set
  - **CLI surface and dispatch** — clap-derive command definitions and dispatch to command handlers; bare invocation prints orientation
    - **bulk grounding via glob** — edge implement <intent> 'src/db/**' should ground every matching registered file in one call, mirroring codefile add's glob support; dogfood finding: 48 files needed 48 invocations
    - **command definitions and dispatch** — clap-derive CLI surface, dispatch to handlers, bare-invocation orientation
      - **shared command entity resolvers** — Shared resolver helpers provide one id/name/fragment lookup contract for command modules instead of copy-pasted local resolve_intent_with_db and resolve_validation_with_db bodies.
    - **graph targeting pin** — Every command resolves its target graph via --graph flag > LOOM_GRAPH env > cwd, through a single resolve_root() helper; a mutating command with a pinned graph ignores cwd entirely, so the cd-fallback incident class (failed cd + mutating script hitting whatever graph cwd lands in) is dead; proven by a validation that mutates from a foreign cwd and finds the write in the pinned graph
    - **graph-write command handlers** — handlers that mutate the graph: intent/edge/codefile/validation/note/rule/ignore plus export-import restoration; all lane-gated
      - **confirmation stamps freshness for drift ranking** — loom intent confirm ratifies status AND appends a kind=confirm note (the freshness event); last_confirmed_at returns the newest stamp or None; events are append-only so alignment history travels in the export with no schema field
      - **intent meaning evolves in place with semantic ripple** — loom intent update rewrites an intent's name/description in place (same node, same history); a description change ripples one hop like sync but for meaning: passing/independent RELATES_TO and GOVERNS, passing IMPLEMENTS and TARGETS go needs_reverification and linked proofs go not_run (blocked keeps its reason); the old wording is preserved in a decision note; a name-only change ripples nothing; deprecated intents are rejected
      - **intent retirement contract** — loom intent retire marks superseded design deprecated with reason and successor recorded as notes; retired intents are INVISIBLE TO COMPUTATION (queues, coverage axes, centrality, the grid, completeness, sync ripple) and VISIBLE TO HISTORY (node, edges, notes remain); the command reports triggered fallout: orphaned children, files losing their only owner, dangling proofs; independent edges add nothing to centrality
      - **validate --all drains pending proofs** — loom validate --all runs every validation whose last_result is not_run (never run or sync-invalidated) in one verb with the same three-phase lock discipline as per-intent validate (resolve, run with DB closed, persist in one transaction); settled passed/failed verdicts are not re-run and blocked proofs keep their recorded reason
    - **intents addressable by name, not only uuid** — every command that takes an intent id should also accept its unique name (or unambiguous prefix); dogfood finding: the driver had to maintain an external name-to-id map across the whole session
    - **proof and bootstrap handlers** — loom validate (runs proofs with the session released) and loom init (idempotent bootstrap)
    - **whoami identity report** — loom whoami reports the acting $LOOM_AGENT identity, the resolved role, and whether lane enforcement is on (a role is set) or off (solo)
  - **SQLite graph persistence** — embedded SQLite graph store behind typed command/repository APIs; schema vocabulary in Rust, physical tables and constraints in SQLite, derived endpoint edge keys, deterministic JSON import/export, and pure snapshot analysis for graph computations
    - **endpoint-constrained edge storage** — RELATES_TO, HIERARCHY, IMPLEMENTS, GOVERNS, VALIDATES, TARGETS, SERVES, and JOURNEYS are keyed by endpoint ids with derived stable edge ids and SQLite uniqueness/foreign-key constraints, not stored edge uuids
    - **graph travel format** — deterministic JSON export and restore-into-fresh-init import so the graph travels with the repo and diffs in PRs
    - **hostile import rejected loudly** — loom import of a malformed, truncated, wrong-typed, or non-UTF-8 loom.graph.json fails with an error naming the offending field and leaves NO partial graph behind; proven by a fuzz-style validation feeding corrupted exports
    - **porting mode: import --as-planned** — loom import --as-planned adopts a source graph's intents, hierarchy, criteria, and validations-as-specs into a fresh target-repo graph, drops all groundings (IMPLEMENTS), marks every leaf planned, and loom guide gains a port mode teaching the re-realization loop; the semantic plane travels, the physical plane is rebuilt in the new language
    - **schema vocabulary and repository boundary** — single-source graph vocabulary declarations (labels, edges, properties, owners, version), shared type structs, graph root targeting helpers, and the GraphReadRepository/GraphReadHandle boundary backed by SQLite
    - **snapshot analysis and annotation helpers** — pure Rust analysis helpers over typed QuerySnapshot data plus annotation-oriented note/vocabulary/meta helpers; SQLite owns storage and mutation while this layer derives queues, integrity, stats, search, smells, and coverage signals
      - **directed handoff notes** — Notes carry an optional audience lane (--for builder|analyzer|fixer|validator|quality); loom note list --for is the lane inbox; loom next sorts notes addressed to the work item's owner role first; quality rules carry inspection_effort (low|mid|high) annotated in the packs and inherited by quality work items
      - **multi-language static analysis coverage** — loom extends language-aware import, symbol, test, generated-file, and project detection beyond Rust, TypeScript, JavaScript, and Python so popular stacks get first-class static analysis while the graph remains language-agnostic
        - **Dart and Flutter static analysis support** — loom extracts Dart imports and declarations and recognizes Flutter conventions such as pubspec.yaml, lib, test, integration_test, and generated Dart files so Flutter apps get useful static-analysis signals
        - **Go static analysis support** — loom extracts Go imports, packages, top-level declarations, test files, and module paths so Go repositories get coverage, coupling, symbol-accountability, sync-narrowing, clone, and smell signals
        - **Kotlin static analysis support** — loom extracts Kotlin package imports, declarations, test conventions, and Android or JVM layout signals so Kotlin repositories receive language-aware coverage, coupling, and smell analysis
        - **Svelte and Bun project support** — loom recognizes Bun projects and extracts useful Svelte component module/script symbols and imports so Svelte and Bun repositories get coverage, coupling, and smell signals instead of degrading to file-only tracking
        - **Swift static analysis support** — loom extracts Swift imports, declarations, test conventions, and Swift package or Apple app layout signals so iOS, macOS, and Swift package repositories receive language-aware static-analysis coverage
        - **shared nonempty string push helper** — Import-analysis modules use one helper for pushing non-empty unique string specifiers into vectors.
        - **shared symbol locator matcher** — Coverage and symbol-accountability queries use one shared identifier-word matcher for deciding whether IMPLEMENTS locators name a symbol.
      - **no silent fallbacks in the query layer** — No query-layer read path swallows a parse or extraction failure via unwrap_or_default: malformed stored JSON (imports, source_refs) and missing columns surface as contextual errors, completing the hardening sweep started in codefile/intent/portability/sync
      - **shared graph query snapshot layer** — A read-only graph projection bulk-loads shared node and edge slices once and serves status, next, report, doctor, and smells through a common query surface.
  - **SQLite-backed graph persistence migration** — Replace Loom's legacy live graph persistence with typed SQLite storage while preserving graph semantics, deterministic export/import, validation/saga honesty, direct CLI operation, and auditability through Loom itself.
    - **SQLite direct concurrency policy** — SQLite is the active embedded graph store: every command opens .loom/graph.sqlite directly with foreign keys, busy timeout, WAL/NORMAL pragmas, and transaction boundaries; loom serve is a retired stub rather than a correctness or performance layer.
    - **SQLite import export parity bridge** — A fresh SQLite graph can import the current deterministic loom.graph.json and export the same semantic nodes, edges, ids, criteria, notes, list fields, and layer order without adopting old backend-private state.
    - **SQLite query and search implementation** — Read-heavy commands use typed SQLite queries or a shared graph projection for status, next, report, doctor, smells, find, and door without preserving Grafeo-specific scan/filter workarounds unless parity requires them.
    - **backend-neutral storage boundary** — Command handlers and query consumers depend on typed SQLite graph storage/repository operations and pure snapshot analysis, not backend-specific query strings or legacy storage types.
    - **migration cutover and rollback path** — Users can dogfood target/debug/loom against the live SQLite graph, keep committed loom.graph.json as rollback/export state, and promote the local binary only after tests, local graph routing, and export checks pass.
    - **storage contract regression suite** — Backend tests prove SQLite storage semantics: transactions, endpoint uniqueness, list round trips, free-text binding, constraints, search behavior, import/export shape, and snapshot reads.
    - **storage documentation and guide refresh** — README, guide, command docs, retired serve notice, and build/install guidance describe SQLite storage accurately and omit obsolete legacy lock/query caveats.
    - **storage responsibility vocabulary coverage** — The migration seeds and applies a small bounded vocabulary for storage, query, export, daemon, schema, parity, and migration work so duplicated responsibility and ownership drift are detectable while the backend is being rewritten.
    - **sync ripple indexed update path** — sync invalidates IMPLEMENTS, RELATES_TO, GOVERNS, TARGETS, and VALIDATES from indexed SQLite state with deterministic cause notes and without per-edge query fan-out.
    - **typed SQLite graph schema** — The SQLite store models Loom's graph as typed node and edge tables with foreign keys, unique endpoint constraints, status CHECKs, queue indexes, JSON-list columns for list facts, and FTS indexes where search uses them.
    - **validation and saga storage isolation** — validate and saga run continue to execute external commands or HTTP with no live graph handle held, then reopen and record results atomically under the new storage backend.
  - **UI/UX visual-register seed flow** — The seed ladder specialised for the visual register: reaction-driven with an HTML mockup as the reaction surface and contract, mockup-as-CodeFile that never satisfies production IMPLEMENTS, and machine-first verification with a human visual-confirm queue
    - **UI state coverage via aspect** — each screen component carries an aspect per UI state (populated, empty, loading, error) so happy_path_only flags a component that has a populated child but no empty or error sibling
    - **design-system standards via QualityRule packs** — UI standards such as accessibility, contrast, touch targets and responsive breakpoints are seeded from rule seed web-ui and mobile and GOVERN the screen and component intents rather than being invented per screen
    - **mockup is contract not realization** — a production screen intent source_refs its HTML mockup and stays lifecycle=planned; the mockup never creates an IMPLEMENTS edge for it, and only an explicit prototype or Storybook intent may IMPLEMENTS a mockup
    - **reaction-driven mockup loop** — for a user_visible screen, loom guidance generates an HTML mockup as the reaction surface and converts each human reaction into graph deltas, looping until reactions stop changing structure
    - **visual-confirm user-gated queue** — subjective aesthetic residue surfaces as manual_check validations with inspected_by set to human, batched into a user-gated lane in loom session prioritised by user presence like align, rulings and blocked
  - **code-primary repo wiki machinery** — the v2 repo wiki machinery: a code-primary wiki whose prose links to source files (not intent UUIDs), with the intent graph as an invisible manifest that certifies the prose via coverage, freshness, and graph-aware consistency gates; hard-cut replacement of v1's graph-primary OKF emitter and the flat loom.wiki.md
    - **code-primary wiki emitter** — the v2 emitter: replaces v1's graph-primary OKF skeleton with a code-primary manifest layer (frontmatter sourceFiles+symbols+provenance) and a prose layer (file-path links, no intent:UUID in reader-facing prose); hybrid structure of bounded topical pages + per-component module pages
    - **graph-aware manifest resolver** — the invisible manifest backbone: resolves file-paths to intents at check time so the consistency gate (the one Qoder cannot do) still falsifies relational claims against typed graph edges even though the reader never sees an intent UUID
    - **wiki lane and self-teaching authoring loop** — the work lane and self-teaching loop: loom next --mode wiki surfaces uncited salient nodes, stale provenance stamps, and fabricated links; for foreign repos loom inits the target graph then orders the LLM to author, with the graph as the invisible manifest
  - **completeness and integrity checking** — vertical spine (tree shape + leaf realization + file reach), coverage reconciliation against disk, doctor audit of schema conformance and provenance lanes
    - **derived problem signals** — loom smells: twins, overlapping ownership, scatter, tangles, undeclared coupling, recurrence, normative gaps — computed from graph structure, each with a remedy
      - **advisory buckets honor decision adjudication** — Advisory smell buckets move current decision-note rulings out of open advisory counts and into adjudicated output, with reopen anchors on the relevant intent or file.
      - **bounded tag vocabulary** — loom vocab maintains a small normalized tag registry for intent tags; write-time validation inlines the registry and drift remedies merge near-duplicate keys so discovery and smells get deliberate collisions.
    - **source corpus coverage** — enumerates structured requirement-like IDs (US-, E-, REQ-, NFR-, INV-, ADR-) in documentation files and reconciles them against the intent graph, surfacing documented-but-unmodeled requirements for inbox triage
    - **source corpus coverage sad path** — When source docs carry no structured requirement IDs, corpus coverage reports completeness as UNKNOWN and routes to loom seed --inbox for LLM triage, never silently claiming full coverage from zero IDs.
    - **verifiable delegated coverage** — Coverage treats files under a declared subtree as covered by a child loom.graph.json export, and reports a missing export as an explicit delegation-target gap.
  - **dual-mode output** — every command renders human-readable text or --json including a graph_state pulse
  - **external interface surface plane** — Represent externally callable surfaces as first-class graph nodes so ownership, journey coverage, quality rules, and implementation grounding can address an interface independently of the saga YAML that calls it.
    - **computed graph population lane** — Loom computes brownfield and schema-upgrade graph-population work, then backfills interface surfaces and CALLS from existing saga specs without changing product-code lifecycle.
      - **interface plane gap detection** — Loom detects gaps in the already-populated InterfaceSurface/CALLS plane, including uncalled surfaces, boundary intents without CALLS bindings, and CALLS edges missing matching VALIDATES edges.
        - **status surfaces populate gaps** — Loom status exposes computed populate work, including interface-plane gaps and the next populate command, so an LLM starting from status can see graph population gaps that need resolution.
    - **interface surface inspection commands** — Expose CLI reads that list and inspect interface surfaces, showing method/path or equivalent identity, owning intent or implementation grounding, saga callers, validation state, and quality-rule verdict coverage.
    - **interface surface schema vocabulary** — Add the schema/type/export/import vocabulary for a generic interface surface node and a stable call edge, with identity fields that support HTTP endpoints first and leave room for CLI commands, RPC methods, and event topics.
    - **saga steps resolve interface calls** — During saga add, normalize each step request into an interface surface, resolve or create that surface, and record the ordered call relationship while keeping the step intent binding as the semantic behavior under proof.
  - **hypothesis plane** — The pre-decision plane: improvement hypotheses that any lane can propose, an analyzer proves against current code, and a builder adopts into planned intents. Speculation stays invisible to coverage and completeness until adoption converts it into the existing lifecycle.
    - **adoption spawns outcome proof** — Adopting a hypothesis writes its predicted_outcome as a not_run Validation attached to the spawned intents; when the validator lane passes it, the hypothesis derives confirmed. Every adopted improvement gets checked for whether it actually delivered.
    - **hypothesis lifecycle commands** — loom hypothesis add/list/show/prove/adopt/reject drives the state machine with gates: claim and predicted_outcome must be substantive and falsifiable, the prover's provenance must differ from the proposer's, only a proposed hypothesis can be proven, and only a supported one adopted. Every transition is recorded as an append-only note.
    - **hypothesis node and TARGETS edge** — Schema v4 adds a Hypothesis node (claim, proposal, predicted_outcome, status: proposed/supported/refuted/adopted/rejected, provenance) and a TARGETS edge (Hypothesis to Intent) carrying the standard inspectable meta. Both persist through the deterministic export/import round-trip with two-phase validation, and loom doctor audits their required properties and value vocabularies.
    - **hypothesis prove records TARGETS confidence** — When a hypothesis is proven, the TARGETS verdicts it stamps carry the prover's explicit confidence instead of leaving the uninspected 0.0 default behind.
    - **smells propose hypotheses** — Structural findings that call for redesign rather than a patch (recurrent trouble, scattered intents, twin intents) emit loom hypothesis add as their remedy command, so the graph's own signals feed the proposal plane instead of dying in notes.
    - **stale hypothesis evidence ripples on sync** — TARGETS edges on supported hypotheses flip to needs_reverification when a target intent's grounded code changes, with a transition note naming the file. Support earned against old code must be re-earned, exactly like RELATES_TO and GOVERNS claims.
    - **triage queue for hypotheses** — loom next --mode triage serves proposed hypotheses ranked by target-intent centrality as optional work items - like discovery and review, triage never blocks phase=complete. loom next --all shows the triage count flagged optional.
  - **intent-spectrum seed-flow guidance** — loom guide --mode seed stages a want-to-contract-to-logic-to-physical ladder so a driver populates the intent register implicitly through question order, with visibility captured at seed time, rather than via a new Intent subtype
    - **auto-stub the acceptance validation at the contract stage** — the contract stage writes a not_run manual_check Validation VALIDATES-linked to the want intent so the want is falsifiable by default, mirroring the hypothesis-adoption predicted-outcome stub
    - **staged want-to-physical seed ladder** — loom guide --mode seed elicits in four ordered stages — want (a user_visible intent plus persona), contract (a validation), logic (internal child intents), physical (implements to code) — one stage and one question at a time
    - **visibility captured at seed time** — every intent created through the seed ladder is born with visibility set — user_visible from the want stage, internal from the logic stage — instead of left unset and triaged later by the align interview
  - **multi-hop audit layer** — Derived audit signals computed over arbitrary-depth traversals of the intent and import graph (RELATES_TO cycles, transitive layering violations, intent islands, bridge centrality), surfaced via smells and graph_state without putting heavy traversal on the per-turn hot path
    - **betweenness centrality in priority scoring** — loom next ranking incorporates bridge (betweenness) centrality so a low-degree chokepoint intent can outrank a high-degree leaf, computed in Rust over the snapshot rather than via SQL recursion
    - **dependency-cycle smell over RELATES_TO** — loom smells reports one finding per RECIPROCAL RELATES_TO pair: two active intents where BOTH directed rows (a->b and b->a) carry a grounded verdict (not uninspected, not independent). RELATES_TO is semantically undirected and its stored direction is not canonicalized, so a long directed cycle is typing-order noise (the old SCC reading produced a vacuous 39-intent blob on loom's own graph); a reciprocal grounded pair is the one honest signal - the same relationship stored twice, double-counting in centrality and able to carry disagreeing verdicts. Mechanically-created uninspected saga path edges are excluded so a round-trip journey never false-gates. A graph with no reciprocal grounded pair reports none.
    - **graded decaying sync ripple beyond one hop** — loom sync flips only direct one-hop RELATES_TO neighbors of a changed file's intents to needs_reverification, while two and three hop neighbors receive a decaying priority bump that does not change their inspection_status
    - **intent-island reachability finding** — loom smells reports any intent subgraph with no path to a system-level root through HIERARCHY and RELATES_TO as an island; a fully connected graph reports none
    - **structure_version cache for topology-keyed metrics** — topology-keyed heavy metrics (cycles, islands, centrality) are cached and stamped with a structure_version that bumps on node or edge add and remove and on layer or lifecycle change, but never on an inspection_status flip
    - **transitive layering-violation detection** — the layering check flags an up-the-order dependency that is clean at every single hop but forms an illegal direction across multiple hops, going beyond the direct-import check
  - **priority-scored work queues** — loom next: one queue per agent role (discovery/fix/build/validate/quality), scored by centrality + urgency - staleness; returns one item with full context so no second lookup is needed
    - **align queue ranks user-intent drift suspicion** — loom next --mode align serves active intents ranked by (1+churn-since-confirm) x ln(1+degree) + age/90 where churn counts sync-flip transition notes on the intent's edges newer than its last confirm stamp; the item presents the meaning for a user interview with exactly four outcome commands (confirm/update/retire/add); validator lane, effort mid, dispatch names the align queue
    - **bulk quality read grouped by intent** — loom next --mode quality --take N returns compact GOVERNS work items in one call, grouped by intent so one code-neighborhood read pays for every rule held against it, with prefilled rule_verdict batch-template lines (existing criterion kept, placeholder evidence forced through the gates) and per-item effort from the rule's annotation; capped at 50; symmetric with loom batch's rule_verdict write
    - **discovery queue ranks pairs by plausibility** — unexplored RELATES_TO pairs should be ranked beyond raw centrality (e.g. shared files, shared domain, co-change) so a driver is not pointed at 31 equally-scored pairs on a 10-intent graph
    - **scale: hot commands bounded on large graphs** — On a synthetic graph of >=500 intents and >=1000 edges, loom status / next / smells / next --all each complete in under 2 seconds; the O(N^2) paths (discovery pair enumeration, twin/overlap smells) are bounded or restructured; proven by a benchmark validation against the synthetic graph
      - **scale benchmark harness** — The run-loom benchmark scripts generate a synthetic graph and fail when status, next, smells, or next --all exceed the hot-command budget.
    - **tiered review queue** — Verdicts recorded with confidence below 0.7 surface in loom next --mode review, ranked (1-confidence) x centrality; re-recording at/above the threshold or overturning resolves the item; every work item carries effort low|mid|high about the WORK (never a model); the fix queue dispatches needs_reverification to the analyzer and failing to the fixer
  - **repo introspection** — gitignore-aware file walk, stack detection, greenfield/brownfield suggestion; runs before init
  - **role lanes and evidence gates** — a declared LOOM_AGENT role is held to its lane at the command boundary; criterion/evidence/notes must be substantive; confidence bounded to [0,1]; solo mode passes all lanes
  - **saga consumer plane** — External-consumer proofs: a saga is an ordered chain of endpoint invocations (captures thread one response into the next request) whose run stamps RUNTIME evidence into the graph — the execution complement to read-evidence grounding.
    - **saga failure diagnosis** — Diagnose failed saga runs into structured, repo-agnostic root-cause categories and actionable next steps without stamping graph verdicts.
    - **saga run stamps the graph** — loom saga add declares the proof (Validation type=saga + VALIDATES edges + uninspected RELATES_TO path + spec as CodeFile); loom saga run translates outcomes into verdicts: passed consecutive pairs stamp passing RELATES_TO with runtime evidence, the failing boundary stamps failing with the broken expectation, unreached pairs stay untouched, and existing edge criteria are preserved. Exits non-zero on failure so loom validate/CI read it.
    - **saga runner halt-on-failure semantics** — The executor runs steps eagerly and in order, halts at the first failure, and reports honest per-step outcomes: steps before the failure passed, the failing step carries every broken expectation, steps after it produce NO outcome (never reached is not failing). All target-observed failures (refusal, timeout, bad JSON, empty capture) are outcomes, not process errors.
    - **saga spec with first-class intent binding** — The YAML saga format: every step names the intent it proves; specs are validated at load (method/JSONPath/json-xor-body) and {{ var }}/{{ env.X }} interpolation resolves vars from initial vars and earlier captures, failing hard on unknown names.
  - **self-teaching surface** — guide/schema/orientation embed the full driving protocol, including lifecycle transitions and status-family separation, so a cold LLM needs no external docs
    - **ask-the-map keyword search** — loom find: BM25 keyword search over active intent names+descriptions, ranked; hits carry hierarchy chain, IMPLEMENTS groundings with locators, and a stale-edge freshness count; scoring runs in Rust because grafeo's text-index CALL returns internal node ids unjoinable to properties through GQL; a miss points at loom coverage to distinguish unmapped from nonexistent
    - **inbox intake boundary** — loom inbox and door provide the single capture-first boundary for free-form human or LLM language: raw text is stored as an InboxItem, triage supplies context and route proposals, normalize records a proposed graph command or answer, and mark closes the card after the real graph command runs separately so intake candidates never masquerade as graph truth or required completion debt.
    - **opt-in lane-skill install** — loom skill list|show|install emits the binary-served lane-skills as SKILL.md files for a harness that wants to PIN them; never required — the binary serves each lane JIT via loom guide --role X. The SKILL.md body delegates the live charge back to the binary so a pinned copy can't drift.
    - **seed guide teaches the user interview** — loom guide --mode seed is explicit-only (never auto-detected) and teaches both loops: elicit (altitude calibrated to user fluency, one question per landing, recommended answers, terminate on enumerable gaps not exhaustion) and align (drive loom next --mode align outcomes); an empty graph's compass routes phase=seed pointing at this guide
    - **session opener teaches the turn-zero ask** — loom session serves turn zero (the user invoked loom with no stated goal): a directive to ask ONE question in the user's language plus a state-aware offer menu where each offer is backed by a live queue and its count and exactly one is recommended; user-gated queues (align drift, hypothesis rulings, blocked proofs) outrank everything an agent can drain alone; works before loom init (import > map > interview) and on an empty graph (interview vs map by source on disk); synonym verbs (start/begin/hello/mode/talk/chat/interview) teach the command
  - **sync flag engine** — mtime-delta detection propagating one hop: RELATES_TO neighbours and passing GOVERNS go needs_reverification, linked validations go not_run; files missing on disk are reported


<!-- loom:prose-start -->






## The layered shape

loom is a single binary built from four concentric layers. Every request enters
at the CLI rim, is dispatched to a command handler, reads or writes through the
persistence core, and is certified by the audit and integrity layer.

### Rim — the CLI surface

The outermost layer is the CLI surface: clap-derive command definitions in
`src/cli.rs` and the dispatch table in `src/commands/mod.rs`.

### Core — SQLite graph persistence

The persistence core is an embedded SQLite store behind typed APIs in
`src/db/sqlite.rs`. The schema vocabulary is declared in `src/db/schema.rs`.
The whole project builds from [`Cargo.toml`](../Cargo.toml) — the root
manifest — and `src/main.rs` is the entry point.

### Wrap — audit, integrity, and work selection

The completeness and integrity checking runs `loom doctor`, `loom smells`, and
`loom coverage`. The multi-hop audit layer in `src/db/queries/snapshot.rs`
serves graph computations. The priority-scored work queues rank the next item.

## How a request travels

A `loom <noun> <verb>` invocation resolves its target graph, dispatches to the
noun handler, calls a typed query in `src/db/queries/`, and renders through
`src/output.rs`.
<!-- loom:prose-end -->
