# Changelog

Notable changes to the `loom` crate. Versioning follows [semver](https://semver.org):
**patch** = bug fixes, **minor** = backward-compatible features, **major** = breaking
changes. (`SCHEMA_VERSION` in `src/lib.rs` is separate — it versions the on-disk graph
schema, not the crate.)

Bump with `scripts/release.sh <patch|minor|major> "<summary>"` — never hand-edit the
version.

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
