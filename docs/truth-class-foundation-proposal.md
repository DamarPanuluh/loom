# Proposal: the truth-class foundation — type every edge by *how it becomes true*

**Status:** Proposal (for discussion). Builds directly on
[`sqlite-multihop-proposal.md`](sqlite-multihop-proposal.md) (`SQL stores, Rust
derives`; `structure_version` cache; the bounded multi-hop audit layer) and
[`maturity-ladder-proposal.md`](maturity-ladder-proposal.md) (`derived, never
stored — pure functions over evidence`). It names a distinction those two
proposals already lean on but never made first-class.
**Date:** 2026-06-29
**Scope:** classify every edge by ONE orthogonal property — `truth_class ∈
{derived, asserted, statistical}` — and let it govern status authorship,
invalidation, and whether the edge is ever an *obligation*. The classification is
a pure function over existing fields (derived in code first; persisted only if
later earned — see Rollout), so the reform targets loom's **accounting and routing
surface**, not its storage substrate (already sparse). One genuinely new stored
mechanism: asserted edges carry a `depends_on` set so multi-hop LLM conclusions
can be memoized and correctly invalidated.

---

## Summary

Three decisions, one narrative.

1. **Every edge has a truth-class, and the truth-class is the determinism
   boundary.** *Derived* edges (imports, calls, containment, hashes) are computed
   by extraction — reproducible bit-for-bit, never inspected, never queued.
   *Asserted* edges (GOVERNS verdicts, manual RELATES_TO, multi-hop conclusions)
   are judgment — non-deterministic at production, but a *pinned fact* once
   recorded, until invalidated. *Statistical* edges (co-change, shotgun, clone)
   are scores — a **ranked candidate feed**, never persisted as an obligation.

2. **`independent` is the default, not a stored verdict, and the N×N grid is a
   candidate generator, not a denominator.** loom does not store 83k pairs — but
   it stores **5,241** `independent` rows and reports `explored 5642/83436` as if
   completing the grid were the goal. Absence of a relationship is the default
   state of every pair; you store an edge only when a real relationship exists
   (asserted/derived) or when a human deliberately records "these *look* coupled
   but are not" (a rare manual independence claim with evidence). The
   intent↔intent coupling question is answered by a **2-hop projection** over the
   structural substrate, computed on demand — never an enumerated checklist.

3. **Breadth is programmatic and deterministic; depth is cognitive and
   memoized.** The deterministic engine computes every *local* fact (bounded
   hops). The LLM does *selective deep* traversal — walking the typed graph as a
   map, reading code at each hop — and its conclusion is written back as an
   asserted edge whose `depends_on` set records the path it relied on. Expensive
   cognition is cached into the graph; `sync` re-opens it only when a node on its
   path changes.

The connective claim: loom's *storage* is already right (sparse, typed). What
leaks is the **accounting** — statistical noise counted as required debt, the
grid counted as an obligation, `independent` materialized toward 83k. Naming the
truth-class fixes the accounting, and `depends_on` is the small addition that
makes multi-hop intelligence durable instead of recomputed.

---

## Context

loom's own graph is the first dogfood victim of the leak. Observed this session:

```
raw edge states: 8886 total · 3075 passing · 5241 independent · 0 failing · 552 stale
360°: explored 5642/83436 · measured 20256/25389
excellence red (debt 18352)
advisories: 18352 waiting — co-change 17961 · shotgun 372 · proof-locality 19
```

Read those numbers through the truth-class lens:

- **8,886 stored edges** — already sparse, NOT N×N. The substrate is fine.
- **5,241 `independent`** — over half the stored edges are verdicts that *no
  relationship exists*. This is the grid leaking into storage one inspection at a
  time; driving `explored` toward 83,436 would materialize ~83k such rows.
- **17,961 co-change** — a *statistical* signal, computed on demand
  (`cochange_suggestions`, outside `compute_smells_from`), correctly NOT stored
  as edges — yet **counted as debt** and folded into the headline `18352`.
- **18,352 "excellence debt"** — overwhelmingly statistical advisory. It is a
  *ranked feed*, not 18k tasks. Presented as guilt, it demoralizes the operator
  and buries the asserted residue that actually needs judgment.

The substrate already obeys "derived, never stored" for co-change. The reform is
to make that principle **explicit, total, and reflected in the accounting**:
statistical never gates and never counts as required debt; `independent` stops
being a stored grid-completion verdict; asserted multi-hop gets a dependency set.

---

### What loom already got right (so this is formalization, not invention)

The truth-class split is not a new theory imposed on loom — it is the name for a
distinction loom *already discovered ad-hoc* and applies inconsistently across
modules. Being honest about that is what makes this low-risk:

- **The N×N grid is already de-gated for production.** `stats.rs` states it
  outright — "the N×N grid was redundant with the smell gate ... it no longer
  gates" — and `horizontally_explored` is computed as `rt_uninspected == 0 &&
  rt_needs_rev == 0` (stored RELATES_TO edges being *current*), NOT grid
  completion. The exhaustive survey is already labelled OPTIONAL discovery.
- **Co-change / shotgun are already advisory.** They feed only the `excellence`
  certification axis; `completion_totals` keeps `required_autonomous` /
  `required_human` debt separate from `excellence_debt`. Production-ready does not
  gate on them today.
- **Physical vs behavioral is already de-facto split.** Physical concerns are
  computed smells (`oversized_file`, `complex_symbol`, `large_behavioral_symbol`);
  behavioral quality is GOVERNS verdicts on intents.

So why act? Because the distinction is **un-named, so it leaks and contradicts
itself:**

- `next/render.rs` still renders `priority_unexplored_pairs` under a *non-optional*
  `horizontal-risk` queue while `stats.rs`, `status.rs`, and `guide.rs` all call
  the same number optional — a **live inconsistency** with no single source of
  truth to resolve it.
- The headline `excellence_debt` (`status.rs`) still sums **raw statistical
  instances** (the `18,352`), presenting an advisory feed as a debt cliff.
- **5,241 stored `independent` rows** still materialize the grid one inspection at
  a time; the `explored` 360° axis still implies a completable grid.

`truth_class` is the single name that makes the proven distinction *first-class
and consistent* — it removes the drift, it does not invent a model. The end-state
below is therefore mostly **subtraction**: fewer gates, fewer stored rows, fewer
concepts to teach.

## Part I — The three truth-classes

### Decision

Classify every edge by `truth_class ∈ {derived, asserted, statistical}`. It is
orthogonal to the edge *kind* (IMPLEMENTS, GOVERNS, RELATES_TO, …) and to
`inspection_status`. Kind says *what the relationship is*; truth-class says *how
its truth is established*; status says *where its inspection stands*.

**Truth-class is a derived classification, not (initially) a stored column.** It
is a pure function over fields loom *already* has — the edge's table (kind),
`RELATES_TO.kinds`, and `inspection_status`: import-coupling/shares-file kinds →
`derived`; `manual` → `asserted`; verdict tables → `asserted`; the git-derived
suggestions → `statistical`. So the routing reform (especially the accounting
win, INV-4) ships as **code over existing data**, reversible, no migration.
Persist `truth_class` as a column only if profiling later shows the derivation is
hot enough to index (Part V); `depends_on` is the only field that genuinely *must*
be stored, and it rides with the Part III multi-hop layer. The eventual stored
form, if earned, is `truth_class TEXT NOT NULL CHECK(truth_class IN
('derived','asserted','statistical'))`.

| truth-class | examples | who writes `inspection_status` | stored? | an obligation / queued? |
|---|---|---|---|---|
| **derived** | imports, calls, containment, hash-staleness, physical file metrics | `sync` / extractor only | yes, but rebuildable from code | **never** — auto-resolved by sync |
| **asserted** | GOVERNS verdict, manual RELATES_TO, multi-hop conclusion, deliberate `independent` | role-gated verdict only | yes | only when `sync` stales it |
| **statistical** | co-change, shotgun-surgery, code-clone, proof-locality | nobody — it is a score | **no** — computed on demand | **never** — ranked feed only |

### Why the boundary is the determinism boundary

- **Derived is reproducible.** Given the same code + git state, extraction yields
  the identical edge set and statuses. The whole derived layer can be wiped and
  rebuilt by `loom sync` bit-for-bit. This is the *first golden-testable
  property* loom can have, and it is impossible today because derived and
  asserted statuses share one mutable column with no class tag.
- **Asserted is persisted-until-invalidated.** Two LLMs may verdict differently;
  the *act* of judging is non-deterministic. But the moment a verdict is written
  (criterion + evidence + confidence), the graph *state* is deterministic — a
  pinned fact until `sync` re-opens it. Determinism is therefore not bolted on;
  it is what the boundary *grants*.
- **Statistical is a function of inputs that drift** (git history grows).
  Deterministic at a pinned commit, not eternally — which is exactly why it must
  never be stored as a verdict or gate.

### Non-goal

Do **not** add `truth_class` to nodes, or invent per-class tables. It is a derived
classification (a pure function in the `*_from_parts` Rust engine — loom's crown
jewel, per the multihop proposal), persisted as at most ONE additive edge column
only if later earned (Rollout, Phase 2). SQL still only stores and filters.

---

## Part II — `relates_to` as a derived projection; `independent` as the default

### Decision

1. **Absence is the default.** A pair with no relationship has **no row**. Stop
   writing `independent` rows to mark "inspected and unrelated" for pairs that are
   *derivably* uncoupled (no shared file, no import path, no co-change). The
   `explored X/83436` denominator is **retired as a gate/obligation** and demoted
   to an optional, ranked discovery surface (consistent with the existing
   "horizontal grid … NOT a gate" language in `status`).

2. **Coupling is a 2-hop query, not a stored plane.** "Does intent A relate to
   intent B?" is answered by projecting over the structural substrate:

   ```
   A --implements--> file_a --(imports | shares-symbol | co-changes)--> file_b <--implements-- B
   ```

   This is computable from edges loom already stores (`implements` + extracted
   import/symbol facts) plus the on-demand git pass. It is *derived* (the import
   path) or *statistical* (the co-change), never an enumerated intent×intent
   checklist.

3. **The escape hatch is a registered asserted edge.** Indirect wiring (event
   bus, DI, config-keyed dispatch, RPC) has no structural path, so the projection
   cannot see it — a human/LLM asserts it as `RELATES_TO` kind `manual`,
   truth-class `asserted`. A deliberate `independent` ("these *look* coupled but
   are not") is likewise an *asserted* edge with evidence — rare, judgment-bound,
   never the default.

### The extension registry (so the hatch does not rot the metamodel)

Freeform edge naming would recreate the soup the typed metamodel exists to
prevent. Every non-core edge kind must be declared:

```
edge_kind_registry(
    kind            TEXT PRIMARY KEY,   -- 'manual' | 'event_bus' | 'config_key' | …
    truth_class     TEXT NOT NULL,      -- derived | asserted | statistical
    endpoint_types  TEXT NOT NULL,      -- e.g. '(intent,intent)' — keeps it typed
    owner_role      TEXT NOT NULL,      -- which lane may assert it
    promotion_note  TEXT NOT NULL DEFAULT ''  -- when a recurring hatch earns first-class status
)
```

Healthy loop: outlier → manual hatch → if the *same shape* recurs, promote it to
a first-class kind. The metamodel evolves under evidence rather than freezing or
rotting.

### Non-goal

No materialized intent×intent grid; no smell *nodes*; the vertical "done" spine
stays one-hop (per the multihop proposal). This part removes obligation, it does
not add a plane.

---

## Part III — Multi-hop: breadth (programmatic) vs depth (LLM), and `depends_on`

### Decision

Split traversal by who pays:

- **Programmatic = breadth.** Every *local* edge, bounded hops (import coupling =
  2 hops), deterministic, total, cheap. The engine never attempts deep semantic
  inference (that turns N² into N³).
- **Cognitive = depth.** The LLM does *selective* unbounded-hop semantic
  traversal, using the typed graph as a navigation scaffold and reading real code
  at each hop. Its conclusion is written back as an **asserted** edge.

### The one new mechanism: a typed dependency set

A multi-hop asserted edge is only as valid as the path it rode — and a path is
made of more than nodes. It depends on the *edges* that connect them (an
`implements` grounding can be re-pointed, an import edge can vanish) and on the
*locators* it read (a `fn foo` can be renamed or moved while the file still
exists). Watching node content hashes alone silently misses all three. So the
dependency set is **typed**:

```
-- additive columns on the asserted-capable edge tables
depends_on   TEXT NOT NULL DEFAULT '[]'   -- JSON list of typed dependency refs (below)
hop_evidence TEXT NOT NULL DEFAULT ''     -- the traversal chain, human-readable
```

Each entry in `depends_on` is one of:

| ref | written as | invalidated when |
|---|---|---|
| **node** | `{"node": "<id>"}` | the node's content hash changes |
| **edge** | `{"edge": "<id>"}` | the edge is removed, or its canonical fingerprint (endpoints, kind, `inspection_status`, locator, evidence) changes (additions have no ID yet — caught by **scope**) |
| **locator** | `{"locator": "<file>:<symbol>"}` | the symbol no longer resolves in the file (rename/move/delete) |
| **scope** | `{"scope": "<kind>", "args": {…}, "fingerprint": "<hash>"}` | the registered scope's canonical fingerprint, re-evaluated each sync, differs |

The first three watch *known* refs by identity. The **scope** ref is the dual:
it watches a *derivable set or predicate* by re-evaluating it and comparing a
stored fingerprint — the only way to catch an **addition** (a new edge has no
prior ID for an ID-ref to watch) or an assertion that depends on an **absence**.
A scope is **not an arbitrary query string** — it is a registered `scope_kind`
with typed args and a single canonical fingerprint function (same discipline as
the edge-kind registry in Part II), so invalidation is deterministic and safe.
Three canonical scope kinds:

- `implements-of` (args: `{intent}`; fingerprint: hash of the sorted file-id set)
  — re-opens when the set of files grounding an intent changes (a *new* grounding
  is added, or one is removed). An ID-ref alone cannot see the addition.
- `coupling-between` (args: `{file_a, file_b}`; fingerprint: `none` | hash of the
  current coupling kinds) — the *file-level* primitive: re-opens the instant a new
  import/co-change appears between two files.
- `coupling-between-intents` (args: `{intent_a, intent_b}`; fingerprint: hash over
  *both* implementation sets **and** the coupling kinds of *every* cross-set file
  pair) — the **load-bearing one for deliberate `independent` assertions** between
  intents (Part II). Because an intent fans out to many files, "A and B do not
  relate" depends on every file-pair *and* on the fan-out itself: this composite
  re-opens when either intent gains/loses a grounding **or** any cross-file
  coupling appears. A single `coupling-between` would miss a new coupling through a
  newly-added file. Without this, an intent-level independence claim rots silently
  — the INV-1/AT-6 failure.

New scope kinds are added to a `scope_kind_registry` (kind → arg schema →
fingerprint fn), never improvised at write time.

- A single-hop assertion's `depends_on` is its two endpoint nodes (back-compat
  default).
- A 5-hop conclusion records every node, **every edge it traversed**, every
  **locator it read**, and a **scope** for any set/absence it relied on along the
  path.
- **`sync` re-opens an asserted edge (→ `needs_reverification`) iff any ref in its
  `depends_on` is implicated**: a node hash moved, a depended-on edge was
  added/removed/restatused, a locator stopped resolving, or a scope's re-evaluated
  fingerprint differs. Touching anything not in the set does nothing.

This is build-system dependency tracking applied to memoized cognition —
expensive reasoning cached, invalidated precisely. It deliberately does NOT reuse
the global `structure_version` stamp (from the multihop proposal) as the trigger:
that stamp bumps on *any* structural change anywhere, so keying off it would
re-open every multi-hop edge on every edit — the exact thrash `depends_on` exists
to avoid. `structure_version` stays the cache key for *whole-graph* metrics
(centrality, SCC); `depends_on` is the *per-edge* dependency set. Different
granularity, different job.

Without `depends_on`, multi-hop assertions either go stale silently (wrong
intelligence) or force re-judging the world on every edit (thrash). It is the
price of durable depth, and it is small.

### Non-goal

No full transitive-reset ripple (the flood the multihop proposal already
forbids). `depends_on` invalidation is *targeted*, keyed on the recorded path,
not on the transitive closure.

---

## Part IV — Quality: behavioral rides intents, physical rides files

### Decision

The stress test that decides this: **a god-file split into many small files.**

- **Behavioral quality rules** (judgment: "single responsibility?", "error
  handling adequate?") stay on **intents** — `governs : quality_rule → intent`,
  exactly as today. The intent is the **stable identity that survives file
  churn**: when the god-file becomes five files, the intent persists, its verdict
  goes `needs_reverification`, and the LLM re-judges *once*. History is preserved
  on the intent, not lost with the deleted file node.
- **Physical file rules** (deterministic: "≤ N lines", "no panic markers",
  "license header") are **derived metrics on the codefile**, truth-class
  `derived` — recomputed automatically on every sync, never a GOVERNS verdict,
  never queued. loom *already* routes these to computed smells (`oversized_file`,
  `large_behavioral_symbol`, `complex_symbol`); this part only *names* them
  `derived` and stops counting them as inspection debt.

### Why not a quality node per file

It re-explodes node count (rules × files) and re-creates the materialization
wall. The GOVERNS *edge* already carries `criterion + inspection_status +
evidence + locator` — that **is** the per-target quality record. When a quality
problem becomes *work*, the node already exists: the `needs_change` intent. No
new node type.

### Non-goal

No `governs : quality_rule → codefile` edge for behavioral rules; no per-file
quality nodes; no stored physical-metric verdicts.

---

## Part V — Metadata: typed facets vs free-text payload

### Decision

Stop fighting honesty (rich free text) against query speed (typed fields).
Separate them:

| layer | content | indexed |
|---|---|---|
| **payload** | `evidence`, `criterion`, notes, `hop_evidence` — prose a human/LLM reads | no |
| **facet** | `truth_class`, `kind`, `target_type`, `locator`, `depends_on`, `note.kind` | yes |

Much of this exists: `idx_note_kind`, `idx_note_target`, `idx_inbox_status_kind`,
`idx_relates_status_priority`. The additions are targeted: index `truth_class`
where routing reads it; add a `note.kind='explanation'` for the **fan-out
collaboration narrative** (intent → "parser.rs tokenizes; validator.rs checks
invariants; store.rs persists") — the code-intelligence surface that rides
`implements.criterion` and the new note kind.

### Derived debt is recomputed, never stored

"Find debt fast" must NOT become "store a debt blob." Statistical/derived debt is
a computed ranked view (`loom debt top`, proposed separately). Only **adjudicated**
debt is persisted — a `needs_change` intent or an inbox card with a source tag —
and those are *already* typed and indexed. Storing derived debt is precisely what
built the 18k wall.

---

## Invariants (implementation-checkable)

- **INV-1 — No grid materialization.** `independent` rows exist only as
  *asserted* deliberate-independence claims with non-empty evidence. A graph at
  rest has `count(relates_to WHERE inspection_status='independent' AND
  truth_class='asserted' AND evidence='') == 0`. The `explored/83436` ratio is
  not an input to any gate.
- **INV-2 — Derived layer is rebuildable.** Deleting all `truth_class='derived'`
  edges and running `loom sync` reproduces the identical derived edge set and
  statuses. (Golden test on a fixture repo.)
- **INV-3 — Asserted edges carry typed provenance.** Every `truth_class='asserted'`
  edge has a non-empty `depends_on` containing at least its two endpoint nodes; a
  multi-hop edge additionally records every node, every traversed edge, every
  locator it read, and a `scope` ref for any set/absence it relied on. The
  write-time gate rejects an empty or endpoint-incomplete set.
- **INV-4 — Statistical is never an obligation.** No `truth_class='statistical'`
  value is ever persisted as an edge row, ever gates a maturity rung, or ever
  appears in `loom next` *required* debt. It appears only in ranked advisory
  feeds.
- **INV-5 — Status authorship is class-partitioned.** `derived` statuses are
  written only by sync/extraction; `asserted` statuses only by a role-gated
  verdict. No code path writes both classes through one function. (Enforced by
  the gate; audited by `loom doctor`.)
- **INV-6 — Targeted invalidation.** `sync` re-opens an asserted edge iff some ref
  in its `depends_on` is implicated: a `node` hash moved, an `edge` was removed or
  its canonical fingerprint changed, a `locator` stopped resolving, or a `scope`'s
  re-evaluated fingerprint differs. A change to anything not in the set leaves the
  edge untouched (no global `structure_version` trigger — that would thrash).

---

## Part VI — The end-state system: three planes, one loop

Fully migrated, loom stops being "one obligation surface with optional bits
bolted off" and becomes **three planes with different physics**, each owning a
command and a cadence:

| plane | truth-class | command | cadence | gate role |
|---|---|---|---|---|
| **structural** | `derived` | `loom sync` recomputes | after any code change | feeds gates; never itself queued |
| **judgment** | `asserted` | `loom next` routes the *stale residue* | per work item | the only required-debt queue |
| **signal** | `statistical` | `loom debt` / `loom smells` ranked feed | on demand | never gates; advisory only |

The loop an operator/LLM actually runs:

```
loom sync     → recompute ALL derived edges (deterministic, golden-testable);
                re-evaluate depends_on scopes; re-open ONLY implicated asserted edges
loom next     → the asserted residue: stale verdicts, broken groundings, needs_change.
                NEVER the grid, NEVER a co-change instance.
loom debt     → ranked statistical CLUSTERS (the compression layer). Confirm one
                → it becomes an asserted edge / refactor task; dismiss → a decision note.
loom explain A B  → the derived 2-hop coupling projection (no stored grid row)
loom intent show  → the fan-out collaboration narrative (Part V) — code intelligence
```

**Current vs proposed surface (no fabrication):** `loom sync`, `loom next`, and
`loom intent show` exist today. `loom debt` is **proposed** (there is no `Debt`
variant in the `Command` enum — its function is served today, unranked, by `loom
smells` / `loom hotspots` / `loom impact`); the ranked-cluster `loom debt` is the
new compression command this proposal calls for. `loom explain` exists but the
`loom explain A B` *coupling-projection* read shown here is **proposed** (today
coupling is surfaced via `loom cluster` / `loom edge explore`). Both proposed
forms are target UX, flagged so this section can't mislead implementation.

**Data model at rest:** the same 10 node tables and 9 edge tables, plus a
`truth_class` classification (derived in code, Part I), `depends_on`/`hop_evidence`
on asserted edges (Phase 1), and the `scope_kind_registry` + `edge_kind_registry`.
No grid rows; no stored statistical edges; `independent` only as an
evidence-bearing assertion carrying a coupling scope. **The system is smaller than
today's**, because the next four parts are mostly subtraction.

---

## Part VII — The hard cut

What is removed, reframed, or kept. Grounded in the current code so it is
actionable, not aspirational. (`R` = reframe/rename, keep the mechanism; `C` =
cut as a concept/obligation; `K` = keep unchanged.)

| surface (today) | location | verdict | becomes |
|---|---|---|---|
| `excellence_debt = advisory.len()+debt.len()+advisories.total` (raw instance sum) | `status.rs::certification_rollup` | **R** | count of **un-triaged statistical clusters**, not raw instances — the `18,352` headline dies |
| `priority_unexplored_pairs` rendered as a non-optional `horizontal-risk` queue | `next/render.rs` | **C** | the inconsistency is deleted; statistical pairs live ONLY in `loom debt`, never a required queue |
| `explored` as a 360° coverage axis (`X/83436`) | `stats.rs::explored_pairs_axis` | **C** | dropped from the vector; coupling is a derived projection, not a coverage denominator |
| `unexplored_pairs` full-survey denominator | `stats.rs`, `scoring.rs::count_unexplored_pairs_from` | **R** | kept as an on-demand projection input for `loom debt`; removed from any status headline |
| stored `independent` rows (5,241) | `relates_to` table | **C** | absence is the default; only evidence-bearing deliberate independence survives (Part II) |
| three-exception "N×N grid re-staling" ripple rule | `guide.rs` RIPPLE; `sync.rs` import-delta | **R** | replaced by the cleaner `coupling-between-intents` scope dependency (Part III) — simpler to teach |
| `horizontally_explored` gate `rt_uninspected==0 && rt_needs_rev==0` | `stats.rs`; `maturity.rs` Hardened | **R** | renamed "asserted coupling residue clear"; mechanically identical, stops implying a grid |
| `Excellent` rung gated on raw advisory debt | `maturity.rs::maturity_ladder` | **R** | gated on **triaged clusters** via existing `RungStatus::NotApplicable` (Part IX) |
| `edge unexplored`, `cluster`, `hotspots`, `impact` | `cli.rs`, `edge.rs` | **K** | useful navigation — kept, but reframed as advisory feeds, never obligations |
| `smells` (computed views) | `smells.rs` | **K** | already correct — the statistical plane; only the *accounting* changes |

**Not cut (explicit):** no node/edge *table* is dropped; no command is deleted
outright (the grid-management commands are demoted to advisory, not removed —
removing tools the operator may still want would be its own kind of arrogance).
The cut is to **obligations and headline numbers**, not capabilities.

---

## Part VIII — The self-teaching surface under truth-class

The teaching surface (`guide.rs` + the gate) is where the paradigm becomes
legible to the next LLM — and it is where the reform pays the largest *verbosity*
dividend, because three named planes replace a tangle of grid quotas, exception
rules, and an `explored` axis.

**What is taught NEW (the spine):** the three planes (derived / asserted /
statistical) become the first thing `loom guide` teaches — "machine computes
structure, you judge meaning, statistics only suggest." Every lane is a verb on
one plane.

**What shrinks or is rewritten:**

- **The analyzer lane (`guide.rs` ROLE_DISCIPLINE `analyzer`)** stops being "grind
  the RELATES_TO grid." Its charge becomes: *confirm derived coupling candidates*
  (the projection proposes; you adjudicate) and *memoize multi-hop conclusions*
  with a `depends_on` set. The "BULK: flood thousands of grid edges" line
  (`guide.rs` ROLE_DISCIPLINE + orchestration notes) is **cut** — there is no grid
  to flood.
- **The 360° teaching (`guide.rs` DEEPER, "grounded · realized · explored ·
  measured · proven")** drops `explored`. The axis no longer exists.
- **The RIPPLE teaching (`guide.rs` RIPPLE)** — today three subtle exceptions about
  when an `independent` edge re-opens — collapses to one rule: *an independence
  assertion re-opens when its `coupling-between-intents` scope fingerprint
  changes.* One sentence replaces a paragraph.
- **The INDIRECT-WIRING teaching (`guide.rs` DEEPER + brownfield `discover`)** is
  **kept and enriched**: it is the escape hatch (asserted `manual` edge), and it
  now MUST carry a coupling scope so it is invalidatable — teaching honesty *and*
  the mechanism in one place.
- **The quality lane (`guide.rs` ROLE_DISCIPLINE `quality`)** gains the explicit
  behavioral/physical line: physical limits are *derived metrics you never
  verdict*; you verdict *behavioral* rules on intents (Part IV).

**What is untouched:** the gate (`gate.rs::LANES`, `enforce_lane`,
`mode_for_role`) and the five role lanes (builder/analyzer/fixer/validator/
quality) keep their ownership boundaries — the anti-laundering guards become MORE
central, because the asserted layer's quality is the only thing the machine
cannot check. The JIT skill model (`loom guide --role <role>`) is unchanged; only
the *content* of the analyzer/quality skills changes.

Net effect on the teaching surface: **fewer concepts, one mental model, no grid
vocabulary.** Less to read every turn — the literal "less verbose" the goal asks
for.

---

## Part IX — Phase & maturity refactor (you allowed this)

The phase machine and the ladder are kept as *shapes* — they are good — but two
rungs are redefined so the architecture's new order shows through.

### The phase cascade stays five literals, `harden` is clarified

`decide_phase` (`stats.rs`) keeps `shape → realize → complete → harden → green`.
The only change is at **`harden`**: it gates on the **asserted coupling residue**
(stored RELATES_TO `uninspected`/`needs_reverification`) + behavioral measurement
+ behavioral siblings — and is *provably independent* of the grid denominator and
every statistical signal. This **resolves the `next/render.rs` inconsistency**:
one source of truth (`truth_class`) decides optional-vs-required, so the
`horizontal-risk` queue can no longer contradict the three modules that call it
optional.

### The ladder keeps six rungs, two are redefined

`maturity_ladder` (`maturity.rs`) keeps `Seeded → Realized → Proven → Hardened →
Production-ready → Excellent` and the `RungStatus {Met, Partial, Unmet,
NotApplicable}` vector. Changes:

- **Hardened** — "RELATES_TO risk backlog not closed" (`maturity.rs`) is reworded
  to "asserted coupling residue" and explicitly excludes statistical signal. Its
  routing lane stays `discovery`/`quality`.
- **Excellent — the rung the original frustration was about — is redefined.**
  Today it is gated on raw advisory debt (`18,352`), an unreachable cliff that
  reads as permanent red. New definition: Excellent is **`Met` when every
  statistical cluster above the impact threshold has been triaged** (each
  confirmed → asserted edge / refactor task, or dismissed → decision note), and
  **`NotApplicable`** when no cluster crosses the threshold. It uses the ladder's
  existing `NotApplicable` vocabulary, so Excellence becomes *achievable and
  honest* — "the signal feed has been triaged to the noise floor," not "18k items
  inspected." This is the direct fix for the daunting-number problem that started
  this whole design.

### The 360° vector

Drops `explored`; keeps `grounded · realized · measured · proven`; optionally adds
`triaged` (statistical clusters addressed / above threshold). The vector now
reports only axes that are either derived facts or asserted judgment — no
completable-grid illusion.

---

## Rollout — derive first, persist only when earned

Phased so the visible win lands at near-zero risk and nothing is persisted until
routing semantics are proven:

- **Phase 0 — code only, reversible, no schema change.** Implement
  `truth_class(edge) -> {derived,asserted,statistical}` as a pure function over
  `(table, RELATES_TO.kinds, inspection_status)`. Re-key the `status`/`next` debt
  rollup so `statistical` signals (co-change, shotgun) become a ranked advisory
  feed, not *required* debt (INV-4). This alone turns `excellence red (debt
  18352)` into the honest asserted residue, and it reverts by deleting one
  function — no migration to undo. Add the `independent`-pruning **dry-run** here
  too (report-only; no deletes yet).
- **Phase 1 — persist what cannot be derived.** Only when the Part III multi-hop
  layer is built: add the `depends_on` / `hop_evidence` columns (genuinely
  un-derivable, LLM-supplied) plus the `scope_kind_registry`. This is the first
  real migration, and it is justified because the data is new, not reclassified.
- **Phase 2 — index, only if hot.** Persist `truth_class` as a stored facet
  column (Part V) *only* if profiling shows the Phase-0 derivation is a read-path
  cost. Until then it stays derived. Execute the `independent` prune for real once
  the dry-run has been reviewed.

The migration notes below describe Phase 1/2 storage; Phase 0 touches no schema.

---

## Migration & back-compat

- **The classification rules are the derivation function (Phase 0), not a
  backfill.** The same `(table, RELATES_TO.kinds, inspection_status)` mapping —
  import-coupling/shares-file → `derived`; `manual` → `asserted`; verdict tables →
  `asserted`; git suggestions → `statistical` — is computed in code, so no column
  is added in Phase 0. Physical-metric smells were never edges → nothing to
  migrate. If `truth_class` is later persisted (Phase 2), the same function seeds
  the one-time backfill via `ensure_taxonomy_columns`.

  The function is **total over all nine edge tables** (the default is `asserted`,
  because most edges are recorded claims/verdicts/design decisions; only the
  mechanically-extracted ones are `derived`):

  | edge table | truth_class | basis |
  |---|---|---|
  | `relates_to` | `derived` if `kinds ⊆ {import-coupling, shares-file}`; `asserted` if `kinds ∋ manual` or a deliberate `independent`; (`statistical` kinds are never stored as rows) | by `kinds` |
  | `calls` | `derived` | extracted mechanically from the saga spec |
  | `hierarchy` | `asserted` | a design decision (parent/child), not extracted |
  | `implements` | `asserted` | a grounding claim; its *staleness* is derived-detected (broken locator), but re-grounding is a builder judgment |
  | `governs` | `asserted` | quality verdict |
  | `validates` | `asserted` | proof link (the validation *result* may be derived when a test runs, but the edge is a claim) |
  | `targets` | `asserted` | hypothesis link |
  | `serves` | `asserted` | persona verdict |
  | `journeys` | `asserted` | persona journey link |

  Rule of thumb: **`derived` = `calls` ∪ mechanically-kinded `relates_to`;
  everything else is `asserted`; `statistical` is never an edge row.**
- **`depends_on` is the only true migration (Phase 1).** It and `hop_evidence` are
  additive columns added when the multi-hop layer lands; existing single-hop
  asserted edges default `depends_on` to their two endpoint nodes.
- **Co-change needs no migration.** It is already computed, never stored — it
  begins life on the correct (`statistical`) side. This proposal only stops
  *counting* it as required debt.
- **`independent` pruning is a one-time, reversible pass.** Existing 5,241
  `independent` rows are reclassified: those reconstructible as derivably-uncoupled
  (no shared file / import / co-change) are *droppable* (absence becomes the
  default); those carrying real evidence become `asserted` deliberate-independence
  claims. The pass is dry-runnable (`--dry-run`) and logged.
- **Travel format.** `loom.graph.json` gains `truth_class` / `depends_on` per
  edge. Old exports without them import with the same inference rules, so a
  pre-reform export still rebuilds (the re-init port: `loom init . && loom import`).
- **Re-init port.** Existing graphs move over by loom's standard re-init +
  import; the rescan re-derives the derived layer (INV-2), so the migration is
  self-healing.

---

## Acceptance tests

- **AT-1 — God-file split (the decisive scenario).** Intent `I` grounds to
  `god.rs`; behavioral GOVERNS verdict `passing`; physical `oversized_file`
  metric present. Split `god.rs` → `a.rs`,`b.rs`,`c.rs`. After `sync`: (a) physical
  metrics recompute deterministically over three files with **no verdict write**
  and `oversized_file` clears; (b) the behavioral GOVERNS verdict on `I` goes
  `needs_reverification`; (c) `I`'s identity and verdict history are intact;
  (d) one re-judge restores `passing`.
- **AT-2 — Derived rebuild determinism.** Snapshot all `derived` edges; delete
  them; `sync`; assert the rebuilt set is byte-identical (INV-2).
- **AT-3 — Targeted invalidation across every ref type.** Asserted edge `A→E`
  with `depends_on` = nodes `[A,B,C,D,E]`, the traversed `edge` `B→C`, the
  `locator` `c.rs:fn step`, and `scope` `coupling-between(d.rs,e.rs)`. Each of
  these, mutated alone, must re-open the edge: (a) change `C`'s file content →
  node ref trips; (b) re-point or delete edge `B→C` → edge ref trips; (c) rename
  `fn step` in `c.rs` → locator ref trips; (d) add a *new* import between `d.rs`
  and `e.rs` → scope fingerprint trips (the case ID-refs cannot see). Reset, then
  change unrelated node `F` and add an unrelated import elsewhere → edge stays
  `passing` (INV-6).
- **AT-4 — No N×N growth.** Add 100 unrelated intents. Assert `relates_to` row
  count grows O(asserted edges added) ≈ 0, not O(N²); the coupling projection
  still answers per-pair on demand (INV-1).
- **AT-5 — Statistical never required.** A co-change signal between two intents
  never appears in `loom next` required debt and never moves a maturity rung; it
  appears only in the ranked advisory feed (INV-4).
- **AT-6 — Independence requires evidence and a composite coupling scope.** Writing
  an `asserted` `independent` edge between two intents with empty evidence is
  rejected at the gate; a valid one must carry a `coupling-between-intents` scope
  ref. Adding a grounding to either intent, or a new import between *any* cross-set
  file pair, re-opens it on next `sync` (INV-1, INV-3, INV-6).

---

## What we are NOT building (explicit non-goals)

- No materialized intent×intent grid; the `explored/83436` denominator is not a
  gate.
- No smell *nodes* — smells stay computed views until adjudicated into a
  `needs_change` intent.
- No per-file quality nodes; no `governs → codefile` for behavioral rules.
- No stored derived/statistical debt blob.
- No full transitive-reset ripple; `depends_on` invalidation is targeted.
- The vertical "done" spine stays one-hop.

---

## Validation against loom's own graph

Applying the reform to the numbers above:

- **Required debt collapses.** The headline `18352` is ~98% statistical
  (co-change 17,961 + shotgun 372). Under INV-4 it becomes a *ranked feed*, not
  required debt. The asserted residue — genuine judgment-bound stale verdicts and
  the 552 stale edges — is what reaches `loom next`. That is the number an
  operator can actually act on.
- **`independent` storage shrinks.** Of 5,241 `independent` rows, those derivably
  uncoupled drop (absence is the default); only evidence-bearing deliberate
  independence survives as `asserted`.
- **Determinism becomes testable.** The derived layer (imports, metrics,
  staleness) gains AT-2 as a permanent regression guard.
- **Code intelligence improves, not degrades.** Noise compresses; the fan-out
  narrative (Part V) and memoized multi-hop paths (Part III) are *new*
  navigational value; the derived layer is trustworthy. The asserted layer's
  quality remains `= f(gate discipline)` — defended by the anti-laundering gates
  (executor proof, fabricated-locator rejection, content-free-criterion routing)
  already hardened in this codebase.

---

## Success criteria — proving "better, less verbose" (not asserting it)

The win must be demonstrable on the same graph, by classification-correctness and
before/after deltas — never by a magic threshold that can be gamed.

- **SC-1 — Classification is total and disjoint.** Every edge maps to exactly one
  `truth_class` via the total derivation function (the nine-table mapping in
  Migration / Phase 0); the required-debt set is provably the `asserted`-stale set
  and provably contains zero `statistical` items. (Test, not a number.)
- **SC-2 — Statistical leaves the gating headline (before/after).** On loom's own
  graph, the gating headline today folds the `18,352` advisory sum into
  `excellence_debt`; after, the gating headline is `required_autonomous +
  required_human + asserted-stale`, and the statistical count appears ONLY in the
  `loom debt` feed. Measure the delta on the identical graph — the statistical
  count must move surface, not shrink by fiat.
- **SC-3 — Output is structurally bounded.** `loom status` / `loom next` headline
  size is bounded to the asserted residue + top-N clusters, independent of graph
  size — a structural property, not a target count. Capture status byte/line count
  before/after.
- **SC-4 — One source of truth (the inconsistency dies).** A test asserts
  `priority_unexplored_pairs` (and any `statistical` edge) never appears in a
  required queue in `next/render.rs` — the `stats.rs`-says-optional /
  `render.rs`-says-required drift is gone.
- **SC-5 — Code-intelligence parity or better, with fewer items.** A FIXED
  question set answered pre- and post-reform: "how are A and C related?", "what
  proves intent I?", "how is I implemented across its files?", "what changes if
  file F changes?". Each must return the same-or-better answer while the graph
  stores/queues fewer items. The fan-out narrative (Part V) and memoized multi-hop
  paths (Part III) are net-new answers the old grid could not give. This is the
  honest test that "less" did not cost intelligence.
- **SC-6 — Determinism is now provable.** AT-2 (derived-layer golden rebuild)
  passes — a regression guard that was impossible before the split.
- **SC-7 — Stored-row delta is reported, not targeted.** The `independent`-prune
  dry-run reports how many of the 5,241 rows drop; the number is an OUTCOME of the
  rule, never a goal to hit.

"Better" = SC-1/4/5/6 (correctness, consistency, parity, determinism). "Less
verbose" = SC-2/3/7 (the gating headline and stored rows shrink while capability
does not). If SC-5 ever fails — an answer gets worse — the reform is wrong on that
axis and stops until it doesn't.

## Open questions

- **Resolved — class lives on the edge, not the node.** One additive column; the
  Rust engine routes on it. Nodes are untyped by truth-class.
- **Resolved — physical rules are derived metrics, behavioral rules are asserted
  verdicts on intents.** The god-file test (AT-1) decides it.
- **Resolved — `independent` is not the default; absence is.** Deliberate
  independence is a rare evidence-bearing assertion.
- **Open — typed `depends_on` capture ergonomics.** How does the LLM declare the
  path cheaply at verdict time, now that a ref may be a node, edge, locator, or
  scope? Candidate: the traversal tooling records visited node/edge/locator refs
  automatically, the LLM adds only the `scope` refs it consciously relied on, and
  `loom edge … ground` accepts `--depends-on <ref,…>` while defaulting to the two
  endpoint nodes when omitted. Needs a small CLI + traversal-tool surface decision.
- **Open — promotion threshold for the extension registry.** How many recurrences
  of a manual hatch kind justify promotion to first-class? Start manual
  (maintainer judgment), add a count-based suggestion only if it leaks.
