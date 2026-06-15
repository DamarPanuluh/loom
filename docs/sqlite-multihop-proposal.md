# Proposal: SQLite storage, a multi-hop audit layer, and a cost-lifecycle discipline

**Status:** Accepted direction; SQLite runtime cutover is implemented, multi-hop
audit work remains proposed.
**Date:** 2026-06-15
**Scope:** the storage substrate (legacy backend → SQLite), the graph
capabilities that substrate unlocks (multi-hop analysis), and the operation
cost model that keeps both safe for an LLM driving loom autonomously.

---

## Summary

Three decisions, one narrative:

1. **Storage.** Keep the SQLite port the way `src/db/sqlite.rs` implements it:
   **typed relational tables, computation in Rust** — not a generic
   `Nodes/Edges` blob and not "push every traversal into SQL." This is the right
   call for loom's access patterns and it retires real accidental complexity
   (the `loom serve` daemon's reason to exist; the legacy edge-id bug class).

2. **Capability.** loom's one-hop floor is deliberate and correct for the
   *vertical "done" spine*. But it makes a whole class of **audit-layer truths
   unexpressible** — dependency cycles, transitive layering violations, intent
   islands, bridge centrality. A bounded **multi-hop audit layer** adds these as
   new `smells`/`graph_state` signals. SQLite recursive CTEs are the natural,
   cheap enabler — the one place CTEs actually earn their keep.

3. **Lifecycle.** Autonomous driving lives or dies on **cost-per-cadence**.
   `loom next`/`loom status` run every turn and must stay light; the multi-hop
   class must never ride the hot path. The discipline: classify every operation
   by tier, cache topology-keyed heavy metrics behind a `structure_version`
   stamp, and keep status-dependent heavy passes as **lazy closures invoked only
   at the phase gate** (the pattern `graph_state` already uses).

The connective claim: the port doesn't just preserve parity — it **opens the
door** to (2), and (3) is the discipline that makes (2) safe to walk through.

---

## Context

loom is driven by an LLM on a long horizon: `loom next` → read/edit code →
`loom sync` → `loom next`, repeated until the graph is vertically complete and
green. Every output is the prompt for the next decision; latency and tokens are
paid on *every* turn. The substrate and the cost model are therefore not
implementation details — they are the load-bearing constraints on whether
autonomous completion is even possible.

The runtime port is implemented: `src/db/sqlite.rs` owns the active graph store,
the CLI opens `.loom/graph.sqlite` directly, and `tests/sqlite_regression.rs`
exercises imported graphs plus representative SQLite mutations. The legacy
query/fallback layer has been removed from source.

---

## Part I — Storage: the SQLite port

### Decision

Port to **typed relational tables with computation staying in the Rust
`*_from_parts` layer.** SQLite is the indexed store; the graph logic stays in
Rust over loaded snapshots.

### Why typed tables, not a generic graph-in-SQL

`sqlite.rs` already models `intent`, `codefile`, `relates_to`, `hierarchy`, …
as distinct tables with real columns, foreign keys, and `UNIQUE` endpoint
constraints. Keep it. A generic `Nodes(id, type, props_json)` + `Edges(...)`
EAV blob would throw away exactly what SQLite is good at — typed columns,
indexes, the query planner — to reimplement a graph store, badly, on top of a
relational one. Typed tables also give the regression tests something concrete
to assert against.

### Why computation stays in Rust

`query_snapshot()` loads each table once and hands `Vec`s to the shared
`*_from_parts` functions (`compute_smells_from_parts`,
`graph_state_from_snapshot_parts`, `check_graph_from_parts`,
`rank_intents_from_parts`). **That shared Rust engine is loom's crown jewel** —
it is what keeps storage and analysis separated. Pushing logic *down* into SQL
would double the surface and forfeit that leverage. Rule: **SQL stores and
filters; Rust derives.**

### What the port retires (the real prize)

The case for SQLite was never query expressiveness — loom's reads are ~95%
one-hop (see Part II). It is **operational**:

- **WAL mode → concurrent readers + one writer for free**, which obviated the
  `loom serve` daemon, whose sole reason to exist was dodging the old backend's
  cross-process lock. The daemon implementation has been deleted; `loom serve`
  is now only a retirement stub for old scripts.
- **The legacy edge-id-shadowing bug class vanishes.** `WHERE r.id = X` matching
  an internal int, derived-not-stored edge keys, the MERGE workarounds — gone.
  Edges become rows with a real `UNIQUE(from_id, to_id)` and real indexes.
- **The perf shape improves.** Indexed `from_id`/`to_id` pushes filtering into
  storage directly instead of scanning broad graph query results in the client.

### What stays the same

The snapshot read path, the `*_from_parts` engine, the export/import travel
format, and `loom.graph.json` as the committed deterministic artifact. The port
is a substrate swap, not a model change.

### Non-goal

Do **not** rebuild the existing one-hop work on recursive CTEs. There is no
payoff; it only adds SQL surface against a graph that is almost entirely one hop
wide. CTEs are reserved for Part II.

---

## Part II — The multi-hop audit layer

### The floor is deliberate

A survey of the query layer found **~95% of loom's reads are one-hop**: priority
scoring is `degree(a) + degree(b)`; the sync ripple is explicitly one hop
(IMPLEMENTS → RELATES_TO neighbors); clustering, smells, and stats are
immediate-neighbor or pairwise. The only existing multi-hop walks are bounded to
the shallow HIERARCHY tree (roll-ups, cycle-check on insert), done cheaply in
Rust.

This floor is **correct for the vertical "done" spine** — "a leaf is realized
iff it has an IMPLEMENTS edge" is a one-hop fact and must stay one. The floor's
cost is on the **horizontal / audit axis**: it makes a class of structural
truths *unexpressible*. Multi-hop adds them.

### Capability catalog

Ordered by recommended priority.

**1. Transitive smells loom cannot express today (highest value).**
Architectural rot hides at depth, not in direct edges.
- *Dependency cycles among intents (SCC detection).* HIERARCHY is cycle-checked
  because it is a tree; RELATES_TO is a general graph with **no** cycle
  detection. A→B→C→A is a real, serious smell loom cannot currently name.
- *Transitive layering violations.* `layering_violation` checks direct imports
  against `layer` order. The dangerous ones are clean at every hop yet form an
  illegal presentation→application→presentation loop across three.
- *Intent islands / unreachable-from-purpose.* Completeness checks "every leaf
  has a parent" (local). It cannot ask "is this subgraph reachable from a
  `system`-level root?" Multi-hop reachability finds **islands of intent** —
  clusters connected to nothing the product claims to do (dead code at the
  intent level).

These are pure `phase=audit` signals — exactly what gates `phase=complete`.

**2. Real centrality, not just degree.**
`degree(a) + degree(b)` is local and misses the most important nodes.
- *Betweenness / bridge centrality.* A low-degree intent that everything routes
  through is a critical chokepoint — currently invisible to `loom next`, which
  ranks it last. That is precisely the work you most want surfaced first.
- *PageRank-style importance.* "Matters because important intents depend on it."
- Note: these are iterative algorithms, **not single CTEs** — compute them in
  Rust over the snapshot (like `degrees` already is). Multi-hop *permission*,
  not SQL recursion, is what they need.

**3. Graded (decaying) ripple.**
One-hop ripple was chosen because full propagation "would reset everything." The
better middle ground, unreachable at one hop: directly-grounded intents flip
hard `needs_reverification` (as today), but 2–3 hops out get a *weak, decaying*
"maybe glance" signal (a small priority bump, not a status flip). Catches genuine
transitive impact **without flooding the queue**.

**4. Path / explanation queries.**
A new *kind* of question: "how are A and C related?" — surface the chain coupling
a presentation intent to a storage intent. The raw material for (1)'s transitive
layering smell, and the way to explain *why* distant parts are entangled.

**5. Cross-plane roll-ups.**
`normative_coverage` already inherits rule verdicts up the ancestor chain —
proof that the pattern is useful. Generalize: proof inheritance (a parent is
proven iff all descendants are), persona journey reachability (does a persona's
SERVES'd intents *transitively* reach a passing JOURNEYS validation?), hypothesis
adoption blast-radius (the transitive set TARGETS would ripple).

### Recommended scope

- **Build:** (1) transitive audit smells, (3) graded ripple, (2) betweenness for
  prioritization.
- **Defer:** (4) path queries (nice, not load-bearing), (5) cross-plane
  roll-ups (incremental on top of the existing ancestor-walk pattern).
- **Never:** full transitive-reset ripple (the flood); and do not move the
  vertical spine off one-hop.

### Where CTEs earn their keep

The reachability/cycle/island family is textbook `WITH RECURSIVE` over indexed
`from_id`/`to_id` columns — cheap in SQLite, painful in the legacy store. Centrality stays
in Rust. So the split is: **recursive CTEs for structural reachability; Rust for
iterative metrics.**

---

## Part III — Operation cost & lifecycle

### The axis that matters is cost-per-cadence

An operation is "heavy" *relative to how often the loop calls it*. `loom next`
and `loom status` run **every turn**. If a multi-hop op silently rides the hot
path, three failures compound across hundreds of turns:

1. **Latency stall** — the LLM blocks on every call.
2. **Token burn** — heavy *outputs* are expensive to read, independent of compute.
3. **Thrash** — recomputing a transitive closure after a one-line verdict that
   did not change topology.

**Lifecycle-correct = match each operation's cadence to how often its underlying
truth actually changes.**

### The three cost drivers

An operation is heavy if it is bad on *any* axis:

| Axis | Light | Heavy |
|---|---|---|
| **Locality** | one node's neighbors / single snapshot scan | whole-graph walk or path enumeration |
| **Key volatility** | keyed on *local status* (flips constantly, cheap to redo) | keyed on *structure* (rarely changes in the loop) |
| **Output size** | bounded projection | grows with graph size |

The middle axis is the lever. **Most loop mutations are status flips, not
structural changes.** Recording a verdict (`edge ground`, `rule verdict`)
changes `inspection_status` — it does **not** add/remove edges. Centrality, SCC,
islands, transitive layering depend on **structure only**, so they survive the
entire verdict grind untouched. That is the whole game.

### The tiers

**Tier 1 — Hot path, recompute freely (every turn).**
`next` selection, `status`/graph_state, `show`/`list`, `cluster`, single-edge
mutations. Local or one snapshot scan with O(E) Rust aggregation. Always fresh,
never cached.

**Tier 2 — Periodic, triggered by code change.**
`sync` (cost ∝ files changed), `batch`. Fire after edits, not per-turn. Fine
as-is.

**Tier 3 — Heavy / global, must NOT ride the hot path.**
`smells`, `doctor`, `report`, `coverage`, and the **entire multi-hop class**.
Two disciplines, applied per op (see below).

### Mechanism A — the `structure_version` cache (for topology-keyed metrics)

For pure-structure metrics (centrality, SCC/cycles, islands, transitive
layering): persist them, stamped with a structure version.

```
meta.structure_version  INTEGER

derived_metrics(
    metric            TEXT PRIMARY KEY,   -- 'betweenness' | 'scc' | 'islands' | …
    structure_version INTEGER,            -- the version it was computed at
    computed_at       TEXT,
    payload           TEXT                -- JSON result
)
```

**Bump `structure_version` on:** edge add/remove, node add/remove, and the node
attributes a structural metric reads (`layer`, `lifecycle`).
**Never bump on:** edge `inspection_status`/`criterion`/`evidence` writes, note
appends — i.e. exactly the high-frequency loop mutations.

**Read protocol:** a heavy command compares the cache's `structure_version` to
`meta.structure_version`. Match → serve cached instantly. Miss → recompute,
re-stamp. During a verdict-heavy grind, structure is stable → the cache is a
permanent hit. SQLite makes this a **persisted** table; each command is a fresh
process, so the cache belongs in the DB rather than process memory.

### Mechanism B — lazy closures at the phase gate (for status-keyed heavy ops)

`smells`/`doctor` depend on *status* (failing edges), so they cannot be
structure-cached. Defer them instead. loom **already has the right primitive** —
`graph_state` passes the heavy smell pass as a lazy thunk:

```rust
graph_state_from_snapshot_parts(
    snapshot, context,
    |snapshot| compute_smells_from_parts(...).map(|r| r.open.len()),  // closure
    || self.count_hypotheses(...),                                     // closure
)
```

Because the compass routes vertical gaps *first* (`ground`/`incomplete` ahead of
`audit`), during the whole build grind the smell closure is **never invoked** —
the cheap checks resolve the phase before the heavy one is needed.

**Generalize this:** every heavy multi-hop signal enters the hot path *only* as a
lazy closure behind cheap decisions, never as an eager column. The hot path
computes light facts and *reaches for* heavy facts only when a decision hinges on
them.

### Mechanism C — output discipline (surface, then dig)

Even a cheap-to-*compute* heavy result can be expensive in *tokens*. Heavy
results summarize on the hot path and enumerate only on explicit dig — the
existing contract, applied doubly:

```
12 dependency cycles — `loom smells --kind cycle`
```

### Why the cost lands when churn is lowest

The multi-hop ops are audit-layer: they matter at `phase=audit`, reached only
when the **vertical spine is closed and the graph has stopped churning.** So the
expensive transitive analysis runs *exactly when structure is most stable* — the
cache hit rate is highest and recompute is paid fewest times, precisely when the
answer matters. The loop spends 99% of turns in the cheap grind and pays for the
heavy layer only at the finish line.

---

## How it fits: the autonomous loop

```
every turn:        loom next / loom status     → Tier 1, always light, always fresh
after code edits:  loom sync                    → Tier 2, bumps structure_version
                                                   (edges touched), one-hop ripple
record a verdict:  edge ground / rule verdict   → Tier 1 write, does NOT bump
                                                   structure_version → heavy caches survive
approaching done:  vertical spine closes        → compass consults the audit gate
                                                   → lazy smell/multi-hop closures fire
                                                   → structure stable → caches hit
                                                   → findings summarized, dug on demand
```

The driver never triggers an O(V·E) walk mid-grind; heavy analysis is amortized
to ~free by the structure stamp; expensive truths surface when the graph is calm
enough to compute them cheaply.

---

## Phased rollout

1. **Finish the SQLite runtime port.** All primary command groups use
   `SqliteGraphStore`; `tests/sqlite_regression.rs` covers imported reads and
   representative mutations; the `*_from_parts` engine remains unchanged.
2. **Flip the default + WAL.** SQLite is the default backend; keep WAL enabled
   and verify the concurrency behavior as usage grows.
3. **Retire the daemon.** Done: the implementation is deleted, `loom serve` is
   a clear retirement stub, and docs point at direct SQLite operation.
4. **Add `structure_version` + `derived_metrics`.** Wire the bump points into the
   edge-write / node-structural-write paths; verify status flips do not bump.
5. **Ship the first multi-hop smell** (recommend SCC dependency cycles) end to
   end: recursive-CTE reachability → `derived_metrics` cache → `loom smells
   --kind cycle` → `graph_state` audit-gate integration via a lazy closure.
6. **Add islands + transitive layering**, then **betweenness** (Rust, snapshot).
7. **Graded ripple** as a refinement to `loom sync`.

Each phase is independently shippable and behind the parity gate.

---

## Risks & mitigations

- **Heavy op leaks onto the hot path.** *Mitigation:* the lazy-closure rule
  (Mechanism B); a test that asserts `status`/`next` never invoke a multi-hop
  function unless the phase gate is reached.
- **Cache staleness / wrong invalidation.** *Mitigation:* `structure_version` is
  one counter with an explicit, audited bump set; a `loom doctor` check can
  recompute one metric live and diff against the cache.
- **Ripple flood from graded propagation.** *Mitigation:* decay is a *priority
  bump*, never a status flip beyond hop 1; bounded depth.
- **Runtime/source split lingers too long.** *Mitigation:*
  `tests/sqlite_regression.rs` is the active runtime gate; the old query/fallback
  layer has been removed after equivalent SQLite-native coverage landed.
- **WAL or direct-open behavior regresses under heavy parallel automation.**
  *Mitigation:* add a SQLite concurrency regression before optimizing parallel
  multi-agent runs; `loom serve` should remain retired unless a new measured
  bottleneck proves otherwise.

---

## Non-goals

- Rebuilding existing one-hop queries on recursive CTEs.
- A generic `Nodes/Edges` EAV schema.
- Moving the vertical "done" spine off one-hop semantics.
- Full transitive-reset ripple on file change.
- Pushing the `*_from_parts` computation engine down into SQL.

---

## Open questions

1. **One `structure_version` or two?** A single counter that bumps on
   layer/lifecycle changes is simplest but slightly over-invalidates layering
   metrics. Start with one; split only if profiling shows it matters.
2. **Centrality cadence.** Recompute betweenness on every `structure_version`
   bump, or only when the audit gate is reached? Likely the latter — it is an
   audit-layer signal, not a hot-path one.
3. **Should graded ripple confidence decay be configurable,** or a fixed
   per-hop factor? Default fixed; expose only if a repo needs it.
