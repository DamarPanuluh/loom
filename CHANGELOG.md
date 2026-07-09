# Changelog

Notable changes to the `loom` crate. Versioning follows [semver](https://semver.org):
**patch** = bug fixes, **minor** = backward-compatible features, **major** = breaking
changes. (`SCHEMA_VERSION` in `src/lib.rs` is separate — it versions the on-disk graph
schema, not the crate.)

Bump with `scripts/release.sh <patch|minor|major> "<summary>"` — never hand-edit the
version.

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
