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
- **applies when normalization** — normalizes and validates applies_when JSON strings and apply signal records for rule recommendation  `src/commands/rule.rs`
- **argv subcommand walk for parse errors** — Walk argv skipping flags to find the deepest matched subcommand whose after_help should print under parse errors.  `src/cli.rs`
- **auto-stub the acceptance validation at the contract stage** — the contract stage writes a not_run manual_check Validation VALIDATES-linked to the want intent so the want is falsifiable by default, mirroring the hypothesis-adoption predicted-outcome stub  `src/commands/guide.rs`
- **bulk grounding via glob** — edge implement <intent> 'src/db/**' should ground every matching registered file in one call, mirroring codefile add's glob support; dogfood finding: 48 files needed 48 invocations  `src/commands/edge.rs`
- **bulk quality read grouped by intent** — loom next --mode quality --take N returns compact GOVERNS work items in one call, grouped by intent so one code-neighborhood read pays for every rule held against it, with prefilled rule_verdict batch-template lines (existing criterion kept, placeholder evidence forced through the gates) and per-item effort from the rule's annotation; capped at 50; symmetric with loom batch's rule_verdict write  `src/commands/next/quality.rs`
- **codefile add handler** — registers new codefiles from path or glob with language detection  `src/commands/codefile.rs`
- **codefile add path expansion** — expands glob paths and skips already-registered files during add  `src/commands/codefile.rs`
- **codefile add result printer** — prints add summary with registered paths and skip count  `src/commands/codefile.rs`
- **codefile cli subcommands** — loom codefile add/show/list/register commands for grounding source files  `src/cli.rs`
- **codefile command dispatch** — dispatches CodefileCmd variants to add/list/show/remove sqlite handlers  `src/commands/codefile.rs`
- **codefile extractor grade label** — maps extractor_grade to human-readable trust label in show output  `src/commands/codefile.rs`
- **codefile list handler** — lists registered codefiles up to limit from sqlite  `src/commands/codefile.rs`
- **codefile not found message** — shared user-facing CodeFile not found error string for lookup-by-key surfaces  `src/commands/codefile.rs`
- **codefile remove handler** — removes a codefile and its IMPLEMENTS edges from the graph  `src/commands/codefile.rs`
- **codefile show handler** — shows ownership view for one codefile with locators and governing rules  `src/commands/codefile.rs`
- **command definitions and dispatch** — clap-derive CLI surface, dispatch to handlers, bare-invocation orientation  `build.rs`, `src/cli.rs`, `src/commands/mod.rs`, `src/main.rs`
- **command typo edit distance** — Levenshtein distance for nearest real command suggestions  `src/commands/mod.rs`
- **computed graph population lane** — Loom computes brownfield and schema-upgrade graph-population work, then backfills interface surfaces and CALLS from existing saga specs without changing product-code lifecycle.  `src/commands/next.rs`, `src/commands/populate.rs`, `src/db/sqlite/edge_writes.rs`, `tests/sqlite_regression.rs`
- **concurrency quality rule pack** — seeds concurrency and performance budget rules for sync discipline locks atomicity and backpressure  `src/commands/rule.rs`
- **confirmation stamps freshness for drift ranking** — loom intent confirm ratifies status AND appends a kind=confirm note (the freshness event); last_confirmed_at returns the newest stamp or None; events are append-only so alignment history travels in the export with no schema field  `src/commands/intent.rs`, `src/db/queries/scoring.rs`
- **corpus cli subcommands** — loom corpus ingest/list/show commands for source document management  `src/cli.rs`
- **data quality rule pack** — seeds data governance rules for migrations ingest loss pii idempotency and lineage  `src/commands/rule.rs`
- **delegate cli subcommands** — loom delegate commands for federated subgraph ownership  `src/cli.rs`
- **design-system standards via QualityRule packs** — UI standards such as accessibility, contrast, touch targets and responsive breakpoints are seeded from rule seed web-ui and mobile and GOVERN the screen and component intents rather than being invented per screen  `src/commands/rule.rs`
- **docker applies when signals** — scopes docker packaging rules to intents mentioning containers or grounding docker artifacts  `src/commands/rule.rs`
- **docker build applies when signals** — extends docker scoping with missing build and run validation signals  `src/commands/rule.rs`
- **docker quality rule pack** — seeds container packaging rules for build proof multistage cache context hardening secrets and runtime contract  `src/commands/rule.rs`
- **domain cli subcommands** — loom domain list/show commands for intent domain vocabulary  `src/cli.rs`
- **door capture doctrine copy** — prints capture-first doctrine reminding agents to normalize before routing  `src/commands/door.rs`
- **door granularity cue copy** — orientation text distinguishing greenfield seeding from brownfield reconciliation  `src/commands/door.rs`
- **door greenfield orientation** — returns greenfield vs brownfield orientation label from has_source flag  `src/commands/door.rs`
- **door landing menu constants** — enumerates every utterance-class to graph-noun landing command shape  `src/commands/door.rs`
- **door routing context renderer** — renders inbox capture result with landing menu and compass context  `src/commands/door.rs`
- **dual-mode output** — every command renders human-readable text or --json including a graph_state pulse  `src/commands/tour.rs`, `src/commands/wiki.rs`, `src/output.rs`
- **edge cli subcommands** — loom edge implement/explore/hierarchy commands for graph relationships  `src/cli.rs`
- **explore cli subcommands** — loom explore subcommands for graph neighborhood inspection  `src/cli.rs`
- **export stale warning constant** — warns when committed loom.graph.json is stale after code changes  `src/commands/mod.rs`
- **graph-write command handlers** — handlers that mutate the graph: intent/edge/codefile/validation/note/rule/ignore plus export-import restoration; all lane-gated  `src/commands/batch.rs`, `src/commands/codefile.rs`, `src/commands/export.rs`, `src/commands/ignore.rs`, `src/commands/import.rs`, `src/commands/intent.rs`, `src/commands/note.rs`, `src/commands/rule.rs`, `src/commands/validation.rs`
- **grounded path needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **ignore cli subcommands** — loom ignore add/remove/list for excluding paths from sync  `src/cli.rs`
- **import path needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **inbox cli subcommands** — loom inbox triage/defer commands for seed intake cards  `src/cli.rs`
- **inbox triage command constant** — canonical inbox triage next-step string for status output  `src/commands/mod.rs`
- **intent cli subcommands** — loom intent add/show/list/confirm commands for intent lifecycle  `src/cli.rs`
- **intent meaning evolves in place with semantic ripple** — loom intent update rewrites an intent's name/description in place (same node, same history); a description change ripples one hop like sync but for meaning: passing/independent RELATES_TO and GOVERNS, passing IMPLEMENTS and TARGETS go needs_reverification and linked proofs go not_run (blocked keeps its reason); the old wording is preserved in a decision note; a name-only change ripples nothing; deprecated intents are rejected  `src/commands/intent.rs`, `src/db/sqlite/writes.rs`
- **intent-spectrum seed-flow guidance** — loom guide --mode seed stages a want-to-contract-to-logic-to-physical ladder so a driver populates the intent register implicitly through question order, with visibility captured at seed time, rather than via a new Intent subtype  `src/commands/seed.rs`
- **intents addressable by name, not only uuid** — every command that takes an intent id should also accept its unique name (or unambiguous prefix); dogfood finding: the driver had to maintain an external name-to-id map across the whole session  `src/db/queries/intent.rs`
- **interface plane gap detection** — Loom detects gaps in the already-populated InterfaceSurface/CALLS plane, including uncalled surfaces, boundary intents without CALLS bindings, and CALLS edges missing matching VALIDATES edges.  `src/commands/interface.rs`, `src/commands/next/render.rs`, `src/commands/populate.rs`, `tests/sqlite_regression.rs`
- **interface surface inspection commands** — Expose CLI reads that list and inspect interface surfaces, showing method/path or equivalent identity, owning intent or implementation grounding, saga callers, validation state, and quality-rule verdict coverage.  `src/cli.rs`, `src/commands/interface.rs`, `src/commands/mod.rs`
- **iso5055 dead code rule id** — single source of truth for dead or duplicate code rule spelling  `src/commands/rule.rs`
- **iso5055 hardcoded secrets rule id** — single source of truth for hardcoded secrets rule spelling  `src/commands/rule.rs`
- **iso5055 quality rule pack** — seeds baseline ISO 5055 reliability security performance and maintainability rules  `src/commands/rule.rs`
- **iso5055 quality rule pack** — seeds the ten CWE-grounded ISO 5055 baseline rules for reliability security performance and maintainability  `src/commands/rule.rs`
- **layer cli subcommands** — loom layer list/show commands for architecture layer vocabulary  `src/cli.rs`
- **long version stamp for binary identity** — loom --version prints crate version plus git build id so local dogfood binaries are distinguishable from release builds  `src/cli.rs`
- **loom paths composition command** — loom paths renders composition proof coverage — which intents are path-proven vs leaf-only vs unproven — as a read-only orienteering surface  `src/commands/paths.rs`
- **mobile lifecycle safe state rule id** — single source of truth for lifecycle safe state rule spelling  `src/commands/rule.rs`
- **mobile quality rule pack** — seeds mobile lifecycle offline permissions and battery rules  `src/commands/rule.rs`
- **mobile quality rule pack** — seeds mobile lifecycle offline permissions threading battery and entry point rules  `src/commands/rule.rs`
- **mockup is contract not realization** — a production screen intent source_refs its HTML mockup and stays lifecycle=planned; the mockup never creates an IMPLEMENTS edge for it, and only an explicit prototype or Storybook intent may IMPLEMENTS a mockup  `src/commands/guide.rs`
- **note cli subcommands** — loom note add/list/prune for transition and decision notes  `src/cli.rs`
- **owned import needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **owned path needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **owned text needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **owned validation group matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **pack names listing** — exposes seedable pack names for help errors and loom detect  `src/commands/rule.rs`
- **pack rule applies when builder** — attaches applies_when json to pack rules for conditional recommendation  `src/commands/rule.rs`
- **pack rule basic constructor** — builds pack rules without evidence examples or applies_when metadata  `src/commands/rule.rs`
- **pack rule constructors** — constructs PackRule values with default empty metadata or evidence-example overrides  `src/commands/rule.rs`
- **pack rule effort classifier** — maps rule names to low mid or high inspection effort tiers for model dispatch  `src/commands/rule.rs`
- **pack rule evidence constructor** — builds pack rules with evidence examples and signal expectations metadata  `src/commands/rule.rs`
- **parse-error contextual teaching** — On teachable parse failures, walk argv to the deepest matching subcommand and append its after_help block so errors teach command shape without doc-hunting.  `src/cli.rs`
- **persona cli subcommands** — loom persona commands for agent role configuration  `src/cli.rs`
- **populate cli subcommands** — loom populate commands for seeding graph gaps  `src/cli.rs`
- **populate next command constant** — canonical populate lane next-step string for status output  `src/commands/mod.rs`
- **proof and bootstrap handlers** — loom validate (runs proofs with the session released) and loom init (idempotent bootstrap)  `src/commands/init.rs`, `src/commands/validate.rs`
- **quality rule pack name lister** — returns the list of all seedable rule pack names for help and detect  `src/commands/rule.rs`
- **quality rule pack registry** — the ordered map of named pack identifiers to their rule arrays powering seed and detect  `src/commands/rule.rs`
- **reaction-driven mockup loop** — for a user_visible screen, loom guidance generates an HTML mockup as the reaction surface and converts each human reaction into graph deltas, looping until reactions stop changing structure  `src/commands/guide.rs`
- **required human gated debt key constant** — status JSON key for human-gated debt bucket  `src/commands/mod.rs`
- **root cli parser struct** — clap Parser root holding global --json flag and the top-level Command subcommand dispatch target  `src/cli.rs`
- **rule AppliesWhen struct** — deserialized applies_when JSON with signal array  `src/commands/rule.rs`
- **rule ApplySignal struct** — single declarative signal with source terms weight and reason  `src/commands/rule.rs`
- **rule IntentRuleSignals struct** — assembled intent text path import and validation signals for scoring  `src/commands/rule.rs`
- **rule PackRule impl** — constructors for PackRule with and without evidence metadata  `src/commands/rule.rs`
- **rule PackRule struct** — data type for seedable quality rule pack entries  `src/commands/rule.rs`
- **rule RuleRecommendation struct** — recommendation result carrying rule intent and score  `src/commands/rule.rs`
- **rule RuleRecommendationIntent struct** — the intent facet of a rule recommendation  `src/commands/rule.rs`
- **rule RuleRecommendationRule struct** — the rule facet of a rule recommendation  `src/commands/rule.rs`
- **rule cli subcommands** — loom rule verdict/list commands for quality governance  `src/cli.rs`
- **rule confidence label** — maps numeric confidence scores to high medium or low tier labels  `src/commands/rule.rs`
- **rule legacy scoring** — legacy score_rule_for_intent scores rules against intent text and path signals  `src/commands/rule.rs`
- **rule normalize applies when** — normalizes and validates JSON applies_when metadata strings  `src/commands/rule.rs`
- **rule pack type alias** — the Pack type alias for named rule arrays  `src/commands/rule.rs`
- **rule recommend from snapshot** — recommend_rules_from_snapshot cross-references rules against intent signals  `src/commands/rule.rs`
- **rule recommend signal assembly** — assembles text path import and validation signals from intents and groundings for rule recommendation scoring  `src/commands/rule.rs`
- **rule recommend signal assembly** — assembles intent text paths imports and validations for applies_when scoring  `src/commands/rule.rs`
- **rule recommend with db** — run_recommend_with_db drives the rule recommend CLI surface  `src/commands/rule.rs`
- **rule recommendation pipeline** — recommend_rules_from_snapshot legacy_score_rule_for_intent run_recommend_with_db and run_check_with_db power the rule recommend and check surfaces  `src/commands/rule.rs`
- **rule rule command entry** — CLI entry point dispatching rule add seed verdict list show check and recommend  `src/commands/rule.rs`
- **rule run with sqlite** — run_with_sqlite dispatches rule add seed verdict list show check and recommend  `src/commands/rule.rs`
- **rule score applies when** — score_applies_when evaluates applies_when JSON signals against an intent  `src/commands/rule.rs`
- **rule score for intent** — score_rule_for_intent delegates to applies_when or legacy scoring  `src/commands/rule.rs`
- **rule scoring add_if** — conditional scoring accumulator  `src/commands/rule.rs`
- **rule scoring helpers** — add_if conditional scoring accumulator confidence_label tier mapping and group_validations_by_intent bucketing  `src/commands/rule.rs`
- **rule show with db** — run_show_with_db resolves and renders a single quality rule by name or id  `src/commands/rule.rs`
- **rule validate apply signal** — validates apply signal source and terms fields  `src/commands/rule.rs`
- **security deep quality rule pack** — seeds AI-generated code security rules for dependency squatting rate limiting and upload validation  `src/commands/rule.rs`
- **security deep quality rule pack** — seeds AI-specific security rules for dependency squatting rate limits response shape and uploads  `src/commands/rule.rs`
- **seed guide teaches the user interview** — loom guide --mode seed is explicit-only (never auto-detected) and teaches both loops: elicit (altitude calibrated to user fluency, one question per landing, recommended answers, terminate on enumerable gaps not exhaustion) and align (drive loom next --mode align outcomes); an empty graph's compass routes phase=seed pointing at this guide  `docs/intent-spectrum-proposal.md`, `src/commands/guide.rs`, `src/db/queries/stats.rs`
- **seedable packs registry** — registers all seedable quality rule packs for detect and seed commands  `src/commands/rule.rs`
- **service quality rule pack** — seeds service contract idempotency timeout auth and observability rules  `src/commands/rule.rs`
- **service quality rule pack** — seeds service contract idempotency timeout compensation auth observability and evolution rules  `src/commands/rule.rs`
- **shared command entity resolvers** — Shared resolver helpers provide one id/name/fragment lookup contract for command modules instead of copy-pasted local resolve_intent_with_db and resolve_validation_with_db bodies.  `src/commands/resolve.rs`
- **skill cli subcommands** — loom skill commands for bundled agent skills  `src/cli.rs`
- **slice cli subcommands** — loom slice commands for subgraph extraction  `src/cli.rs`
- **smells AdjudicatedSmell** — pub struct AdjudicatedSmell  `src/db/queries/smells.rs`
- **smells COMPLEX_SYMBOL_COGNITIVE** — pub const COMPLEX_SYMBOL_COGNITIVE  `src/db/queries/smells.rs`
- **smells COMPLEX_SYMBOL_CYCLOMATIC** — pub const COMPLEX_SYMBOL_CYCLOMATIC  `src/db/queries/smells.rs`
- **smells DEBT_KINDS** — pub const DEBT_KINDS  `src/db/queries/smells.rs`
- **smells DEEPLY_NESTED_SYMBOL_DEPTH** — pub const DEEPLY_NESTED_SYMBOL_DEPTH  `src/db/queries/smells.rs`
- **smells DELETION_SAFETY_PREAMBLE** — pub(crate) const DELETION_SAFETY_PREAMBLE  `src/db/queries/smells.rs`
- **smells DUP_TAG_WEIGHT** — pub const DUP_TAG_WEIGHT  `src/db/queries/smells.rs`
- **smells DUP_UNTAGGED_SHARED_TOKENS** — pub const DUP_UNTAGGED_SHARED_TOKENS  `src/db/queries/smells.rs`
- **smells DUP_UNTAGGED_SIMILARITY** — pub const DUP_UNTAGGED_SIMILARITY  `src/db/queries/smells.rs`
- **smells DeletionContext_a_** — impl DeletionContext<'a>  `src/db/queries/smells.rs`
- **smells DeletionContexta_classify** — pub(crate) fn DeletionContext<'a>::classify  `src/db/queries/smells.rs`
- **smells DeletionContexta_clause** — pub(crate) fn DeletionContext<'a>::clause  `src/db/queries/smells.rs`
- **smells DeletionContexta_new** — pub(crate) fn DeletionContext<'a>::new  `src/db/queries/smells.rs`
- **smells HYPOTHESIS_BACKLOG_LIMIT** — pub const HYPOTHESIS_BACKLOG_LIMIT  `src/db/queries/smells.rs`
- **smells HYPOTHESIS_STALE_DAYS** — pub const HYPOTHESIS_STALE_DAYS  `src/db/queries/smells.rs`
- **smells KIND_ARCH_VERDICT_CONTRADICTS** — pub(crate) const KIND_ARCH_VERDICT_CONTRADICTS  `src/db/queries/smells.rs`
- **smells LARGE_BEHAVIORAL_SYMBOL_LINES** — pub const LARGE_BEHAVIORAL_SYMBOL_LINES  `src/db/queries/smells.rs`
- **smells LEXICAL_SIGNAL_TOKEN_CARRIER_FLOOR** — pub const LEXICAL_SIGNAL_TOKEN_CARRIER_FLOOR  `src/db/queries/smells.rs`
- **smells MANY_ARGUMENTS** — pub const MANY_ARGUMENTS  `src/db/queries/smells.rs`
- **smells MANY_AWAITS** — pub const MANY_AWAITS  `src/db/queries/smells.rs`
- **smells MANY_EXIT_PATHS** — pub const MANY_EXIT_PATHS  `src/db/queries/smells.rs`
- **smells MIN_CLONE_LINES** — pub const MIN_CLONE_LINES  `src/db/queries/smells.rs`
- **smells MIN_STRING_CONTRACT_CHARS** — pub const MIN_STRING_CONTRACT_CHARS  `src/db/queries/smells.rs`
- **smells MIN_STRING_CONTRACT_TOKENS** — pub const MIN_STRING_CONTRACT_TOKENS  `src/db/queries/smells.rs`
- **smells OVERSIZED_FILE_LINES** — pub const OVERSIZED_FILE_LINES  `src/db/queries/smells.rs`
- **smells SIZE_ADVISORY_KINDS** — pub const SIZE_ADVISORY_KINDS  `src/db/queries/smells.rs`
- **smells STRING_CONTRACT_SAFETY_PREAMBLE** — pub(crate) const STRING_CONTRACT_SAFETY_PREAMBLE  `src/db/queries/smells.rs`
- **smells Serialize_for_Smell** — impl Serialize for Smell  `src/db/queries/smells.rs`
- **smells SiteIntent** — impl SiteIntent  `src/db/queries/smells.rs`
- **smells SmellCtx** — struct SmellCtx  `src/db/queries/smells.rs`
- **smells SmellInputs** — pub struct SmellInputs  `src/db/queries/smells.rs`
- **smells SmellReport** — pub struct SmellReport  `src/db/queries/smells.rs`
- **smells SmellTeaching** — pub struct SmellTeaching  `src/db/queries/smells.rs`
- **smells Smell_id** — pub fn Smell::id  `src/db/queries/smells.rs`
- **smells Smell_intent_ids** — pub fn Smell::intent_ids  `src/db/queries/smells.rs`
- **smells StringContractLoc** — struct StringContractLoc  `src/db/queries/smells.rs`
- **smells TANGLE_INTENTS** — pub const TANGLE_INTENTS  `src/db/queries/smells.rs`, `src/db/queries/smells/physical.rs`
- **smells TWIN_SIMILARITY** — pub const TWIN_SIMILARITY  `src/db/queries/smells.rs`
- **smells adjudicate** — rules a smell finding via a decision note with gate validation  `src/db/queries/smells.rs`
- **smells arg after** — extracts the positional argument after a given flag in a CLI string  `src/db/queries/smells.rs`
- **smells behavioral symbol kind** — classifies a symbol as structural or behavioral for accountability  `src/db/queries/smells.rs`
- **smells build ctx** — fn build_smell_ctx  `src/db/queries/smells.rs`
- **smells capped join** — fn capped_join  `src/db/queries/smells.rs`
- **smells cochange_suggestions** — pub fn cochange_suggestions  `src/db/queries/smells.rs`
- **smells command surface** — fn command_or_public_surface  `src/db/queries/smells.rs`
- **smells edge explore ids** — fn edge_explore_ids_from_text  `src/db/queries/smells.rs`
- **smells evidence list cap** — caps per-smell evidence lists at 20 entries  `src/db/queries/smells.rs`
- **smells jaccard** — pub fn jaccard  `src/db/queries/smells.rs`
- **smells lexical tokens** — fn lexical_signal_tokens  `src/db/queries/smells.rs`
- **smells locate proof** — fn locate_test_proof  `src/db/queries/smells.rs`
- **smells name lifecycle** — formats a lifecycle label with a colored emoji prefix  `src/db/queries/smells.rs`
- **smells normalized contract** — fn normalized_contract_string  `src/db/queries/smells.rs`
- **smells parent dir** — fn parent_dir  `src/db/queries/smells.rs`
- **smells parse cargo test selectors** — fn parse_cargo_test_selectors  `src/db/queries/smells.rs`
- **smells path matches module** — fn path_matches_module  `src/db/queries/smells.rs`
- **smells phrase** — returns a human-readable phrase describing an intent site  `src/db/queries/smells.rs`
- **smells proof locality from parts** — fn proof_locality_from_parts  `src/db/queries/smells.rs`
- **smells proof_locality_suggestions** — pub fn proof_locality_suggestions  `src/db/queries/smells.rs`
- **smells quoted arg after** — fn quoted_arg_after  `src/db/queries/smells.rs`
- **smells recurrent teaching** — fn recurrent_teaching  `src/db/queries/smells.rs`
- **smells rfc3339 after** — fn rfc3339_after  `src/db/queries/smells.rs`
- **smells scatter_threshold** — pub fn scatter_threshold  `src/db/queries/smells.rs`
- **smells serialize** — custom serializer for Smell that omits empty fields  `src/db/queries/smells.rs`
- **smells shape hash eligible** — fn shape_hash_eligible  `src/db/queries/smells.rs`
- **smells shellish tokens** — fn shellish_tokens  `src/db/queries/smells.rs`
- **smells short_contract_excerpt** — fn short_contract_excerpt  `src/db/queries/smells.rs`
- **smells shotgun_surgery_suggestions** — pub fn shotgun_surgery_suggestions  `src/db/queries/smells.rs`
- **smells smell_identity** — fn smell_identity  `src/db/queries/smells.rs`
- **smells smell_intent_ids** — fn smell_intent_ids  `src/db/queries/smells.rs`
- **smells stable_hex** — fn stable_hex  `src/db/queries/smells.rs`
- **smells string_contract_is_noise** — fn string_contract_is_noise  `src/db/queries/smells.rs`
- **smells teaching table** — maps smell kinds to human-readable teaching guidance text  `src/db/queries/smells.rs`
- **smells teaching_for** — fn teaching_for  `src/db/queries/smells.rs`
- **sorted pair coupling key** — lexicographic min-max pair key for direction-agnostic coupling sets  `src/commands/mod.rs`
- **source cli subcommands** — loom source commands linking intents to source documents  `src/cli.rs`
- **staged want-to-physical seed ladder** — loom guide --mode seed elicits in four ordered stages — want (a user_visible intent plus persona), contract (a validation), logic (internal child intents), physical (implements to code) — one stage and one question at a time  `src/commands/guide.rs`
- **stats BlockedValidationSummary** — impl BlockedValidationSummary  `src/db/queries/stats.rs`
- **stats CoverageAxis** — impl CoverageAxis  `src/db/queries/stats.rs`
- **stats NoteLogStats** — impl NoteLogStats  `src/db/queries/stats.rs`
- **stats classify_blocked_gate_reason** — fn classify_blocked_gate_reason  `src/db/queries/stats.rs`
- **stats contains_any** — fn contains_any  `src/db/queries/stats.rs`
- **stats decide_phase** — fn decide_phase  `src/db/queries/stats.rs`
- **stats edge_status_summary** — fn edge_status_summary  `src/db/queries/stats.rs`
- **stats explored_pairs_axis** — fn explored_pairs_axis  `src/db/queries/stats.rs`
- **stats noncurrent_uninspected_validation_edges_from_snaps** — fn noncurrent_uninspected_validation_edges_from_snapshot  `src/db/queries/stats.rs`
- **stats pair_key** — fn pair_key  `src/db/queries/stats.rs`
- **stats proof_axes** — fn proof_axes  `src/db/queries/stats.rs`
- **stats validation_backlog_summary** — fn validation_backlog_summary  `src/db/queries/stats.rs`
- **status surfaces populate gaps** — Loom status exposes computed populate work, including interface-plane gaps and the next populate command, so an LLM starting from status can see graph population gaps that need resolution.  `src/commands/status.rs`, `tests/sqlite_regression.rs`
- **sync CodeRippleTarget struct** — tracks an intent plus the changed symbols that need ripple handling  `src/commands/sync.rs`
- **sync ScannedCodeFile struct** — holds a codefile with its current content_hash detected changes and new imports  `src/commands/sync.rs`
- **sync SyncContext struct** — borrowed snapshot of active intents and edge indexes for the sync ripple pass  `src/commands/sync.rs`
- **sync SyncData struct** — owned loaded graph state consumed by the sync ripple pass  `src/commands/sync.rs`
- **sync SyncState struct** — accumulated change report tracking what the sync ripple flagged  `src/commands/sync.rs`
- **sync affected intents** — collects intent ids from implements edges affected by changed codefiles  `src/commands/sync.rs`
- **sync backfill mechanical kinds** — backfills imports and shared_file kind tags on unkined RELATES_TO edges  `src/commands/sync.rs`
- **sync build report** — assembles the SyncReport from ripple results and changed file counts  `src/commands/sync.rs`
- **sync cap json arrays** — caps JSON arrays in sync report at REPORT_CAP and records true totals  `src/commands/sync.rs`
- **sync codefile changed** — detects content-hash and mtime changes for a single codefile  `src/commands/sync.rs`
- **sync command entry** — CLI entry point resolving root and opening the SQLite store for sync  `src/commands/sync.rs`
- **sync compact transitions** — trims per-target transition note history to the transition cap  `src/commands/sync.rs`
- **sync compute coupled pairs** — derives import-coupling pairs from codefiles for the import-exemption ripple path  `src/commands/sync.rs`
- **sync flag code ripple** — flips IMPLEMENTS edges to needs_reverification when grounded code changed  `src/commands/sync.rs`
- **sync flag governs** — flips GOVERNS verdicts to needs_reverification when the inspected code changed  `src/commands/sync.rs`
- **sync flag relates** — flips RELATES_TO edges to needs_reverification when either side's code changed  `src/commands/sync.rs`
- **sync flag serves** — flips SERVES edges to needs_reverification when grounded code changed  `src/commands/sync.rs`
- **sync flush pending hashes** — writes accumulated content_hash updates for changed codefiles  `src/commands/sync.rs`
- **sync group intents by codefile** — indexes implements edges by codefile for fast lookup  `src/commands/sync.rs`
- **sync invalidate delegation validations** — flips delegation-tracking validations to not_run on code change  `src/commands/sync.rs`
- **sync invalidate validations** — flips linked validations to not_run when grounded code changed  `src/commands/sync.rs`
- **sync load sync data** — loads all graph edges and codefiles needed for the sync ripple pass  `src/commands/sync.rs`
- **sync next sync step** — computes the next_step routing directive after sync  `src/commands/sync.rs`
- **sync print json renderer** — renders the SyncReport as capped JSON with _total companion fields  `src/commands/sync.rs`
- **sync print missing files** — prints registered files missing on disk in human sync output  `src/commands/sync.rs`
- **sync print report dispatcher** — dispatches to print_sync_json or print_sync_text based on printer mode  `src/commands/sync.rs`
- **sync print stale locators** — prints stale IMPLEMENTS locators in human sync output  `src/commands/sync.rs`
- **sync print text renderer** — renders the full human sync report with files changed edges flagged and next step  `src/commands/sync.rs`
- **sync report cap** — caps per-section report lists in sync output  `src/commands/sync.rs`
- **sync ripple delegations** — flips delegation validations to not_run and re-reads child exports on sync  `src/commands/sync.rs`
- **sync run with sqlite** — core sync loop loading data detecting changes and applying ripples  `src/commands/sync.rs`
- **sync scan codefile** — reads a codefile from disk computes content_hash and checks for changes  `src/commands/sync.rs`
- **sync scan files and flag changes** — iterates codefiles detecting content-hash changes and dispatching to flag helpers  `src/commands/sync.rs`
- **sync sqlite store alias** — type alias for SqliteGraphStore used in sync functions  `src/commands/sync.rs`
- **sync update facts and flag locators** — re-extracts symbol facts for changed codefiles and flags stale locators  `src/commands/sync.rs`
- **tag cli subcommands** — loom tag vocabulary commands  `src/cli.rs`
- **text path and import needle matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **top-level cli command enum** — clap Subcommand enum listing every loom top-level verb (init, status, next, intent, edge, …) wired into the dispatch match  `src/cli.rs`
- **unknown command teaching handler** — teaches correct noun-verb invocation for unrecognized CLI tokens  `src/commands/mod.rs`
- **validate --all drains pending proofs** — loom validate --all runs every validation whose last_result is not_run (never run or sync-invalidated) in one verb with the same three-phase lock discipline as per-intent validate (resolve, run with DB closed, persist in one transaction); settled passed/failed verdicts are not re-run and blocked proofs keep their recorded reason  `src/commands/validate.rs`
- **validate CommandOutcome enum** — discriminating ran_inert or unknown runner outcome from validation execution  `src/commands/validate.rs`
- **validate CoverageReport impl** — adds symbol_executed and coverage_verdict methods to CoverageReport  `src/commands/validate.rs`
- **validate CoverageReport struct** — holds LCOV coverage data keyed by source file path  `src/commands/validate.rs`
- **validate CoverageVerdict enum** — Confirms Irrelevant or NotProven verdict from LCOV coverage report analysis  `src/commands/validate.rs`
- **validate Grounding struct** — pairs a validation command's test file with its grounded codefiles  `src/commands/validate.rs`
- **validate ProofRelevance enum** — Confirmed Irrelevant or NotProven judgement of a test proof against grounded code  `src/commands/validate.rs`
- **validate command only prints** — detects when a validation command only echoes a pass string without running a test  `src/commands/validate.rs`
- **validate command source files** — extracts source file paths from a validate command line for import analysis  `src/commands/validate.rs`
- **validate conftest chain** — finds the conftest.py chain for a Python test file under the src layout  `src/commands/validate.rs`
- **validate count before** — counts lines before a given line number in LCOV report  `src/commands/validate.rs`
- **validate count raw imports** — counts static import statements from a test file to grounded codefiles  `src/commands/validate.rs`
- **validate coverage symbol executed** — checks whether an LCOV coverage report shows the grounded symbol was executed  `src/commands/validate.rs`
- **validate discover coverage** — finds an LCOV coverage file for a test file by convention or env var  `src/commands/validate.rs`
- **validate grounding by validation** — maps a validation to its intent's IMPLEMENTS grounding for proof relevance  `src/commands/validate.rs`
- **validate is env var name** — checks whether a token looks like an environment variable name  `src/commands/validate.rs`
- **validate leading count** — extracts the leading integer cargo-test passed count from runner output  `src/commands/validate.rs`
- **validate manual verdict sticky** — guards hand-marked passed/failed verdicts from being overwritten by re-validation  `src/commands/validate.rs`
- **validate parse lcov** — parses an LCOV tracefile into source-file line execution counts  `src/commands/validate.rs`
- **validate passed count** — extracts the cargo-test passed count from the last matching line in output  `src/commands/validate.rs`
- **validate proof discrimination** — classifies a validation run as discriminating ran_inert or unknown  `src/commands/validate.rs`
- **validate proof relevance** — determines whether a validation proof exercises the grounded code via imports or coverage  `src/commands/validate.rs`
- **validate remove import lines** — strips import statements from source file content for symbol-usage analysis  `src/commands/validate.rs`
- **validate result edge status** — maps validation result to the edge inspection status  `src/commands/validate.rs`
- **validate run validation command** — executes a validation command capturing stdout stderr exit code and timing  `src/commands/validate.rs`
- **validate saga builtin engine** — detects whether a saga command uses the built-in loom saga run engine  `src/commands/validate.rs`
- **validate saga missing env** — detects unresolved env var references in saga command templates  `src/commands/validate.rs`
- **validate strip literals comments** — removes string literals and comments from source code before symbol scanning  `src/commands/validate.rs`
- **validate symbol used in source** — checks whether a grounded symbol name appears in a test file body outside imports  `src/commands/validate.rs`
- **validate tap pass count** — parses TAP output line to extract the pass count  `src/commands/validate.rs`
- **validate terminate command tree** — kills a spawned command and its whole process group on timeout  `src/commands/validate.rs`
- **validate transitive imports** — computes the transitive closure of imports from a test file  `src/commands/validate.rs`
- **validation add handler** — creates a new validation node with type command and optional intent link  `src/commands/validation.rs`
- **validation check proof shape** — validates that a proof command uses a real test runner not just echo  `src/commands/validation.rs`
- **validation cli subcommands** — loom validation add/mark/list for proof commands  `src/cli.rs`
- **validation command entry** — CLI entry dispatching validation add list show update delete and mark subcommands  `src/commands/validation.rs`
- **validation delete handler** — removes a validation node and its VALIDATES edges  `src/commands/validation.rs`
- **validation group matchers** — matches intent signals for rule recommend applies_when scoring  `src/commands/rule.rs`
- **validation mark edge status** — maps mark status to the edge inspection_status value  `src/commands/validation.rs`
- **validation mark handler** — records a manual passed failed or not_run verdict on a validation  `src/commands/validation.rs`
- **validation mark next step** — returns the next_step routing message after a mark operation  `src/commands/validation.rs`
- **validation resolve from list** — resolves a validation by id name or fragment for show delete and update  `src/commands/validation.rs`
- **validation show handler** — renders a single validation with its linked intents and last run status  `src/commands/validation.rs`
- **validation update handler** — updates a validations name description type or command  `src/commands/validation.rs`
- **visibility captured at seed time** — every intent created through the seed ladder is born with visibility set — user_visible from the want stage, internal from the logic stage — instead of left unset and triaged later by the align interview  `src/commands/guide.rs`
- **visual-confirm user-gated queue** — subjective aesthetic residue surfaces as manual_check validations with inspected_by set to human, batched into a user-gated lane in loom session prioritised by user presence like align, rulings and blocked  `src/commands/session.rs`
- **vocab cli subcommands** — loom vocab list/register for controlled terminology  `src/cli.rs`
- **web ui quality rule pack** — seeds web UI view states accessibility XSS and responsive breakpoint rules  `src/commands/rule.rs`
- **web-ui quality rule pack** — seeds web UI view states accessibility XSS responsiveness and design system rules  `src/commands/rule.rs`

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
- **db ensure initialized** — creates .loom/ and initializes the SQLite store if it does not exist  `src/db/mod.rs`
- **db explicit graph static** — session-wide static holding the explicitly pinned graph path  `src/db/mod.rs`
- **db explicit pin** — reads the LOOM_GRAPH env or --graph flag for targeting a non-cwd graph  `src/db/mod.rs`
- **db loom dir resolver** — returns the .loom directory path for the project root  `src/db/mod.rs`
- **db open read handle** — opens a read-only graph handle from the cwd or --graph pin  `src/db/mod.rs`
- **db path resolver** — returns the .loom/graph.sqlite path for the project root  `src/db/mod.rs`
- **db set explicit graph** — stores the explicit graph path from --graph flag for the session  `src/db/mod.rs`
- **db sqlite db path** — returns the .loom/graph.sqlite path for a given root directory  `src/db/mod.rs`
- **endpoint-constrained edge storage** — RELATES_TO, HIERARCHY, IMPLEMENTS, GOVERNS, VALIDATES, TARGETS, SERVES, and JOURNEYS are keyed by endpoint ids with derived stable edge ids and SQLite uniqueness/foreign-key constraints, not stored edge uuids  `src/db/queries/relates_to.rs`
- **graph travel format** — deterministic JSON export and restore-into-fresh-init import so the graph travels with the repo and diffs in PRs  `src/commands/export.rs`, `src/commands/import.rs`
- **interface surface schema vocabulary** — Add the schema/type/export/import vocabulary for a generic interface surface node and a stable call edge, with identity fields that support HTTP endpoints first and leave room for CLI commands, RPC methods, and event topics.  `src/db/schema.rs`, `src/db/sqlite.rs`, `src/types.rs`
- **schema vocabulary and repository boundary** — single-source graph vocabulary declarations (labels, edges, properties, owners, version), shared type structs, graph root targeting helpers, and the GraphReadRepository/GraphReadHandle boundary backed by SQLite  `src/commands/migrate.rs`, `src/db/mod.rs`, `src/db/schema.rs`, `src/types.rs`
- **snapshot analysis and annotation helpers** — pure Rust analysis helpers over typed QuerySnapshot data plus annotation-oriented note/vocabulary/meta helpers; SQLite owns storage and mutation while this layer derives queues, integrity, stats, search, smells, and coverage signals  `src/commands/explain.rs`, `src/db/queries/meta.rs`, `src/db/queries/mod.rs`
- **typed SQLite graph schema** — The SQLite store models Loom's graph as typed node and edge tables with foreign keys, unique endpoint constraints, status CHECKs, queue indexes, JSON-list columns for list facts, and FTS indexes where search uses them.  `src/db/sqlite/schema_ddl.rs`

### developer-experience

- **source corpus coverage** — enumerates structured requirement-like IDs (US-, E-, REQ-, NFR-, INV-, ADR-) in documentation files and reconciles them against the intent graph, surfacing documented-but-unmodeled requirements for inbox triage  `src/commands/corpus.rs`, `src/db/queries/corpus.rs`

### docs

- **code-primary repo wiki machinery** — the v2 repo wiki machinery: a code-primary wiki whose prose links to source files (not intent UUIDs), with the intent graph as an invisible manifest that certifies the prose via coverage, freshness, and graph-aware consistency gates; hard-cut replacement of v1's graph-primary OKF emitter and the flat loom.wiki.md  `docs/repo-wiki-ladder-proposal.md`
- **code-primary wiki emitter** — the v2 emitter: replaces v1's graph-primary OKF skeleton with a code-primary manifest layer (frontmatter sourceFiles+symbols+provenance) and a prose layer (file-path links, no intent:UUID in reader-facing prose); hybrid structure of bounded topical pages + per-component module pages  `src/commands/wiki.rs`
- **graph-aware manifest resolver** — the invisible manifest backbone: resolves file-paths to intents at check time so the consistency gate (the one Qoder cannot do) still falsifies relational claims against typed graph edges even though the reader never sees an intent UUID  `src/commands/wiki.rs`
- **storage documentation and guide refresh** — README, guide, command docs, retired serve notice, and build/install guidance describe SQLite storage accurately and omit obsolete legacy lock/query caveats.  `README.md`, `docs/COMMANDS.md`, `docs/CONTRIBUTING.md`, `docs/daemon-design.md`, `src/commands/guide.rs`

### graph-integrity

- **hypothesis prove records TARGETS confidence** — When a hypothesis is proven, the TARGETS verdicts it stamps carry the prover's explicit confidence instead of leaving the uninspected 0.0 default behind.  `src/cli.rs`, `src/commands/hypothesis.rs`, `src/db/sqlite/edge_writes.rs`

### health

- **completeness and integrity checking** — vertical spine (tree shape + leaf realization + file reach), coverage reconciliation against disk, doctor audit of schema conformance and provenance lanes  `docs/maturity-ladder-proposal.md`, `src/commands/complete.rs`, `src/commands/coverage.rs`, `src/commands/doctor.rs`, `src/commands/hotspots.rs`, `src/commands/impact.rs`, `src/commands/report.rs`, `src/commands/status.rs`, `src/db/queries/completeness.rs`, `src/db/queries/comprehensiveness.rs`, `src/db/queries/integrity.rs`, `src/db/queries/maturity.rs`, `src/db/queries/maturity/tests.rs`, `src/db/queries/stats.rs`, `src/db/queries/stats/tests.rs`, `src/db/queries/symbol_accountability.rs`
- **composition proof coverage** — composition_coverage_from_snapshot classifies active intents as path-proven (assembly exercised by a saga or multi-intent spanning proof), leaf-only (unit tests only), or unproven; never gates green, always additive  `src/db/queries/composition.rs`
- **derived problem signals** — loom smells: twins, overlapping ownership, scatter, tangles, undeclared coupling, recurrence, normative gaps — computed from graph structure, each with a remedy  `src/commands/domain.rs`, `src/commands/layer.rs`, `src/commands/smells.rs`, `src/db/queries/smells.rs`, `src/db/queries/smells/normative.rs`, `src/db/queries/smells/physical.rs`, `src/db/queries/smells/semantic.rs`, `src/db/queries/smells/source_fact_tests.inc`
- **report command handler** — opens the graph and renders the full health report envelope  `src/commands/report.rs`
- **report data struct** — holds the assembled report sections before rendering  `src/commands/report.rs`
- **report human renderer** — prints the prose report with centrality and edge-status sections for agents  `src/commands/report.rs`
- **report json renderer** — renders the structured report envelope with capped list fields for --json mode  `src/commands/report.rs`
- **report list cap constant** — caps per-section report lists so agents see headlines not thousands of rows  `src/commands/report.rs`
- **report snapshot assembler** — assembles status, gaps, and blocked-validation slices from a query snapshot  `src/commands/report.rs`
- **report text truncator** — truncates long evidence and criterion strings at unicode character boundaries for human report lines  `src/commands/report.rs`
- **self-calibrating smell thresholds** — tukey_upper_fence derives outlier/far-outlier bounds from a repo's own owner-count distribution; replaces hardcoded magnitude constants so tangle and coupling detectors adapt to any repo  `src/db/queries/calibrate.rs`
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
- **synthetic graph benchmark regression tests** — Regression tests prove gen_synth_graph add_hier and add_impl emit hierarchy and IMPLEMENTS edges in generated benchmark graphs; validations run the test methods, not production helpers.  `tests/gen_synth_graph_test.py`

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
- **agent intake discipline (capture out-of-band findings)** — loom's driving protocol and every role charge teach the agent to capture an out-of-band finding (debt, ambiguity, an unowned gap, a scope question) as a triageable inbox lead rather than fix it silently or decide inline; the intake boundary refuses agent gap-laundering the same way the door refuses user prose-laundering  `src/commands/guide.rs`
- **bounded tag vocabulary** — loom vocab maintains a small normalized tag registry for intent tags; write-time validation inlines the registry and drift remedies merge near-duplicate keys so discovery and smells get deliberate collisions.  `src/commands/vocab.rs`, `src/db/queries/vocab.rs`
- **discovery queue ranks pairs by plausibility** — unexplored RELATES_TO pairs should be ranked beyond raw centrality (e.g. shared files, shared domain, co-change) so a driver is not pointed at 31 equally-scored pairs on a 10-intent graph  `src/db/queries/scoring.rs`
- **harness orchestration affordances** — loom exposes scheduling-advice facts (work slices, conflicts, parallel-safety class, strength tier) and an orchestrator topology hat so any external harness can safely fan out subagents; loom informs and advises while the harness executes
- **horizontal work slices** — loom slice plan computes conservative disjoint territories (clusters of related intents plus their codefile footprint) from existing cluster, smells and impact signals, with cross-slice conflict edges, so parallel agents never receive overlapping territory  `src/commands/slice.rs`
- **hypothesis lifecycle commands** — loom hypothesis add/list/show/prove/adopt/reject drives the state machine with gates: claim and predicted_outcome must be substantive and falsifiable, the prover's provenance must differ from the proposer's, only a proposed hypothesis can be proven, and only a supported one adopted. Every transition is recorded as an append-only note.  `src/cli.rs`, `src/commands/hypothesis.rs`
- **hypothesis list formatter** — formats hypothesis rows for list command output  `src/commands/hypothesis.rs`
- **hypothesis list handler** — lists hypotheses from sqlite with filters  `src/commands/hypothesis.rs`
- **hypothesis node and TARGETS edge** — Schema v4 adds a Hypothesis node (claim, proposal, predicted_outcome, status: proposed/supported/refuted/adopted/rejected, provenance) and a TARGETS edge (Hypothesis to Intent) carrying the standard inspectable meta. Both persist through the deterministic export/import round-trip with two-phase validation, and loom doctor audits their required properties and value vocabularies.  `src/db/schema.rs`, `src/types.rs`
- **hypothesis not found message** — shared error when hypothesis id or name lookup fails  `src/commands/hypothesis.rs`
- **hypothesis plane** — The pre-decision plane: improvement hypotheses that any lane can propose, an analyzer proves against current code, and a builder adopts into planned intents. Speculation stays invisible to coverage and completeness until adoption converts it into the existing lifecycle.
- **hypothesis show handler** — shows one hypothesis with transitions and evidence  `src/commands/hypothesis.rs`
- **indirect-wiring inspection discipline** — loom teaches that import analysis sees only static wiring; indirect wiring (event pub/sub, DI/registry, config-keyed dispatch, RPC/queue) shares only a string key or type and must be hunted while grounding, recorded as a manual RELATES_TO, proven by a saga, and captured as a lead when incomplete — wiring completeness is a judgment+proof axis loom cannot compute mechanically  `src/commands/guide.rs`
- **model-neutral strength tier on work packets** — every work packet from next and slice plan carries a model-neutral capability tier (effort low mid high plus a risk facet) derived from centrality, lane effort and rule inspection effort, so a harness maps difficulty to its own model roster without loom naming a vendor model  `src/commands/next/scoring.rs`
- **orchestrator topology hat** — loom guide --mode orchestrate serves a spawn-agnostic driving protocol: read the fact surface, dispatch disjoint slices, hand each subagent its role charge plus territory boundary, sync after code edits, then re-plan; it names no spawn mechanism and no model, states that loom workflow supersedes repo workflow docs, and keeps the orchestrator out of the gated write-lane roles  `src/commands/guide.rs`
- **parallel-safety classification** — each slice and packet is classified safe, exclusive-slice or serial, plus human-gated and blocked, conservatively so that when loom is unsure it marks conflicting, letting a harness know which work may run concurrently  `src/commands/next/scoring.rs`
- **priority-scored work queues** — loom next: one queue per agent role (discovery/fix/build/validate/quality), scored by centrality + urgency - staleness; returns one item with full context so no second lookup is needed  `src/commands/cluster.rs`, `src/commands/next/refactor.rs`
- **scale benchmark harness** — The run-loom benchmark scripts generate a synthetic graph and fail when status, next, smells, or next --all exceed the hot-command budget.  `.claude/skills/run-loom/bench.sh`
- **shared graph query snapshot layer** — A read-only graph projection bulk-loads shared node and edge slices once and serves status, next, report, doctor, and smells through a common query surface.  `src/db/queries/snapshot.rs`
- **slice-scoped work queue** — loom next accepts a slice filter to restrict any mode queue to one slice territory, so a dispatched subagent only sees work inside its assigned boundary  `src/commands/next/slice_filter.rs`
- **smells propose hypotheses** — Structural findings that call for redesign rather than a patch (recurrent trouble, scattered intents, twin intents) emit loom hypothesis add as their remedy command, so the graph's own signals feed the proposal plane instead of dying in notes.  `src/db/queries/smells.rs`, `src/db/queries/smells/lifecycle.rs`
- **stale hypothesis evidence ripples on sync** — TARGETS edges on supported hypotheses flip to needs_reverification when a target intent's grounded code changes, with a transition note naming the file. Support earned against old code must be re-earned, exactly like RELATES_TO and GOVERNS claims.  `src/commands/sync.rs`
- **sync flag engine** — mtime-delta detection propagating one hop: RELATES_TO neighbours and passing GOVERNS go needs_reverification, linked validations go not_run; files missing on disk are reported  `src/commands/sync.rs`
- **synthetic graph hierarchy edge builder** — Helper that deduplicates parent-child pairs while generating synthetic benchmark graph hierarchy edges.  `.claude/skills/run-loom/gen_synth_graph.py`
- **synthetic graph implements edge builder** — Helper that deduplicates and records IMPLEMENTS edges while generating the synthetic benchmark graph.  `.claude/skills/run-loom/gen_synth_graph.py`
- **triage queue for hypotheses** — loom next --mode triage serves proposed hypotheses ranked by target-intent centrality as optional work items - like discovery and review, triage never blocks phase=complete. loom next --all shows the triage count flagged optional.  `src/commands/next/modes.rs`
- **wiki lane and self-teaching authoring loop** — the work lane and self-teaching loop: loom next --mode wiki surfaces uncited salient nodes, stale provenance stamps, and fabricated links; for foreign repos loom inits the target graph then orders the LLM to author, with the graph as the invisible manifest  `src/commands/wiki.rs`


<!-- loom:prose-start -->


















<!-- loom:prose-end -->
