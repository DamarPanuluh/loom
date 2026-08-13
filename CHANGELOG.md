# Changelog

Notable changes to the `loom` crate. Versioning follows [semver](https://semver.org):
**patch** = compatible bug fixes; before 1.0, **minor** may deliberately break an
unstable surface; at or after 1.0, **minor** is backward-compatible and **major**
is breaking. (`SCHEMA_VERSION` in `src/lib.rs` separately versions the on-disk graph.)

Bump with `scripts/release.sh <patch|minor|major> "<summary>"` — never hand-edit the
version.

## [0.31.4] - 2026-08-13
- regenerate the committed v12 dogfood export with compiler-v6 Journey validations and restore the release gates
  - `loom.graph.json` predated the compiler's `surface_locator` exercise facet, so the release gate's doctor pass reported `broken_journey_proof_chain` for every imported Journey proof. The export was rebuilt with the current compiler (init → import → sync → surface-accept → `journey compile` for all 30 Journeys → export), so the imported graph now carries complete Exercise chains and the dogfood/fixpoint gates pass end to end (`dogfood: OK — 30 Journey(s) structurally current`).
  - The regenerated export carries exactly one `tests/**` ignore rule, and the four `journeys/surfaces/*.surface.json` fixtures were refreshed for the current `src/sync.rs` fingerprint (the previously committed `expected_hash` was stale).
  - The API-boundary test's four consumer `cargo check` builds now serialize on one mutex, removing a spurious ENOENT race against a shared `CARGO_TARGET_DIR` inside release-gate snapshots.

## [0.31.3] - 2026-08-13
- close the Journey settlement trust boundary at the root: only the Store-owned guarded runtime mints S3-eligible Journey evidence; compiler v6
  - Settlement now accepts only observations minted by the Store-owned compile → execute → settle entrypoints (`run_and_settle_compiled_validation`, `run_interactive_and_settle_compiled_validation`, `resume_and_settle_compiled_validation`), which derive the canonical proof, execution root, coverage, and executable boundary from the store itself. The harness guard stays alive across compilation, execution, the post-execution recheck, and settlement. The public execution APIs (`execute`, `execute_observed`, `execute_interactive`, `resume_interactive`) produce ordinary untrusted reports that settlement refuses — an exact canonical proof executed in an attacker-controlled root (fake relative executable or PATH shim) can no longer settle against the trusted store.
  - Evidence now binds to what actually executed: covered-file hashes are captured immediately before execution, rechecked immediately after it, persisted verbatim into the run record (never resampled at settlement), and settlement refuses on any drift in the root, operation-exercise projection, proof bytes, executable boundary, or covered hashes. A covered file modified between an interactive run and its resume refuses settlement without consuming the token.
  - Run evidence persisted by settlement carries a `locally_minted` marker; a local store reload re-mints trusted assertion provenance only for marked rows. Imports were already downgraded to prose and stay that way.
  - Journey compiler version is now **6**. Compiler-v5 proofs are not current: `loom sync` resets them, and they must be recompiled and rerun to earn S3. Outstanding human-gate continuations use runtime schema v2 and must be re-issued.
  - `journey surface` templates now emit typed `output.captures` for every authored `produces` entry, no longer fabricate `exercises` (downstream-process provenance must be authored), and state clearly that human-gated Journeys require structural editing. Full `SurfaceManifest` validation coverage added (outputs, downstream inputs, optional exercises, multi-step, human decisions).
  - Release inventory regenerated for the current source tree (264 entries) and the four `journeys/surfaces/*.surface.json` fixtures refreshed the stale `src/sync.rs` expected_hash.

## [0.31.2] - 2026-08-13
- close remaining Journey S3 trust-boundary gap for caller-authored compiled proofs; schema v12 graphs require no rebuild
  - Settlement recompiles the current accepted surface and requires exact canonical proof-byte equality before trusting a sealed observation. `execute_observed` may still run a deserialized proof for diagnosis; that observation cannot settle or earn S3.
  - The generated `journey surface` template now emits operations and bindings from the authored Journey steps. Callers still replace only repository-specific CodeFile keys and locators.

## [0.31.1] - 2026-08-13
- harden compiler-owned Journey assertion evidence so only a local compiler-owned run can earn S3; schema v12 graphs require no rebuild
  - Trusted assertion provenance is minted only from a sealed Journey observation after the compiler-owned runtime actually executes. Public Deserialize, `Assertion::observed`, caller-built `RuntimeReport`s, generic command validation, and imported graphs cannot create S3-eligible Journey assertion evidence.
  - Journey compiler version is now **5**. Compiler-v4 proofs are not current: `loom sync` resets them, and they cannot retain S3 or proven readiness. Recompile and rerun every Journey proof after upgrading.
  - The generated `journey surface` template is a complete valid manifest once repository-specific CodeFile keys and locators are replaced; it no longer emits a dangling setup block.

## [0.31.0] - 2026-08-12
- add compiler-owned Journey operation exercises for cross-process S3 proof entries; schema v12 graphs require no rebuild
  - Optional `CliOperation.exercises` (`id`, `codefile`, `locator`, `observed_by`) declare downstream boundary entries without changing surface ownership.
  - `journey compile` creates Exercises topology for bound operation exercises, aggregates same-CodeFile locators, and stores provenance facets.
  - S3 remains derived: a downstream entry is eligible only when its observed assertion passed and call-graph reach finds a realizing symbol.
  - Journey S2 guidance never recommends `loom edge exercises`; doctor rejects malformed exercise provenance.

## [0.30.1] - 2026-08-12
- clarify cold-LLM Journey guidance and protect compiler-owned proof topology; schema v12 graphs require no rebuild

## [0.30.0] - 2026-08-12
- ship Journey authoring lint, portable release trust seams, and proven 30-Journey rehearsal closure

**Breaking pre-1.0 release.** Schema v12 replaces executable Journey proof specs
with authored Journey roots. Loom refuses every older SQLite graph and
`loom.graph.json` export untouched; there is no in-place migration. Translating
operations, endpoints, or old step-to-Intent references into authored user
meaning would fabricate product judgment.

Rebuild after upgrading:

1. Preserve the old export only as historical reference, then initialize a new graph.
2. Register repository code and use `loom bootstrap suggest` for non-authoritative clues.
3. Author strict `loom.journey/v1` artifacts with stable IDs, actors, semantic actions,
   expectations, typed inputs/outputs, and optional declarative profiles.
4. Add each Journey and inspect `loom journey derive`; a human must authorize every
   exact hash-bound `loom.journey-derivation/v1` manifest before `derive-accept`.
   The strict manifest records a proposal ID/rationale, explicit create-or-reuse
   Intent operations, criteria/rationales, relationship reconciliation, and no
   unresolved question. Acceptance records an adopted Proposal; identical replay
   is idempotent.
5. Implement and ground the accepted technical Intents, build the real target-repository
   CLI, and accept its structured `loom.journey.surface/v1` projection.
6. Compile and run the selected Journey profile to establish the compiler-owned S3 proof.

- Adds first-class `Journey` roots and asserted `Derives`, `Surfaces`, and `Proves`
  topology. Semantic edits invalidate their hash-bound projections.
- Adds `journey add/show/list/remove/map`, read-only `derive` and `surface` packets,
  human-gated `derive-accept`, atomic `surface-accept`, and compiler-owned
  `compile/run/diagnose/freeze/drift` lifecycle.
- Removes executable authored steps, transport-specific Journey artifacts, direct
  Journey metadata on `validation add`, and the old coverage/invariant/prompt families.
- Converts all 27 dogfood artifacts to strict semantic roots.
- Runtime provenance now separates write authority (`LOOM_AGENT=llm:<lane>`) from self-declared executor attribution (`LOOM_AGENT_PROFILE=loom-auditor`, etc.). One typed identity is resolved before locking and propagated to facts, journals, graph/proof lock holders, and `whoami`; profiles never grant authority, and `whoami` reports their source and verification status. Adjudications and verdicts retain the canonical lane instead of collapsing to `llm`; bare, empty, noncanonical, and unknown authority values fail closed. Schema v11 stores absent `fact.asserted_profile` values as SQL `NULL`, leaving legacy rows honestly unprofiled.
- Read-only graph commands wait up to ten seconds for an in-flight writer while competing writers remain fail-fast at two seconds, preventing routine `loom status --json` calls from surfacing transient lock-contention failures.
- TypeScript `export const <name>` locators resolve consistently across syncs.
- Grounding creation refuses role collisions and locator changes on inspected edges instead of mutating settled evidence; same-role pre-verdict re-grounding remains available, while settled changes require explicit `set-role`, `set-locator`, or removal.
- Duplicate-intent rectify clears persist for the exact pair of intent descriptions and reopen only when either description changes.
- Adds read-only `loom checkpoint recommend` for an explicit Intent or cohesive bundle. It fails closed unless the selected work is implemented and ratified, relevant validations pass, sync and doctor are clean, the export is fresh, and the Git diff maps unambiguously to the selected scope; successful output lists exact included and excluded paths, checks, and a deterministic suggested message without staging, committing, or pushing.
- Semantic checkpoint guidance permits an acting LLM to make or defer an exact-path local commit, never `git add -A`; pushing still requires a current human decision bound to the repository, remote, branch, and commit. Git history remains evidence outside Loom truth, and no change-count heuristic is used.
- Adds read-only stable source-anchor issuance through `loom codefile anchor`; it prints the exact marker without editing source or graph state. Sync validates inserted anchor locators through the shared locator policy, preserving valid anchors while surfacing genuinely stale groundings.

## [0.29.2] - 2026-08-09
- make validation registration atomic and preserve unchanged clean quality scans

## [0.29.1] - 2026-08-06
- restore host-mediated human decisions in release builds

## [0.29.0] - 2026-08-03
- locator drift re-opens groundings that name nothing, shared proof commands are reported, ordered steps gate readiness, and loom answers what stands on a behavior

## [0.28.0] - 2026-08-01
**Breaking** (minor bump because 0.x): all 16 paginated `list --json` commands now
return a self-describing `{ "items": [...], "pagination": {...} }` envelope
instead of a bare array. Pagination metadata includes the requested offset and
limit, returned and total counts, and an explicit `has_more`/`next_offset` pair;
the final page sets `next_offset` to `null`. Migrate JSON consumers from
`response[]` to `response.items[]`.

## [0.27.0] - 2026-07-26
**Breaking** (minor bump because 0.x): schema moves v3 → v4. Older binaries
refuse a v4 graph and say to upgrade. Run `loom sync --rebuild` after upgrading —
see below for why that is not optional this time.

- **A test runner's own summary is evidence about the output.** `cargo test`
  reporting "4 passed; 0 failed" states WHAT it checked, which is strictly more
  than `exit_code: 0`. It previously graded S1 with "content assertions: 0"
  while a journey asserting one substring graded S2 — loom was telling every
  repo that its real test suite established only liveness. Counted
  conservatively now (a positive pass count AND explicit zero failures), and
  kept separate from assertions declared in a spec, because a spec's
  expectations are checked BY loom while a summary is the tool reporting on
  itself.
- **Calls inside macros are extracted.** A macro's arguments parse as an
  unstructured token tree, so `assert_eq!(effective(a, b), c)` was invisible.
  In Rust that hid almost every call a test makes — exactly the calls the S3
  call witness reads — so every Rust suite was invisible to it.
- **`loom sync --rebuild`.** Sync only re-derives files whose CONTENT changed,
  so upgrading loom silently kept the old binary's call graph, symbol map and
  findings. `wipe_derived + sync` is the INV-2 operation named in three doc
  headers and there was no way to invoke it. Asserted truth is untouched.
- The `proven` rung requires every implemented leaf at S2, as designed but
  never implemented. A passing proof that establishes only liveness no longer
  satisfies it.
- The shallow-proof smell gates on proof STRENGTH rather than a `proof_kind`
  label — S3 states "reaches the code it proves", checked from the call graph,
  which is what the label stood in for.
- Retirement is total: a retired behavior contributes no residue, no proof
  debt, no ownership and no smells. Its code stays registered and becomes
  visible coverage work rather than quietly counting as covered.
- `loom observe` releases the graph lock before running its command. Holding it
  made any child that opened the graph — including every `loom journey run` —
  block and exit non-zero, which loom recorded as a FAILING verdict against a
  passing behavior.
- Migration 4 strips `proof_level` from validation bodies: dead since strength
  became derived, and printed directly above the derived grade it contradicted.

Together these move this repo from 59 unproven implemented leaves to 5, and 13
journey gaps to 7 — not by lowering a bar, but by reading evidence that was
already in the tree.

## [0.26.0] - 2026-07-25
**Breaking**, despite the minor bump: this is 0.x, so a major would mint 1.0.0
and claim a stability this codebase has not earned. Read the removals before
upgrading — the schema moves v2 → v3 and old binaries refuse a v3 graph (they
fail closed, and now say to upgrade rather than to migrate).

- Evidence spine: nothing counts that loom cannot re-check. Every asserted fact
  carries typed evidence — a run loom performed, a fingerprinted span, a journal
  entry it wrote — and `assert_fact` is the one door. Prose is recorded and never
  counts. Migration 3 ERASES every existing asserted verdict, because not one of
  them was anchored: 39 of 51 ratifications had no journal entry behind them, and
  54 of 59 proofs were "passing" because an agent said so.
- Anchoring floors per claim kind, each naming the command that reaches it — and
  each declining to demand an anchor where none could exist (an absence, an
  ungrounded relationship, a finding that flags no file).
- Proof strength S0–S5, derived from the proof's shape rather than claimed by its
  author. `exit_code` is deliberately not a content assertion; counting it is how
  a one-step journey became the strongest evidence class in loom's own graph.
- `sync` re-verifies every anchor in one pass; the ~470-line ripple matrix is
  gone. Symbol-scoped sparing survives because a locator run re-resolves its
  symbol and fingerprints its body.
- Wantedness is earned: `de_facto` from three falsifiable conjuncts, so an agent
  can work all night and the human is asked only where judgment and evidence
  diverge. `loom intent reject` is the cheap other half of the authority.
- New: `loom observe` (wrap a command, keep what loom saw), `loom impact`,
  `loom absorb`, `loom audit` (including `--efficacy`), `loom deepen`,
  `loom decide`. MCP serves all of them in-band.
- The ladder no longer ends. `deepen` is the top rung and is permanently open —
  there is no `complete`.

**Removed**: `--proof-level` and the L0–L6 vocabulary (strength is derived);
`intent ratify --by-policy`, `loom policy ratification`, and the whole
ratification-policy engine (approval by attribute is strictly worse than approval
by evidence); `waiver:proof` and `waiver:journey` (proof is not waivable);
`record_verdict`/`record_finding_verdict` as public store writes.

## [0.25.0] - 2026-07-19
- ratification + lived graph (rethink rings 1-5)

## [0.24.0] - 2026-07-08
- Unify ownership smells on graph connectedness: `tangled_file` fires when ≥2 realizing owners of a file are not one connected neighborhood (relates/hierarchy/scenario-of/…); retire the `max_file_owners` count gate and fold former `overlapping_ownership` into the same rule; legacy `max_file_owners` in exports is ignored on load
- Operator-feedback precision release: symbol-scoped staleness (sync keeps a per-symbol fingerprint map per codefile and spares realizing groundings whose locator symbol the change did not touch — reported as edges_spared; same-named symbols fold into one fingerprint, and no-locator/unresolvable-locator groundings stale file-scoped as before), evidence anchoring (every verdict stamps a fingerprint of each cited file:line span as an asserted evidence_spans facet; sync grades re-opens 'cited evidence intact, cheap re-confirm' vs 'cited evidence rewritten, full re-inspection', a rewritten cited span re-opens even an unchanged-symbol grounding, and citing lines that never existed fails closed at record time), and the vague_intent smell (an active intent whose description hedges without one observable outcome materializes as an adjudicable finding)

## [0.23.0] - 2026-07-07
- docs-reality reconciliation (wiki/federation/apply/policy shipped surfaces, hardened-grid + INV-5 invalidation design notes), INV-1 + INV-3 gate/queue invariant tests, plane/contract doc headers on all modules, dependency modernization (rusqlite 0.40, tree-sitter 0.26, reqwest 0.13, std file locking replaces fs2, serde_norway replaces deprecated serde_yaml), honest journey non-JSON-body failures, dogfood graph completed to the full maturity ladder (34 intents, 54/54 files owned, 35 passing proofs, all six rungs met)

## [0.22.0] - 2026-07-06
- loom mode <owned|observed> sets the graph mode after init (build/fix/coverage/elaborate lanes on/off); sync never touches it, so the observed flag is reachable instead of orphaned. loom next --mode <m> --all lists a single queue's full depth — every item it would serve, in priority order, as lightweight rows — so a queue status reports as hundreds deep is pageable (the singular next still serves the full top packet)

## [0.21.2] - 2026-07-06
- Compass never routes to an empty or disabled lane: implemented-but-ungrounded intents are now served by the build lane and counted in queues.build via a shared predicate with the realized rung; build/coverage route on queue counts so an observed graph is never pointed at a force-disabled lane; loom next --all surfaces per-queue depth ([n] markers + additive queue_counts in JSON)

## [0.21.1] - 2026-07-05
- Maturity ladder display honors bottom-up order: rungs above the lowest unmet rung render as blocked (derived blocked/blocked_by fields) instead of showing a higher rung as satisfied above an open lower rung; each rung's own state is unchanged

## [0.21.0] - 2026-07-05
- PoC/experiment evidence reaches the build packet: hypothesis adopt copies proposal+prediction+proof-evidence onto the spawned intent, task add --target lands close/abandon outcomes as notes on the target intent, node/edge packets inline the adjudication trail via Store::notes_for (cap 6 + overflow read), and the prove lane self-teaches the supported->adopt->build handoff (prove command next_step, prove packet, build purpose names notes as prior record) so a proven idea is never stranded

## [0.20.0] - 2026-07-04
- loom wiki — reader-first documentation as a tracked projection: wiki plan grounds a draft page in the intents it documents, wiki next emits a verified brief (documented intents' descriptions, groundings, proof status) for an agent to write reader-first prose, wiki record marks it fresh (gated on the prose existing), and sync stales a page precisely when a documented intent, its code, or its proof drifts; adds a WikiPage node + Documents edge — the graph governs truth and freshness, never layout

## [0.19.1] - 2026-07-04
- Fix: loom journey add is idempotent — upsert by journey_id (dedupe duplicate validation nodes, reset proof to not_run on spec change, reconcile step Validates edges); journey run tolerates and repairs existing duplicates by merging their links instead of failing 'add it first'; add loom journey remove <id>

## [0.19.0] - 2026-07-04
- Build lane is requires-aware: serve a prerequisite before the intent that requires it (unmet = a requires target not yet implemented, matching the completeness prerequisites axis); a fully-blocked candidate is served with a blocked reason so a requires cycle never stalls the lane

## [0.18.0] - 2026-07-04
- Domain-blind engine core: extraction+scan behind a Deriver seam (sync orchestrates seed derivers), proofs behind a ProofRunner seam, exports behind a Projection seam, code files behind an ArtifactClass ground-artifact contract, truth-class membership as declared registry data, evidence policy as portable config with the new loom policy command, and a derived-floor balance surfaced in loom status

## [0.17.0] - 2026-07-04
- loom apply gains adjudications/vocab/tags batch sections — durable finding verdicts, vocab registration, and intent tagging in one atomic transaction, each through the same gate as loom finding verdict / vocab add / intent tag add (shared adjudicate_finding + tag_intent); applied in dependency order (vocab first, tags last)

## [0.16.2] - 2026-07-04
- Add loom threshold list/set/reset — hand-set the structural finding gates as portable config.thresholds (manual counterpart to calibrate); reset drops the key so gates revert to shipped defaults rather than a pinned snapshot; checked value conversions, calibrate output now shows max_file_owners

## [0.16.1] - 2026-07-04
- Make the tangled_file owner-count gate configurable: TANGLE_OWNERS const becomes Thresholds.max_file_owners (default 2, strict > — behavior-identical to the old >=3), portable in config.thresholds; calibrate preserves it rather than fitting owner counts

## [0.16.0] - 2026-07-04
- Detector deepening: per-symbol metrics (arg count, nesting depth; complexity now computed for Python/Go/JS/TS), new large_symbol/deep_nesting/excess_args findings, thresholds as portable config with loom calibrate (repo-fitted gates from the worst-tail quantile), scan adapters gain --format json (field=path maps, dotted paths, items= envelopes — pulse/qualirs-ready)

## [0.15.0] - 2026-07-03
- Convergence release: one verdict grammar (positional outcomes on rule verdict / validation verdict / hypothesis prove), validation run replaces top-level validate, remove/unlink replace delete/ungovern, intent update absorbs set+mark (ripple decided by fields, uniform --reason), intent tag typed, surface gaps absorbs interface gaps, value-enum next --mode and guide --role, intake routing taught by guide/session/door, smells materialize as adjudicable findings (durable loom finding verdict across syncs); legacy stripped: saga alias/type/spec key, --status/--result/--verdict flags, mark/delete/ungovern spellings

## [0.14.0] - 2026-07-03
- Grounding roles on implements edges (realizes|consumes|configures|verifies): edge --role / set-role / rehome, consumes never owns coverage, seam-drift-only staleness, coverage-packet disambiguation, consumer_owned_file smell, consumes_without_seam doctor gate; exposes is asserted-only; audit hardening: fail-closed LOOM_AGENT, independent verdicts require criterion+evidence, redefine ripples implements groundings, sync preserves scan findings, observed queue counts, import format check, ValidationType/journey wiring, edge-show facets

## [0.13.0] - 2026-07-02
- Field-report fixes: fix queue is strictly failing-verdict repair — stale claims reroute to analyze (stale-first) and fixer packets never carry verdict authority, with compass/session/queue-counts on the same partition; journey invariant update --asserts re-points the asserts edge in place (node + note trail preserved); notes attach to edges (node-first resolution, honest no-match error); default scan parser pairs svelte-check-style two-line diagnostics, custom --map stays per-line

## [0.12.0] - 2026-07-02
- Drive-tested smoothing: complete CRUD surface on every graph family (note/task/hypothesis/rule/vocab/proposal/journey-coverage/invariant/intent/scan gain update-rename-remove where semantics allow; retire stays the supersession path), intent rename via update --name, truthful per-queue backlog in loom status (same partition loom next serves), pack_drift smell (seeded rules vs shipped pack, idempotent re-seed remedy), ghost-file handling (missing-file coverage packet, GONE annotations in read sets, never-hashed deletions ripple once via missing_rippled marker), quiet SIGPIPE exit

## [0.11.0] - 2026-07-02
- Sensors + Definition-of-Complete: loom scan adapters (any language's linters/checkers become derived findings, config travels in the export), regex pattern pre-screening in quality packets (confirm-or-refute hits), completeness scorecard with six axes + waivers (re-open on redefinition), elaborate queue growing the surroundings humans forget (scenario families via --aspect, prerequisites, batched open questions for the human), portable config in loom.graph.json (layer order/ignores/globs/adapters survive import), workitem split into cohesive submodules

## [0.10.0] - 2026-07-02
- PM rebuild for weak-worker fidelity: journey absorbs saga (one executor, hidden alias); review queue with confidence<0.7 routing; disjoint fix/quality/validate queues; self-contained work packets (read_set, inline descriptions, stale causes, prefilled quoted write-backs); per-axis correct_when criteria in guide+packets; door landing menu; inbox show/link/status + positional mark; loom note; write-back pulse (--json + next everywhere); 6 quality packs with few-shot examples; sync stales targets edges + records stale causes; deterministic validation resets

## [0.9.0] - 2026-07-01
- Global --json across all read/show/list commands; split commands.rs and cli.rs into cohesive submodules

## [0.8.0]
- **Full CRUD completeness.** UPDATE: `edge set-locator`, `intent set`, `surface update`,
  `validation update`. READ: `validation|hypothesis|task show`, and `intent show` now
  surfaces level/visibility/tags. DELETE convenience: `validation unlink`,
  `rule ungovern`, `rule add`/`remove`, `edge remove --reason`, `intent reactivate`.
  New `set_node_body` store primitive.

## [0.7.0]
- **Edge deletion + delete-completeness.** `edge remove` (asserted-only; refuses derived),
  plus `codefile|validation|surface|vocab|ignore|inbox remove`. Fixed `delete_node`
  orphaning edge-scoped facets/tags — it now deletes incident edges in a transaction with
  a derived-node guard.

## [0.6.0]
- **Integration monitoring:** surface-plane ripple in `sync`, `edge call`,
  `guide --role monitor`, `codefile rescan`, upstream-deletion ripple. **Non-Rust
  extraction** (Python/Go/JS/TS). `--observed` wiring (build/fix lanes off on observed
  graphs). Post-judgment `→ next` follow-up. Fixes: atomic `validation mark`,
  short-`[id]` resolution. (Version reconciled up from 0.1.0 to sit above the prior global
  0.5.0.)
