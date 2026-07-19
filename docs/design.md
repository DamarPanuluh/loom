# loom v2 — Design

**Status:** CONCEPT-LOCKED — all §10 forks settled (provisional, reversible). The doc is the
authoritative v2 architecture; code follows ring by ring. The earlier loom (`../../loom`) is a
**read-only reference oracle** — consult it for *how* working mechanisms behave (tree-sitter
extraction, sync ripple edge cases, SQL/locking patterns, saga HTTP execution). We re-derive the
*code* clean; we never copy it. Carrying v1 code would carry v1's accumulated conventions and
coupling.
**Relationship to v1:** greenfield rewrite — the old loom is the read-only reference oracle
described above; we re-derive clean, never copy.

---

## 1. What loom is

loom maintains a **falsifiable graph of what a codebase is supposed to do, where that lives, and
how it is proven** — so an LLM can understand and safely evolve a codebase across a long horizon.
The model supplies judgment and evidence; loom supplies durable memory, queues, staleness,
coverage, and integrity gates. Every command's output is the prompt for the LLM's next decision.

The v1 lesson that defines v2: loom's value is **truth that doesn't rot and work that is honestly
ranked**. v1 proved the substrate (extraction, persistence, sync) but tangled three different
*kinds of truth* into one obligation surface, producing an 18,000-item "debt" wall that was 98%
advisory noise. v2 separates them from the foundation.

---

## 2. The spine: three truth-class planes

Every fact in the graph belongs to exactly one **plane**, defined by *how its truth is
established*. This is the organizing principle of the whole system — schema, CLI, maturity,
teaching all derive from it.

| plane | truth-class | who establishes it | command | gates? |
|---|---|---|---|---|
| **Structural** | `derived` | the machine (extraction) | `loom sync` recomputes | feeds gates; never itself queued |
| **Judgment** | `asserted` | a human/LLM verdict or design decision | `loom next` routes the stale residue | the ONLY required-work queue |
| **Signal** | `statistical` | a heuristic over history/structure | `loom debt` ranked feed | never gates; advisory only |

**Determinism boundary = truth-class boundary:**
- `derived` is **reproducible** — wipe it, re-run sync, get a byte-identical result. Golden-testable.
- `asserted` is **persisted until invalidated** — a pinned fact with evidence; sync re-opens it precisely when something it depended on changes.
- `statistical` is **never stored** — computed on demand, ranked, presented as a feed, never an obligation row.

The three rules that fall out, and must hold everywhere:
1. **Statistical never gates and never enters required debt.** It is a feed (`loom debt`), full stop.
2. **Derived facts are never re-judged; derived Finding nodes are served for asserted adjudication.** Sync owns derived fact status (a human never "inspects" an import); untriaged or stale Finding nodes still route to triage so the operator can write a separate asserted adjudication.
3. **Absence is the default.** A pair with no relationship has no row. "Independent" is a rare, *evidence-bearing* judgment, not a checkbox to fill for every pair.

---

## 3. The loop

```
loom sync     recompute the structural plane (deterministic); re-open only the asserted
              facts whose dependencies changed.
loom next     the asserted residue: stale verdicts, broken groundings, unbuilt intents.
              Never the grid. Never a co-change instance.
loom debt     ranked statistical clusters — the compression layer. Explicit
              `loom debt promote <cluster-id> --evidence <TEXT> [--confidence <0..1>]`
              creates exactly one asserted Finding (`source: debt_promotion`) that
              enters ordinary finding triage; dismissal is adjudicating that finding
              `rejected`, not a separate dismiss API. The raw feed stays advisory.
loom status   where you stand (maturity) + the single next move (compass).
```

---

## 4. Data model

### Nodes — cornerstones and their supporting families

Two **cornerstone** nodes anchor the graph, each with a family of **supporting nodes** that
carry the follow-up concerns their question raises (see §4d for the full rationale):

- **`Intent`** — what the code should do. The atomic "what." Family:
  `QualityRule` (compliance norms: security, performance, defect, style), `Validation` (proofs:
  test, assertion, benchmark, journey), `Hypothesis` (proposed changes — milestone 2).
- **`CodeFile`** — where the code lives. The "where." Family:
  `CodeRule` (structural norms: size, complexity, safety), `Finding` (evidence-backed observations; derived for programmatic producers, asserted for manual observations, §4d sub-fork), `InterfaceSurface` (public surfaces — milestone 2).

Auxiliary nodes (not cornerstone-anchored): `Note` (audit-trail text on any target),
`InboxItem` (un-decomposed intake). These are cross-cutting, not family members.

**The edge kinds are named after the family's role**, not after a vague verb: `governs` from the
quality-rule family, `validates` from the validation family, `flags`/`assesses` from the
finding/code-rule family. The `edge_kind_registry` (§4) enforces that each kind's endpoints match
its family's node types — which is exactly the type-safety the unified edge model needs.

### Edges — LOCKED (A) unified typed edge table

v1 used **nine separate edge tables** (hierarchy, implements, governs, validates, relates_to,
targets, serves, journeys, calls). That gave per-endpoint FK integrity but scattered the
truth-class logic across nine places and made "every edge has a truth_class" a cross-cutting
patch rather than a column.

Two candidate models for v2:

- **(A) Unified typed edge table.** One `edge(from, to, kind, truth_class, status, criterion,
  evidence, confidence, …)`. Honors the "uniform A —{kind}{status}— B" instinct; truth_class is
  one column, not nine patches; new edge kinds are data, not schema. Cost: endpoint types are not
  FK-enforced per kind (a `governs` edge's `from` must be a rule — enforced in code, not schema).
- **(B) Typed edge tables (v1 shape).** Keep per-kind tables; add `truth_class` to each. FK
  integrity per endpoint; query-planner clarity. Cost: the truth-class spine is replicated nine
  times; adding a kind is a migration.

**Assistant recommendation: (A) unified, with a small typed-endpoint check in code.** The whole
v2 thesis is "truth-class is the organizing axis" — it should be one column the router reads, not
nine. The FK loss is recoverable with a cheap validation layer. **Locked: (A) — see §10.**

**Worked shapes, so the call is concrete:**

```sql
-- (A) Unified. One edge table + one facet table that serves nodes AND edges.
CREATE TABLE edge (
  id          TEXT PRIMARY KEY,
  from_id     TEXT NOT NULL,
  to_id       TEXT NOT NULL,
  kind        TEXT NOT NULL,   -- hierarchy|implements|governs|validates|relates|triggers|sequence|flags|assesses|exposes|calls|covers|asserts
  truth_class TEXT NOT NULL,   -- derived | asserted
  status      TEXT NOT NULL DEFAULT 'uninspected',
  criterion   TEXT NOT NULL DEFAULT '',
  evidence    TEXT NOT NULL DEFAULT '',
  confidence  REAL NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL,
  inspected_by TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_edge_truth   ON edge(truth_class, status);  -- the router's hot path
CREATE INDEX idx_edge_kind    ON edge(kind, status);
CREATE INDEX idx_edge_from    ON edge(from_id, kind);
CREATE INDEX idx_edge_to      ON edge(to_id, kind);
-- endpoint typing enforced in code by an edge_kind_registry:
--   kind -> (from_node_type, to_node_type, truth_class, allowed_status[])
```

Under (A), "the asserted residue `loom next` routes" is ONE indexed query:
`SELECT * FROM edge WHERE truth_class='asserted' AND status IN ('uninspected','needs_reverification')`.
Under (B) it is a 9-table UNION. (A) is why concern 1 and concern 2 get simple: facets and
reactions are just more rows in the same two tables.

The cost is real and worth naming: a `governs` edge's `from` must be a `QualityRule` and `to` an
`Intent`. SQLite FKs can't express "depends on kind". So a write-time check reads the
`edge_kind_registry` and rejects a mistyped endpoint — the same guarantee, in code instead of DDL,
plus a `loom doctor` audit that re-validates every edge's endpoints against the registry.


### `truth_class` is a stored column from day one

Greenfield has no migration cost, so we do NOT repeat v1's "derived in code" stopgap. Every edge
row carries `truth_class` explicitly, set at write time by the plane that owns it.

### Provenance: `depends_on` — LOCKED (column in, single-hop until traversal lands)

The precise-invalidation mechanism (an asserted edge records the nodes/edges/locators/scopes it
relied on, so sync re-opens it only when one is implicated). Powerful but only pays off once
multi-hop LLM traversal exists. **Recommendation: design the column in, leave it single-hop
(endpoints only) until the traversal layer lands.** **Locked: column in, single-hop — see §10.**

---

## 4b. Facets: properties & tags — the index layer — LOCKED (canonical registry + `loom find --tag --where`)

The graph is also a **code-intelligence index**: an LLM must be able to *find what it needs* by
attribute, not just by traversing. That requires facets to be **canonical**, uniform across
nodes and edges, and indexed.

- **Property** = a typed `key=value` attribute drawn from a **registered property schema** (key,
  value-type, allowed values, meaning). One canonical key per concept — never free-form, because
  free-form drifts (`auth` / `authn` / `authentication`) and a drifted index is useless. Used for
  *filtering*: `visibility=user_visible`, `aspect=sad`, `layer=domain`, `language=rust`.
- **Tag** = a membership label from a **registered vocabulary** (term + definition), many-to-many.
  Used for *grouping/discovery*: "the auth cluster", "every security-boundary edge".
- **Uniform on nodes AND edges.** Same facet mechanism both places — a `triggers` edge can carry
  `condition=auth-failure` and tag `security`, just as an intent carries `visibility` and `auth`.

**The unifying insight: facets have a truth-class too.**
- `derived` facets are recomputed by sync: a codefile's `language`, `loc`, `symbol_count`.
- `asserted` facets are pinned judgments: `visibility`, `layer`, `aspect`, tags.
This makes the index honest — a derived facet can never be stale-but-trusted, and an asserted
facet carries provenance.

**LLM-facing surface:** `loom schema` prints the registry (every property key + tag + meaning);
`loom find --tag auth --where visibility=user_visible` returns a precise set via `(key,value)` and
(tag,target)` indexes. **Locked: canonical registry + `loom find --tag --where` — see §10.**

## 4c. Behavior taxonomy — capabilities, flows, and reactions — LOCKED (edge-native reactions, capability = intent + aspect)

Today we have two points — `intent` (atomic) and `saga` (flow) — and feel the missing middle.
The clean spine is **what → how it composes → how it's proven**:

1. **WHAT — the Intent (the only atomic *what*).** A "capability" is **not a new node** — it is an
   intent at capability altitude (a user-visible component/feature whose criterion is "the system
   can do X"). The `aspect` facet differentiates the kinds of behavior:
   `capability | happy | sad | fallback | edge_case | invariant`.
2. **COMPOSES — relationships between intents, expressed as edges (in the unified table):**
   - **Sequence** (`saga`/flow): ordered steps — "user does A then B then C." A journey.
   - **Reaction** (`trigger`/when-then): a condition/event and the response intents it requires —
     "if X happens, then Y and Z must hold." This is the shape of error handling, security rules,
     business rules, invariants — your "if something happens it should do this and this."
3. **PROVEN — a Validation** exercises an intent or a composition (unit/assertion test, saga run,
   scenario run).

So *sagas and reactions are both compositions* — sagas are the sequential kind, reactions are the
conditional kind. That unifies them and keeps the node count small.

**The fork — how to model a reaction:**
- **(i) Edge-native.** A `triggers` edge from the triggering context to each required-response
  intent, carrying a `condition` property and a shared `scenario` tag to group the set. No new node.
  Maximally streamlined; ideal for "on X, do Y."
- **(ii) Scenario node.** A first-class `{ given, when, then[] }` spec referencing intents, proven
  by a validation. More structure for rich multi-condition contracts; one more node type.

**Assistant recommendation:** start **edge-native (i)** — it rides the unified edge model and
keeps v2 small — and promote a reaction to a Scenario node only when contracts get genuinely rich
(multiple conditions, ordered responses). Capability = intent + `aspect`, never a new node. **Locked: edge-native — see §10.**

## 4d. Supporting nodes — the cornerstone families — LOCKED (families with norm/occurrence split; Finding = single observation node — LOCKED)

"Govern" is too broad: one edge carrying security, performance, defects, structure, and
observability flattens fundamentally different *concerns* into one undifferentiated relation —
a defect verdict and a performance verdict have nothing in common except the word "govern." The
fix is **not** to add attributes to the govern edge; it is to recognize that each cornerstone
node has a **family of supporting nodes**, and the edge kinds are named after the family.

### The principle

Each cornerstone node answers a question. Its supporting nodes answer the *follow-up* concerns
that question raises:

| cornerstone | question | supporting-node family | the edge kind |
|---|---|---|---|
| `Intent` | what should the code do? | `QualityRule` (compliance: security, performance, defect, style) | `governs` |
| `Intent` | ↑ | `Validation` (proof: test, assertion, benchmark, manual_check) | `validates` |
| `Intent` | ↑ | `Hypothesis` (a proposed change to this behavior) | `targets` |
| `CodeFile` | where does it live? | `Finding` (a code-quality concern at a specific location) + `CodeRule` (the reusable norm it's an occurrence of) | `flags` + `assesses` |
| `CodeFile` | ↑ | `InterfaceSurface` (a public surface this file exposes) | `exposes` |

So "govern" is not one edge — it is the **`governs` edge from the `QualityRule` family**. A
security rule, a performance rule, and a defect rule are three distinct `QualityRule` nodes, each
linked to an `Intent` by its own `governs` edge with its own status. The `QualityRule` node
carries the *concern* (its `category` property: `security | performance | defect | style | robustness`);
the edge carries the *verdict* (passing/failing/independent) and the *evidence*. This separates
the norm from the measurement — they were conflated in v1 under one edge.

### Why families, not attributes on the edge

- **A rule is a reusable norm, not a per-edge property.** "ISO 5055 dead-code detection" is one
  rule; it governs many intents. Storing it as an attribute on each `governs` edge duplicates the
  rule and loses its identity. A `QualityRule` node is the single source of the norm; the edge is
  the verdict against it.
- **A rule has its own lifecycle** (seeded from a pack, refined, retired). Edges don't.
- **A rule carries its own metadata** (detection logic, severity, effort class). Edges carry
  evidence; rules carry *what to look for*.
- **The same applies to every family.** A `Validation` is a reusable proof (runnable, re-runnable);
  the `validates` edge is the link to an intent. A `Finding` is a concern at a location; the
  `flags` edge links it to the file it's about, and `assesses` links it to the `CodeRule` it's an
  occurrence of.

### The cornerstone families (settled for MVP, extensible)

**Intent family** (what should the code do, and what holds it to a standard):
- `QualityRule` — a norm the code must satisfy. Property `category ∈ {security, performance,
  defect, style, robustness, …}`. Seeded from packs or authored. Edge `governs : rule → intent`.
- `Validation` — a proof that behavior holds. Property `type ∈ {test, assertion, benchmark,
  manual_check, journey, scenario, contract}`. Carries a `command`. Edge `validates : validation → intent`.
- `Hypothesis` (milestone 2) — a proposed change to an intent's behavior, proven before adoption.
  Edge `targets : hypothesis → intent`.

**CodeFile family** (where it lives, and what concerns its structure raises):

The Intent family splits norm (`QualityRule`) from verdict (the `governs` edge). The CodeFile
family needs the **same split** — a reusable standard is not the same thing as an occurrence at
a location:

- **`CodeRule`** — a reusable structural norm a file (or symbol) must satisfy. Property
  `category ∈ {size, complexity, style, safety, …}`. Seeded from packs or authored. Examples:
  "a file should not exceed N lines", "no `panic!`/`unwrap` in production code", "a function with
  >M branch count should split." A `CodeRule` is to a `CodeFile` what a `QualityRule` is to an
  `Intent` — the norm; not the measurement.
- **`Finding`** — an occurrence: a specific location (file + optional symbol) where a `CodeRule`
  is **violated or satisfied**. Programmatic findings use truth-class `derived` (recomputed by sync from the file's own content). Manual LLM/tool observations use truth-class `asserted` via `loom finding add`, with file/link evidence in the body. Derived producers add `flags : finding → codefile` (the location) + edge `assesses : finding → coderule`
  (which rule it's an occurrence of). Property `kind ∈ {oversized_file, complex_symbol,
  tangled_file, panic_marker, …}`. This is the **structural signal made a graph citizen** —
  physical code-quality concerns live as nodes so they can be queried, tagged, and adjudicated
  into work (a confirmed finding becomes a `needs_change` intent).

The split is what makes "what code-quality concerns exist in this file?" a graph query:
`CodeRule` is the standard, `Finding` is the occurrence, `flags` locates it, `assesses` names the
  norm it's an occurrence of. Without `CodeRule`, every `Finding` would carry its threshold and
  logic inline — duplicating the norm and losing its identity (the exact problem the advisory
  flagged: no reusable standard for file-level concerns).

- **`InterfaceSurface`** (milestone 2) — a public surface a file exposes. Edge `exposes :
  interface → codefile` is asserted-only; calls link via `calls : validation → interface`.

### What this changes from v1

v1 had `governs`, `validates`, `targets`, `serves`, `journeys`, `calls` as edge kinds but the
supporting nodes (`QualityRule`, `Validation`, `Hypothesis`, `Persona`, `InterfaceSurface`) were
treated as a flat secondary list, not as **families anchored to a cornerstone**. v2 formalizes the
anchor: every supporting node belongs to exactly one cornerstone's family, and its edge kinds are
named for the family's role (`governs` from the quality-rule family, `validates` from the
validation family, `flags`/`assesses` from the finding/code-rule family). The
`edge_kind_registry` enforces the endpoint types per kind — exactly the type-safety the unified
edge model (§4) needs.

### The norm/occurrence split, stated once for both families

| cornerstone | norm node | occurrence node | locating edge | norm-naming edge |
|---|---|---|---|---|
| `Intent` | `QualityRule` | the `governs` verdict (on the edge) | `governs : rule → intent` | — (the rule *is* the norm) |
| `CodeFile` | `CodeRule` | `Finding` | `flags : finding → codefile` | `assesses : finding → coderule` |

For the Intent family, the "occurrence" is the verdict itself — the `governs` edge *is* the
measurement (passing/failing/independent against the rule). For the CodeFile family, the
occurrence is a `Finding` node (it has a location and a kind), linked to both its file (`flags`)
and its rule (`assesses`). The asymmetry is real: Intent verdicts are one-hop (rule→intent), while
file findings are two-hop (finding→file + finding→rule). The unified edge model handles both
naturally — they're just different edge kinds with different endpoint types in the registry.

### Finding = single observation node — LOCKED

Structural findings (`oversized_file`, `complex_symbol`, `tangled_file`, `panic_marker`) are deterministic (derived from the file's own content, not git history) and have a specific location (file + symbol) that deserves stable identity for querying and adjudication. Manual LLM/tool observations are evidence-backed findings too, but asserted via `loom finding add`; their file/link evidence lives in the body and they do not get sync-owned `flags`/`assesses` edges. Statistical signals (`size_outlier` and shipped git-history `co_change` clusters; clone/shotgun/recurrence remain design-only) stay computed views in `loom debt`, never nodes.

Decision: **Finding is the one node type for evidence-backed observations.** Locked. Programmatic producers use derived findings; LLM/tool observations use asserted findings; both share listing, triage, adjudication, and stale-aware closeout. This keeps one signal lane without introducing an Observation/CodeAuditItem parallel store.

## 5. The maturity ladder

`Seeded → Wanted → Realized → Proven → Hardened → Production-ready → Excellent`, each a rung in
a vector (not a scalar), with the lowest unmet rung as the routing focus.

`Wanted` (a deliberate 2026-07 addition — see `rethink-lived-graph.md`) gates on **ratification**:
every active intent needs the human authority's evidence-bearing "yes, this is wanted". Any agent
(human or `llm:*` lane) may mint intents — a solo mint is born ratified (the minting act is the
evidence); a lane mint is born unratified and honestly fails this rung until a human runs
`loom intent ratify`. Ratification is the one write denied to every LLM lane (INV-8): the LLM may
author everything and ratify nothing. Redefining a ratified intent stales its ratification to
`needs_reconfirmation` — wantedness rots with meaning, like every other asserted fact.

Each rung's `state` is its own per-concern truth, computed independently, so the ladder never
lies by counting absent machinery as failure. But the **display** honors bottom-up order: any
rung above the lowest *unmet* rung is shown as blocked (`⊘ … (blocked by <gate>)`) rather than
satisfied, so a higher rung never reads as reached while a lower one is still open.
`NotApplicable` rungs are transparent — they never act as the gate.

The v2 fix baked in from the start: **Excellent gates on open findings/smells, not raw
statistical-debt instance count.** Statistical clusters stay advisory (`loom debt`); only an
explicit `loom debt promote` mints a separate asserted Finding that can enter ordinary finding
triage and therefore the Excellent open-finding gate. No 18k cliff from unpromoted heuristics.

`Hardened` gates on the **asserted coupling residue** (stale/uninspected asserted relationships)
plus — a deliberate 2026-07 revision — the **unmeasured quality pairs**: every non-deprecated
`QualityRule` crossed with every **root** implemented intent that has no `governs` verdict yet.
This is not the v1 grid coming back: it is bounded (roots only — a component verdict covers
descendants unless a leaf needs its own), computed on read, never stored as rows, and every pair
is served as real work by `loom next --mode quality`. The original "never on a grid denominator"
rule guarded against an *unbounded, un-adjudicable* denominator; a seeded rule nobody is ever
asked to measure is the opposite failure (a dead norm), so the ladder now refuses Hardened until
each seeded rule has been measured at least at root altitude. The single shared predicate lives
in `workitem::unmeasured_quality_pairs` so the ladder, the queue count, and the served work can
never disagree.

---

## 6. Invariants (the test contract)

These are the properties the test suite must guard from the first commit:

- **INV-1 — No grid materialization.** Adding N unrelated intents creates O(N) hierarchy edges, not O(N²) relationship rows.
- **INV-2 — Derived rebuildable.** Delete the structural plane, sync, get a byte-identical set.
- **INV-3 — Statistical never required.** No statistical signal is ever a stored edge, a gate input, or a `loom next` required item.
- **INV-4 — Absence is default.** `independent` rows exist only with non-empty evidence.
- **INV-5 — Class-partitioned authorship.** Derived status is written only by sync; asserted only by a verdict path. No function writes both.

Two more joined the contract during the build (the "7 invariants" of `build-plan.md` and
`graph-model.md`):

- **INV-6 — Evidence gate.** A `passing`/`failing`/`independent` verdict with an empty criterion
  or evidence is rejected at the write boundary.
- **INV-7 — Role gate.** A write from the wrong lane (per `LOOM_AGENT`) is rejected; unknown
  agent strings fail closed.
- **INV-8 — Ratification is human-only.** `loom intent ratify` is rejected for EVERY `llm:*`
  lane (not just wrong lanes), fail closed, no override flag. Absent ratification reads as
  unratified — wantedness is never presumed.

**Precision on INV-5 — invalidation vs authorship.** Sync never *authors* asserted truth, but it
does *invalidate* it: staling an asserted edge to `needs_reverification`, resetting an asserted
`Validation`'s result to `not_run`, or marking a `WikiPage` stale when its documented scope
drifts. Invalidation moves a fact to an "unknown, needs re-judgment" state and is exactly sync's
job; what is partitioned is the authoring of *settled* states (verdicts, evidence, adoption),
which only the judgment paths may write. Tests that assert "sync never writes asserted facts"
mean settled states, not invalidation.

---

## 7. Greenfield principles

- **Reference, never port.** Read `../../loom` to learn behavior; write fresh code. No copy-paste.
- **Rust, same proven crates.** `clap`, `rusqlite` (bundled), `tree-sitter` (+ rust/py/go/ts/js), `reqwest` (rustls, blocking), `serde`. This keeps `../../loom` a *directly usable* reference and re-uses battle-tested dependencies.
- **Docs are first-class.** Every module opens with a doc comment stating its plane and contract. A `docs/` tree explains the concept; the design doc and the code never drift (a check enforces it, as v1's wiki did).
- **Truth-class native.** The router reads `truth_class`; nothing infers it ad hoc.
- **Smaller than v1.** v2 is mostly subtraction — if a v1 concept doesn't serve the three planes, it does not come across.

---

## 8. What comes from v1, and how

| v1 mechanism | v2 treatment | reference value |
|---|---|---|
| tree-sitter extraction (`repo.rs`, `ts_imports.rs`) | re-derive clean; same crates | HIGH — the language quirks are hard-won |
| SQLite store + cross-process flock (`db/sqlite*`) | re-derive clean; same `rusqlite` (std file locking replaced `fs2` once Rust 1.89 stabilized it) | HIGH — WAL + locking patterns |
| sync ripple (`commands/sync.rs`) | re-derive on the three-plane model | HIGH — staleness edge cases |
| sagas / HTTP proofs (`saga/`) | port the *spec format*; rebuild the runner if the proof plane is in scope | MEDIUM |
| maturity/compass/stats | rebuild clean (this is what was tangled) | LOW — reference the gate *intent*, not the code |
| guide/teaching (130KB `guide.rs`) | rebuild minimal around three planes | LOW — v1's is the cautionary tale |

---

## 9. Build sequencing — LOCKED (rings 1–7 are the MVP)

The goal bar is: builds, tests pass, and the binary can **drive a real codebase** (dogfood on
itself). We must agree the minimum surface that clears that bar. Candidate MVP ring:

1. **Core model + store:** Intent/CodeFile nodes, the unified edge table, SQLite, `init`/`import`.
2. **Structural plane:** codefile registration, tree-sitter extraction, grounding (IMPLEMENTS), `sync` recompute.
3. **Judgment plane:** intent lifecycle, hierarchy, the `next` queue over asserted residue.
4. **Maturity + compass:** `status`, the ladder, the single-next-move router.
5. **Quality plane:** rules, GOVERNS verdicts.
6. **Signal plane:** smells computation, `loom debt` ranked feed.
7. **Coverage + export/import + dogfood.**

Deferred rings (only if we decide they're in the "fully working" bar): sagas, personas,
hypotheses, federation, wiki, vocab/layers.

**Recommendation: rings 1–7 are the MVP that clears "dogfoodable."** Sagas/personas/etc. are a
second milestone. **Locked: rings 1–7 — see §10.**

---

## 10. Decisions — LOCKED (provisional, reversible)

1. **Edge model → (A) Unified typed edge table** + one `facet` table for nodes and edges. §4.
2. **MVP scope → rings 1–7** (core model → structural → judgment → maturity → quality → signal →
   coverage/export). Sagas/personas/hypotheses/federation/wiki = milestone 2.
3. **`depends_on` → column designed in, single-hop (endpoints only)** until a traversal layer lands.
4. **Language → Rust**, same proven crates (clap, rusqlite bundled, tree-sitter, reqwest rustls,
   serde + serde_json; std file locking replaced fs2, serde_norway replaced the deprecated
   serde_yaml) — keeps `../../loom` a directly usable reference oracle. Plus
   `anyhow` for error handling (required by the repo's `rs-result-type` rule: `Result` aliases
   default `E = anyhow::Error`).
5. **Identity → binary stays `loom`**, built in `loom_new`, **fresh graph** (no v1 `loom.graph.json`
   import in MVP — v1 modeled the old concept; a v1→v2 importer is a later, optional bridge).
6. **Reaction model → edge-native (i)** (`triggers` edges + `condition` property + `scenario` tag);
   promote to a Scenario node only if contracts get rich.
7. **Facets → canonical registry** (property keys + tags, each with a truth-class) queried via
   `loom find --tag <t> --where <key>=<val>`.
8. **Finding + CodeRule → LOCKED.** `Finding` is the one evidence-backed observation node: structural producers create derived findings + CodeRule as reusable norm; manual observations create asserted findings; statistical signals stay computed views. §4d.
9. **Non-negotiable deferred rings for v1 → none** (clean MVP first; milestone 2 adds the rest).

---

*All decisions locked. Build ring by ring, each green before the next.*
