# Contributing to loom

This document holds the **contributor-only** conventions and rationale for
working on loom itself — everything an agent *driving* loom does not need, and
which loom's CLI cannot self-teach. For using loom, see [`CLAUDE.md`](../CLAUDE.md)
and run `loom guide` / `loom schema` / `loom help`.

These sections were extracted verbatim from the previous 666-line `CLAUDE.md`
when that file was trimmed to a dogfood stub ("use loom"). Nothing here was
rewritten — only relocated. The live map of this codebase is the graph
(`loom.graph.json`) plus `loom find` / `loom coverage`; the structure snapshot
below can drift as files move, so trust the graph over the snapshot.

---

## Codebase structure

```
src/
├── main.rs               entry point, command dispatch
├── cli.rs                clap CLI definitions (derive)
├── types.rs              all structs: Intent, CodeFile, QualityRule, Validation, Note,
│                         InspectionStatus, NoteKind, EdgeType, WorkItem, SyncReport, etc.
├── output.rs             dual-mode rendering (human / --json) + graph pulse
├── repo.rs               filesystem introspection: gitignore-aware walk + stack detection
├── agent.rs              acting-agent resolution for provenance (--by / $LOOM_AGENT / "llm")
├── saga/                 the consumer plane's engine (pure Rust: reqwest/rustls)
│   ├── mod.rs            module wiring + design rationale
│   ├── spec.rs           YAML spec format, load-time validation, {{ }} interpolation
│   └── runner.rs         sequential executor: captures, asserts, halt-on-failure outcomes
├── gate.rs               the enforcement layer: role lanes (declared role held to its
│                         lane; solo mode passes) + evidence gates (substantive
│                         criterion/evidence/notes, confidence ∈ [0,1])
├── db/
│   ├── mod.rs            graph root resolution + read-handle abstraction
│   ├── sqlite.rs         SQLite schema, import/export bridge, runtime reads/writes
│   ├── schema.rs         THE schema vocabulary: labels/edges/props + per-field owner role,
│   │                     version, required-property tables
│   └── queries/          pure snapshot analysis: search, queues, integrity, stats, smells
│       ├── mod.rs          module wiring + flat re-export
│       ├── vocab.rs        VocabTerm registry + intent tags (bounded vocabulary:
│       │                   normalize/merge/rarity-weighted collision + look-alike)
│       ├── meta.rs         GraphMeta and transition defaults
│       ├── completeness.rs vertical-completeness spine (tree + realization join)
│       ├── relates_to.rs   unresolved RELATES_TO analysis
│       ├── scoring.rs      priority scoring + per-mode candidate selection (loom next)
│       ├── snapshot.rs     QuerySnapshot — one graph load feeding scoring/stats/compass
│       │                   coherently (the production read path; no per-query reloads)
│       ├── find.rs         BM25 keyword search over intents (loom find)
│       ├── smells.rs       derived problem signals (split-brain, scatter, tangle,
│       │                   layering, vocab drift, …) with per-finding remedy; plus
│       │                   the non-gating ADVISORIES — cochange_coupling (git),
│       │                   nonlocal_proof (a `proven` leaf whose only test lives in
│       │                   another module — the proof-locality check), and code_clone
│       │                   (cross-file exact-text duplication via per-symbol
│       │                   body_hash) (loom smells)
│       ├── stats.rs        counts / centrality / graph_state pulse / completeness gaps
│       └── integrity.rs    graph integrity checks (loom doctor)
└── commands/
    ├── init.rs           loom init
    ├── status.rs         loom status (+ compass + pulse)
    ├── intent.rs         loom intent *
    ├── edge.rs           loom edge *
    ├── next.rs           loom next
    ├── cluster.rs        loom cluster
    ├── sync.rs           loom sync (flag engine; stamps last_synced)
    ├── validate.rs       loom validate
    ├── codefile.rs       loom codefile * (glob-aware add)
    ├── validation.rs     loom validation *
    ├── hypothesis.rs     loom hypothesis * (propose/prove/adopt/reject)
    ├── vocab.rs          loom vocab * (registry + the in-band tag nudge `validate_tags`)
    ├── note.rs           loom note *
    ├── rule.rs           loom rule *
    ├── saga.rs           loom saga * (consumer-plane proofs: declare, run, stamp the path)
    ├── batch.rs          loom batch (bulk JSONL verdicts — the post-sync drain)
    ├── persona.rs        loom persona * (audience segments + SERVES/JOURNEYS edges)
    ├── layer.rs          loom layer order|list|clear (architecture dependency direction)
    ├── domain.rs         loom domain (DEPRECATED alias of `loom layer`)
    ├── report.rs         loom report (+ completeness gaps)
    ├── doctor.rs         loom doctor
    ├── guide.rs          loom guide
    ├── schema.rs         loom schema
    ├── find.rs           loom find (ask the map)
    ├── inbox.rs          loom inbox (raw language intake cards)
    ├── door.rs           loom door (capture utterance → InboxItem + matches + landing menu)
    ├── session.rs        loom session (turn zero: ask-the-user playbook + offer menu)
    ├── hotspots.rs       loom hotspots
    ├── coverage.rs       loom coverage
    ├── smells.rs         loom smells (derived suspicions; OPEN findings gate green)
    ├── detect.rs         loom detect
    ├── ignore.rs         loom ignore *
    ├── delegate.rs       loom delegate (federation: hand a subtree to a child graph)
    ├── migrate.rs        loom migrate (SQLite schema verification)
    ├── export.rs         loom export [--check] (commit the graph as deterministic JSON)
    └── import.rs         loom import (rebuild a graph from an export)
```


## The LLM-driver output contract

loom's only user is an LLM agent on a long horizon: every output is the prompt
for the agent's next decision, and after context compaction loom's output is
the only memory that survives. Four invariants, enforced by shared helpers in
`src/output.rs` (`print_anchor`, `with_anchor`, `pulse_json`, `more_marker`,
`apply_limit`, `SECTION_CAP = 10`, `LIST_LIMIT = 50`) — every new command MUST
follow them:

1. **Anchor after mutation.** Phase-moving verdicts (edge ground/issue/
   independent/fix, validation mark, rule verdict, sync, import, saga run,
   validate, intent confirm/retire) end with `→ Next: <runnable command>` +
   the two-line pulse in human mode, and `next_step` + `graph_state` fields in
   json. Construction steps called in rapid mapping loops (implement,
   hierarchy, tag, codefile add, vocab add, …) get a LIGHT anchor — the
   `next_step` line/field without the pulse, so loops don't drown in repeated
   state. An agent never needs a separate `loom status` to know where it
   stands.
2. **Human/json parity.** Whatever guidance human mode prints, json carries
   (orchestrated agents run `--json`; a hint that lives only in `println!` is
   invisible to them). List commands always emit
   `{"<noun>s": [...], "total": N, "truncated": bool}` — never a bare array,
   never a data-dependent shape.
3. **Bounded output.** Anything that scales with graph size is capped:
   inventory lists honor `--limit` (default 50, `0` = all), sub-sections
   inside work items and show views cap at `SECTION_CAP` keeping the NEWEST
   notes (addressed-to-role notes always survive the cap), and every
   truncation prints `… +N more — <runnable fetch command>` (an affordance,
   never an apology). Errors teach: a failure names the corrective command or
   inlines the valid choices — never a bare "not found". SYNTAX failures are
   covered systematically (cli.rs `parse_or_teach` + commands/mod.rs
   `teach_unknown`):
   - every clap parse error (missing flag, bad value, stray positional)
     reprints the failing command's EXAMPLE after_help under the error — an
     EXAMPLE block IS the command's error message;
   - a ratchet test (`every_flag_requiring_command_ships_an_example`) fails
     the build when a command with required flags lacks an EXAMPLE, so new
     commands can't ship friction-shaped;
   - unknown top-level tokens land in an `external_subcommand` catch-all:
     noun-less verbs and synonyms (`update`/`rename`/`retire`/`add`/`ground`/
     `prove`/…) answer with the real invocation and the agent's own argument
     spliced in; typos get a real edit-distance suggestion (clap's stock tip
     once mapped `update` → 'guide');
   - `intent update` additionally catches positional wording and a missing
     --reason with the full shape (evolved / --reword / rename).
4. **Surface, then dig.** Payloads embed PROJECTIONS — the fields the next
   decision needs — never full records: work items carry the `*Surface` types
   from `src/types.rs` (intent without timestamps, grounding as
   path/locator/status, notes deduplicated with a `×times` count), the json
   `graph_state` field is `pulse_json` (the two human pulse lines,
   structured), and `--json` prints COMPACT (pretty-printing is token spend).
   Every elision names its runnable dig command: `loom intent show`,
   `loom edge show`, `loom note list`, `loom status --json` (the one place
   the FULL GraphState travels). Same reason `loom next --take` template
   lines omit `criterion`: `loom batch` reuses the recorded one, so neither
   loom nor the agent re-transmits text the graph already holds.


## Build

```bash
cd /Users/laptopdp/Developer/damarpanuluh/loom
cargo build
```

The runtime graph store is SQLite via `rusqlite`. The default tree-sitter
grammar crates compile C sources for richer imports, so the default build still
needs a working C compiler. Release builds currently use thin LTO for faster
iteration. Use `cargo build --no-default-features` for the dependency-light
heuristic import path.

Run `cargo test` for the query-layer regression suite (in `db/queries/mod.rs`),
which covers the relationship reliability rule below. Also run
`cargo test --no-default-features` before release changes that touch sync or
import extraction so the fallback path stays green.

**Zero-warning, rustfmt-clean is the bar.** Formatting is a mechanical invariant
like the build-failing ratchet tests — the repo uses stock stable rustfmt
(`rustfmt.toml`, edition pinned to Cargo.toml). Before every commit:

```bash
cargo fmt                          # or `cargo fmt --check` to verify
cargo clippy --all-targets         # must be warning-free
cargo build                        # must be warning-free (default + --no-default-features)
cargo test                         # green
```

Test-only items (constants/imports used solely by `#[cfg(test)]` code) carry
`#[cfg(test)]` so the non-test profile stays warning-free — don't `#[allow]`
them. (No CI is wired yet; these are the pre-commit gate by hand. A
`cargo fmt --check` + `clippy -D warnings` CI step is the natural enforcement
when CI lands.)


## Key design decisions (why, not just what)

**Why SQLite now:** Loom's production operations are bounded graph reads/writes
with explicit queues, lifecycle state, and exportable JSON identity. SQLite gives
us durable embedded storage, transactions, normal tooling, easy migrations, and
fewer runtime locking surprises. Recursive CTEs and targeted adjacency indexes
cover the graph traversals loom actually executes today; more advanced graph
analysis can still be layered later as derived tables, virtual tables, or a
specialized analysis engine without replacing the core SQLite store.

**Why `independent` is a state not an edge type:** Independence is a verified claim about a relationship, not a different kind of relationship. Encoding it as state keeps the schema clean and queries uniform.

**Why Validation is a node not a property:** Validations are reusable, runnable, and have their own lifecycle (last_run, last_result). Nodes are the right abstraction for entities with identity and state.

**Why separate state from meta:** State drives the workflow engine. Meta provides evidence. Keeping them conceptually distinct prevents the agent from confusing "what do we know" with "how do we know it."

**Why loom sync propagates one hop:** Full graph propagation from a file change would be too aggressive — everything would reset. One hop (IMPLEMENTS → RELATES_TO neighbors) is the right blast radius for a file-level change. System-level changes require explicit re-initialization.

**Why edge identity is DERIVED, not stored (schema v4):** edges are unique per endpoint pair, so an edge's id is computed at read time — `schema::edge_key(type, from, to)` → `rt:<a>:<b>` / `imp:` / `gov:` / `val:` / `tgt:` / `hy:` — never written to the store. The uuid it replaced was redundant identity that regenerated on every import so note targets broke in transit and forced scan-to-find-by-id reads. Derived keys are stable across machines and exports by construction. SQLite stores endpoint columns directly and treats the derived key as API identity.
