# PLAN: whole-codebase quality sweep (reuse / simplification / efficiency / altitude)

Started 2026-08-23. Mode: **solo, phased**. Driver keeps the code in its own
attention; no subagents — fanning out would defeat the point of the request.

## Goal

Apply the four `/simplify` lenses to **every** file in `src/`, then `tests/`.
Not a bug hunt (`/code-review` owns that). Quality only:

- **Reuse** — new/duplicated code that re-implements something the repo already has.
- **Simplification** — redundant or derivable state, copy-paste variants, deep
  nesting, dead code.
- **Efficiency** — repeated I/O or computation, sequential independent work,
  blocking work on hot paths, closures that pin large scopes alive.
- **Altitude** — special cases layered on shared infrastructure where
  generalizing the mechanism is the real fix.

## Contract

- **Phase = read + judge only.** Findings append to `gates-cleanup/findings.md`.
  No edits during read phases; a phase that edits cannot be re-run cleanly.
- **Fix waves come after the read sweep**, grouped by mechanism, not by file, so
  one duplicated helper is killed once everywhere rather than patched per site.
- **Zero behavior change.** Public paths (`use loom::x::Y`) keep working.
  Anything that changes behavior is recorded as a finding and skipped, not applied.
- **Never edit an existing test to make it pass.** A test that fails after a fix
  means the fix was wrong.
- **Verification**: `cargo check` per fix wave; `cargo test` at each branch gate;
  `scripts/local-ci.sh` (= `cargo build` + `scripts/dogfood.sh --check`) at the end.
- **Findings ledger is append-only.** It is the trace that survives a context
  refresh. Restore context by reading it, not by re-reading the code.

## Finding format (one line per finding, in findings.md)

    [P<n>] <file>:<line> | <lens> | <summary> | COST: <what it costs> | FIX: <the smaller form>

## Phases

Read sweep over `src/` — 17 phases, ~3.3 MB total.

| # | Name | Files | ~KB |
|---|---|---|---|
| P1 | root-core | lib, main, model, cli, commands, limits, text, packet, deriver, statistics, signal, identity, grammar, artifact, policy, truth | 132 |
| P2 | root-exec | scan, subprocess, runner, proof, harness, prescan, callgraph, locator, anchor, fsglob, registry | 259 |
| P3 | root-truth | evidence, audit, journal, checkpoint, divergence, thresholds, maturity, ratification, review, risk, coverage | 300 |
| P4 | root-flow-a | packs, batch_auth, candidate_surface_policy, sync, mcp | 212 |
| P5 | root-flow-b | journey_gate, travel, journey_exercises, lane, seed, rolelease, federation, absorb, pattern, research | 197 |
| P6 | store-a | store/facts, store/derived, store/schema | 178 |
| P7 | store-b | store/{edges,open,facets,mod,judgments,adjudications,lock,codec}, store/nodes/ | 165 |
| P8 | workitem | workitem/, workitem/contracts/, workitem/queues/ | 253 |
| P9 | commands-a | capture_cmd, status_cmd, orient_cmd, edge, discover_cmd | 183 |
| P10 | commands-b | codefile_cmd, apply_cmd, context_cmd, proposal_cmd, pattern_cmd, graph_cmd, audit_cmd, judgment_cmd | 147 |
| P11 | commands-c | the 11 small command files, diagnostics_cmd/, domain_cmd/, intent/ | 181 |
| P12 | commands-d | commands/journey/, commands/proof_cmd/ | 169 |
| P13 | journey | journey/ | 178 |
| P14 | journey-runtime | journey_runtime/ | 155 |
| P15 | release | release/ | 185 |
| P16 | signal | signal/, signal/debt/, completeness/ | 185 |
| P17 | extract | extract/, proofstrength/, cli/, cli/subcommands/ | 221 |

Then:

| # | Name | What |
|---|---|---|
| PX | cross-cut | Read findings.md whole; cluster by mechanism; kill each duplication once. |
| PF1..n | fix waves | Apply clustered fixes; `cargo check` per wave. |
| PT | tests sweep | `tests/` (77 files, 2.0 MB) — declared secondary, run after src lands. |
| PG | final gate | `cargo test` + `scripts/local-ci.sh` green; report audit. |

## Status log (append only)

- 2026-08-23: plan written; baseline `cargo test` started.
- 2026-08-23: baseline green at 1f036b2 — passed=1301 failed=0. P1 read (16 files, 14 findings); P2 started (registry, prescan; 4 findings).
- 2026-08-23: PIVOT. Solo pass over 22/198 files produced 22 findings, but every strong one came from
  grepping the whole tree, not from bulk-reading a slice — the finding is the SPREAD, and a file-slice
  fan-out hides exactly that. Replaced the 17 file-slice read phases with 8 mechanism sweeps
  (text-helpers, enum-vocab, defaults-limits, key-literals, repeated-io, copy-paste-twins, altitude,
  dead-derivable), each sweeping all 198 files, then 2 adversarial checkers and 1 coverage critic.
  Run wf_6ccbfd84-132. The 22 solo findings are the floor the sweep must beat, not a duplicate to re-find.
- 2026-08-23: Sweep wave 1 landed. 94 raw -> 20 dropped by adversarial verify -> 74 confirmed
  (reuse 28, simplification 19, altitude 18, efficiency 9; 11 flagged behavior-changing). Ledger now
  96 findings. Coverage critic reports 71/198 files never named, efficiency lens thin, and four
  unassigned mechanisms (function size/nesting, tests/, error-message vocabulary, JSON output envelopes).
  Wave 2 must close those before any fix wave starts.
- 2026-08-23: Wave 2 landed and merged. 69 raw -> 58 confirmed (0 unjudged; the uid echo fixed wave 1's
  matching bug). Ledger 154 findings / 1,030 sites / 11 clusters. Unswept src files 47 -> 22.
  Wave 2's own critic returned ready_to_fix=false, but its stated blocker was "wave 2 never landed in the
  ledger" — it ran inside the same workflow, before the merge. Re-judged after merging: the clusters it
  named as half-mapped (D-json-envelope 2->9 findings, E-efficiency 9->20) are now backed by 198-file sweeps.
  Independently spot-checked the C-limits-registry claim: 17 entries registered vs ~34 real numeric limit
  constants crate-wide, so the sweep's "14 missing" is real and slightly understated. Proceeding to fix.
- 2026-08-23: FIX WAVE 1 applied (7 files, zero behavior change): artifact::fingerprint delegates to
  fingerprint_bytes (FNV constants were written twice); text::bounded_head_tail added and adopted by
  release/section_06.rs + journey_runtime/process.rs (byte-identical 19-line twins); store/schema.rs
  parse_crate_version made pub(crate) and commands.rs's byte-identical parse_version deleted;
  model::short adopted at capture_cmd.rs:612 and workitem/mod.rs:491. Verified: cargo clippy --all-targets
  0 warnings; cargo test 1304 passed 0 failed (baseline 1301 + 3 new text:: tests). No test edited.
- 2026-08-23: FIX WAVE 2 applied (cluster C-limits-registry). loom limits went 17 -> 29 entries; every
  added entry references the enforcing module's constant, so the file's own claim ("cannot drift from
  enforcement") is now true for 12 more limits. 3 co_change internals skipped with a recorded reason.
  Verified: clippy 0 warnings; cargo test 1304 passed 0 failed (unchanged from wave 1 — no regression).
- 2026-08-23: FIX WAVE 3 (cluster A, locator key). model::LOCATOR_FACET + Store::edge_locator; 24 inlined
  reads collapsed across 14 files; 8 remaining literals routed through the const. Two findings corrections
  recorded: the claimed 62 sites are really 30 (32 were JSON keys/enum strings/prose), and the proposed
  const home would have inverted store->locator ring direction. clippy 0, tests 1304/0.
- 2026-08-23: FIX WAVE 4 (cluster E, partial). Replaced 7 full-table loads used only for .len() with the
  store's existing COUNT queries. Snapshot-reuse refactor deliberately NOT started: the finding's premise
  about signal::doctor is wrong (doctor touches `store` 7 times beyond snapshot()), so it needs a
  (&Store, &Snapshot) thread through maturity.rs + audit.rs — its own pass, not a tail-end change.
- 2026-08-23: cluster E partial VERIFIED — clippy 0, cargo test 1304 passed 0 failed.
- 2026-08-23: cluster E partial VERIFIED — clippy 0, cargo test 1304 passed 0 failed.
- 2026-08-23: FIX WAVE 5 (cluster E complete). Snapshot reuse: `loom status` built the whole-graph snapshot
  5x (ladder->smells, audit::backlog->doctor+smells, doctor, layer_detector_state); now builds 1 and threads
  it through new `_with(&Store, &Snapshot)` variants. Existing public fns kept as one-line wrappers, so no
  standalone caller broke. Finding correction: it claimed these fns work off `snap` alone — doctor touches
  `store` 7 more times, so the variants take a (&Store, &Snapshot) pair.
  Verified: cargo check 0, clippy 0, cargo test 1304 passed 0 failed.
- 2026-08-23: FIX WAVE 6 (cluster I, partial). src/testutil.rs added (cfg(test) only) with one TmpRoot;
  6 of 8 duplicate harnesses migrated, 5 dead counters + 9 dead imports removed. Finding correction: only
  6 of the 8 were genuine twins — scan.rs's returns Result and seeds fixture files, prescan.rs's exposes `path`
  as a field; both reverted rather than forced. Verified: check 0, clippy 0, tests 1304/0.
- 2026-08-23: FIX WAVE 7 (cluster B mostly, G + A partial). str_enum! exported crate-wide; OwnerRole and
  TruthAxis converted onto it; identity::Agent::parse and policy::gateable_roles now derive from
  OwnerRole::ALL; grammar's two restated lifecycle consts replaced by IntentLifecycle::all_names/active_names
  in model.rs; dead ratification::ASSERTED_STATES deleted; thresholds' 4 gate-name spellings collapsed to one
  Gate enum (GATES kept as public API, now built from a const fn so tests are untouched); `fn layer` moved
  from the dispatcher into domain_cmd/layers.rs; store::LAYER_ORDER_META_KEY added and routed through 6 sites.
  Verified after each: cargo check 0, clippy 0, cargo test 1304 passed 0 failed.
- 2026-08-23: FIX WAVE 8 (cluster H, in progress). (a) FNV-1a schedule: the offset basis + prime existed at
  5 sites in 3 modules (artifact.rs, release/section_08.rs TreeHasher AND its own fingerprint_bytes,
  store/codec.rs) all feeding hashes compared against each other — now one streaming `artifact::Fnv1a`;
  constant sites 5 -> 1. Streaming retained because section_08 documents that buffering ~/.rustup got the
  release rehearsal SIGKILLed. (b) JSON canonicalizer: 5 copies under 4 names in 5 modules, 3 byte-identical
  — now `src/canonical.rs` (+3 tests). (c) `.loom` / `loom.graph.json` routed through crate::LOOM_DIR /
  crate::GRAPH_EXPORT at 11 sites. Verified: clippy 0, tests 1307 passed 0 failed (1304 + 3 new).
- 2026-08-23: FIX WAVE 8 continued. Dead completeness re-export deleted; signal.rs unused-import shim removed
  and its two dependents re-pointed. Recorded one SKIP (ratify twins — fix is either behavior change or ~50
  test edits) and one DEFER (PromptContract Default — blocks not uniform, 28 hand edits, formatting-only).
  Verified: clippy 0, tests 1307 passed 0 failed.
