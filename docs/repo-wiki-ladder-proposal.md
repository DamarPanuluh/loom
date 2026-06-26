# Proposal: the repo-wiki ladder — a code-primary wiki with the graph as invisible manifest

**Status:** Proposal (v2 — hard-cut of v1). Companion to
[`maturity-ladder-proposal.md`](maturity-ladder-proposal.md),
[`intent-spectrum-proposal.md`](intent-spectrum-proposal.md), and
[`ui-ux-flow-proposal.md`](ui-ux-flow-proposal.md) — joins the same coherent
proposal set.
**Date:** 2026-06-26 (v2 — hard-cut replacement of the 2026-06-25 v1 proposal)
**Scope:** add a second, orthogonal "done" axis — **comprehension** — that an LLM
discharges by authoring a **code-primary** repo wiki whose prose links to source
files (not graph UUIDs), with the intent graph as an **invisible manifest** that
certifies the prose mechanically — coverage, provenance freshness, and
cross-link↔edge consistency — never by trusting prose. The LLM *proposes*, loom
*certifies* by resolving file-paths back to the graph at check time. Adds **no
node or edge types**.

**v1 → v2 reframe.** v1 (shipped 2026-06-25) was graph-primary: prose cited
intents via `[name](intent:UUID)`, structure was loom-plane-shaped
(architecture←hierarchy, glossary←vocab, flows←sagas). Research on Qoder's Repo
Wiki surfaced that a human wiki's vocabulary is the **codebase's** (file paths,
module names, symbols), not the tool's. v2 keeps v1's four gates (the part
Qoder structurally cannot do) but moves the graph behind the curtain: the reader
sees `[`src/saga/runner.rs`](../src/saga/runner.rs)`; loom internally resolves
that file to its intent and runs coverage/freshness/consistency against *that*.
The `intent:UUID` never appears in reader-facing prose. The graph becomes
loom's manifest — invisible to the reader, present for the gates.

---

## Summary

The concern: loom's `loom wiki` is a faithful *map* of the graph — overview,
architecture tree, components-by-domain, quality bars — but it is not a *guide*.
It is a deterministic projection: same graph → identical bytes, byte-`--check`ed
for freshness, deliberately "not a second teacher." A newcomer reading it learns
*what every responsibility is named*; it does not teach them *how the system
fits together, where to start, or how a request travels*. That cognitive layer —
the thing a human repo wiki is actually for — is missing, and it is missing on a
genuinely different axis from "is the code done."

**Recommendation: add a COMPREHENSION axis to the maturity rung-vector, not a
rung above it.** The maturity ladder asks *does the artifact behave?*
(`RECORD ≠ DISCHARGE` of behavior). Comprehension asks *can a human rebuild the
mental model from the writing alone?* (`RECORD ≠ DISCHARGE` of explanation).
These are independent — a Production-ready codebase can be totally opaque — so
comprehension joins the rung-vector as a parallel dimension, **routing-gated** to
begin only once the code axis is green. The work is drained by a new lane,
`loom next --mode wiki`, and discharged by a **code-primary wiki bundle** the LLM
authors.

The bundle is **two layers of one artifact**: the **manifest layer**
(frontmatter with `sourceFiles` + `symbols` + content-hash provenance stamp —
byte-`--check`able, machine-owned) and the **prose layer** (code-primary
narrative with file-path links — gate-checked, LLM-owned). v2 **fully replaces**
v1: the flat `loom.wiki.md` and the graph-primary OKF emitter are deleted; the
code-primary emitter + manifest resolver are loom's single wiki. The graph
stays the verification spine, never the reader's vocabulary.

The whole risk is that an LLM-written narrative collapses into
vibes-with-extra-steps — the exact failure `maturity-ladder-proposal` calls the
non-negotiable. It does not, because three of the four gates are mechanical:
**coverage** (every salient graph node is *grounded* by some page's
`sourceFiles`), **freshness** (every page's provenance stamp matches the current
content-hash of the files it cites — `loom sync` applied to a wiki page), and
**consistency** (every file-path link resolves to a grounded intent; every
relational claim in prose has a backing graph edge — a link asserting a
relationship the graph denies is a *fabricated-relationship* finding). Only prose
*quality* stays human-judged, and it lands in a **user-gated queue** exactly like
the visual-confirm residue in `ui-ux-flow-proposal` — loud, never machine-green.

It stays loom's **teach-adapt-over-hardcode** principle and the
maturity-ladder's **projection-not-schema** move: comprehension is a projection
over existing planes plus a frontmatter stamp, checked by a new command — no new
node or edge types.

---

## Context: the map loom has, and the guide it lacks

`loom wiki` (`src/commands/wiki.rs`) renders four sections from the graph:
*Overview* (counts), *Architecture* (the hierarchy tree with each intent's
one-line description), *Components & code* (intents grouped by domain, each with
its grounded files), and *Quality bars* (the rule corpus by category). It is
explicitly a **deterministic projection** — "same graph → identical bytes, so
`--check` is a byte comparison" — and its own header warns it is "not a second
teacher (agents drive the graph, humans read the wiki)."

That design is correct for what it is: a freshness-checkable completion artifact.
It is already load-bearing — wiki freshness (`wiki --check`) is the
**Production-ready** rung's *Durable* gate (`maturity-ladder-proposal` line 249).
Keep it.

But read `loom.wiki.md` as a newcomer. It is 105 intent descriptions in a tree,
then the same descriptions regrouped by domain, then 56 quality norms. Every line
is true and every line is a leaf fact. There is no "loom is a SQLite-backed
graph; a command opens the store, mutates under a transaction, and `sync`
re-hashes — start in `src/commands/mod.rs`." There is no walkthrough of how
`loom next` routes work, no "why endpoint-keyed edges instead of edge uuids," no
reading order. The map has every street labelled and no "you are here, the
downtown is over there." That guide layer is what a repo wiki is *for*, and it is
a cognitive act — it requires understanding, not projection.

loom already treats this kind of understanding as first-class on the behavior
side: the **cognitive dimensions** (`journey`, `behavioral` in
`src/db/queries/comprehensiveness.rs`) and the per-repo `rubric_teaching` are
"the LLM's cognitive role." This proposal extends *cognitive* from **modeling
completeness** ("did you enumerate every responsibility?") to **explanatory
completeness** ("can a human understand it?") — the natural next axis.

---

## The research basis: why code-primary (the Qoder comparison)

Research on Qoder's Repo Wiki (Alibaba's AI coding platform) grounded the v1→v2
reframe. Qoder generates a wiki committed to `.qoder/repowiki/` with this shape:

```
repowiki/{lang}/
├── index.md
├── architecture.md
├── dependencies.md
└── modules/
    ├── auth.md
    └── payments.md
```

Each page carries code-native frontmatter:

```yaml
---
kind: module
sourceFiles:
  - src/cli/commands/code.tsx
  - src/code/setup.ts
symbols:
  - codeCommand
  - buildCodeToolset
---
```

The body is prose *about the code*: module responsibility, core entry, key flows,
common modification points, risk points — with inline code blocks and file-path
references. A reader clicks `src/cli/commands/code.tsx` and lands in code. The
reader needs no tool installed.

**The comparison exposed a coupling in loom's v1 design.** v1 fused "what the
reader sees" and "what the machine checks" into one citation: `[name](intent:UUID)`
was both the reader's navigation link AND the machine's verification anchor. This
made the wiki unreadable without loom (the UUID is opaque) and made the structure
loom-plane-shaped rather than code-shaped.

```
Qoder:  [prose + code links]  ──read──>  human
        [manifest sidecar]    ──check─>  machine   (invisible to reader)

loom v1: [prose + intent:UUID] ──read──>  human   (LEAKS graph vocab)
                                ──check─>  machine   (fused citation)

loom v2: [prose + code links]  ──read──>  human     (code-primary, no UUID)
         [frontmatter manifest]──check─>  machine   (resolves file→intent→edge)
```

**But Qoder has two structural holes loom already solved:**

1. **No consistency gate.** Qoder's `DocumentStatus` includes `failed` and
   `paused` — a doc can be silently wrong and sit there indefinitely. Loom's
   *consistency* gate (a link asserting a relationship the graph denies is a
   fabricated-relationship finding) is structurally impossible for Qoder because
   it has no typed-edge model.
2. **No coverage gate.** Qoder has no notion of "uncited code." A module with no
   wiki page is just missing. Loom's coverage gate (every salient node cited by
   ≥1 page) makes "an unexplained responsibility" a first-class finding.

**The v2 resolution: take Qoder's reader-facing shape, keep loom's verification
spine, and make the graph the invisible manifest.** The graph resolves
file-paths to intents at check time — invisible to the reader, present for the
gates. This keeps every gate working (including the one Qoder cannot do) while
making the wiki readable without loom installed.

---

## The reframe: comprehension is a parallel axis, gated — not a rung above

`maturity-ladder-proposal` already established that "done" is a **rung-vector,
not a scalar," because the axes are genuinely independent (loom is *Hardened
before Proven*): "ordinal for routing, vector for truth." Comprehension is
simply another independent entry in that vector:

| | maturity (code) axis | comprehension (legibility) axis |
|---|---|---|
| **Asks** | does the artifact *behave* as specified? | can a human *rebuild the mental model* from the wiki alone? |
| **Discharge** | a discriminating test/saga that RAN | a wiki page that COVERS a node, is FRESH, and is CONSISTENT with the graph |
| **Surfaced** | rungs Seeded→Production-ready | one axis: `Documented ◐ N/M` |

Why a parallel axis and not a sixth rung *above* Production-ready: understanding
is not "more shipped." A library can be Production-ready and undocumented, or
richly documented and unproven. Stacking it as rung 6 would falsely claim
legibility is the last increment of shippability. It is a different question, so
it is a different vector entry.

**But routing is gated.** `loom next` *focuses* comprehension only once the code
axis reaches Production-ready. This is the user-surfaced insight, and it is
load-bearing, not cosmetic:

1. **Don't document churn.** Writing a narrative of a half-built subsystem
   documents a moving target — you rewrite it every sync. By Production-ready the
   code has stopped moving.
2. **Write from a trustworthy source.** Production-ready means the graph is a
   complete, proven, fresh source of truth. The wiki is a projection of it, so
   the source must be settled first.
3. **Stamps stay valid → tiny churn.** Because the code is stable, the
   provenance stamps (below) rarely break, so the comprehension queue is usually
   empty; an interjection re-stales only the touched page (the free-lane
   "local, proportional to blast radius" invariant).

So the vector reads, e.g., `Code: Production-ready ✓ · Documented ◐ 7/12`. Below
Production-ready it reads `Documented — (gated)`: honest, not a 0% block.

---

## The citation model: code-primary, graph-invisible

This is the core of the v2 reframe. **The reader's vocabulary is the codebase's;
the graph is the machine's backbone, never the reader's.**

### What the reader sees

Prose links to source files and symbols — plain markdown, no tool required:

```markdown
The saga runner ([`src/saga/runner.rs`](../src/saga/runner.rs)) halts at the
first failure and reports honest per-step outcomes: steps before the failure
passed, the failing step carries every broken expectation, steps after it
produce no outcome.
```

A reader clicks `src/saga/runner.rs` and lands in code. No UUID, no
loom-specific vocabulary in the prose body.

### What the machine checks

Each page's frontmatter declares its grounding — code-native, like Qoder's:

```yaml
---
type: module
title: "Saga runner"
sourceFiles:
  - src/saga/runner.rs
  - src/saga/spec.rs
symbols:
  - run_saga
  - SagaStep
provenance:
  src/saga/runner.rs: sha-abc123
  src/saga/spec.rs:      sha-def456
  intent:saga-runner:    structure_version-42
---
```

At `loom wiki --check` time, loom resolves each `sourceFiles` entry to its
grounded intent via the graph's IMPLEMENTS edges. The graph is the manifest:
it maps file → intent → edges, so every gate runs against typed graph state
even though the reader never sees a UUID.

**The `intent:UUID` appears nowhere in reader-facing prose.** It lives only in
the frontmatter `provenance` stamp (machine-readable, not rendered) and in
loom's internal resolution. This is the separation v1 lacked: the citation
mechanism (file paths in prose) is decoupled from the verification backbone
(graph resolution at check time).

---

## The structure: hybrid topical + per-component module pages

The bundle's structure is **hybrid**: a bounded set of topical pages for
cross-cutting concerns, plus one page per component intent for module deep-dives.
This reconciles two pressures:

- **Qoder's insight:** module pages (one per code module) are what a reader
  expects — "where is the auth code, what does it do." Code-shaped.
- **v1's insight:** bounded topical pages decouple doc-count from node-count,
  so a 1200-intent repo doesn't get 1200 files. A fixed set of cross-cutting
  pages is repo-agnostic.

### Topical pages (bounded, cross-cutting)

| Wiki page | loom plane it projects / is checked against |
|---|---|
| `index.md` | reading order + the coverage ledger |
| `architecture.md` | hierarchy + RELATES_TO + `loom layer order` + hotspots/centrality |
| `getting-started.md` | build/test validations, entrypoints, `loom detect` stack |
| `glossary.md` | the bounded `loom vocab` registry (near-direct projection) |
| `decisions.md` | the `kind=decision` notes (reason + history already stored) |
| `flows.md` | sagas + `user_visible` intents (the narrative spine) |

A repo with no journeys renders *Flows* "—" (auto-satisfied), exactly as the
maturity ladder renders a vacuous `boundary` dimension — that is what makes the
axis set repo-agnostic. ~6 topical pages, bounded, regardless of graph size.

### Module pages (one per component intent)

Each `component` intent gets a `modules/<slug>.md` page, grounded by its
IMPLEMENTS files:

```
loom.wiki/
├── index.md
├── architecture.md
├── getting-started.md
├── glossary.md
├── decisions.md
├── flows.md
└── modules/
    ├── cli-surface.md
    ├── sqlite-graph-persistence.md
    ├── self-teaching-surface.md
    └── ...
```

A module page's `sourceFiles` are the component's grounded files; its prose
explains the responsibility, key entry points, flows, and modification points —
Qoder's module-deep-dive shape, grounded in the component's code.

**The salient set stays altitude-calibrated:** components get module pages; the
83 leaf features do not — they are reachable through their component's page and
the topical skeleton. This keeps the count bounded (21 module pages on loom, not
105) while being code-shaped (each page is about a real code module).

---

## Coverage: every salient node grounded via sourceFiles

Coverage becomes: **every salient intent's grounded files appear in some page's
`sourceFiles`** (or are linked in prose, which loom extracts and resolves). The
RECORD ledger (identical in shape to the existing `journey`/`behavioral` ledgers
in `comprehensiveness.rs`) is the set of **uncovered salient nodes**:

- every `component` intent (owes a module page),
- every node in `loom hotspots` (high centrality / tangled files — the things a
  newcomer *must* be told about — owes a mention in a topical or module page),
- every discharged `user_visible` journey (owes a walkthrough in `flows.md`),
- every `InterfaceSurface` (owes an entry in `flows.md` or a module page),
- every `loom vocab` term (owes a glossary entry).

An uncovered salient node is an *unexplained responsibility* — a `--mode wiki`
queue item. Adding a new component later adds a module page; adding a leaf adds a
citation obligation to its component's page. Bounded structure, graph-backed
completeness.

Granularity is altitude-calibrated, like intent granularity itself: **system +
component + hotspots + journeys + vocab are salient; the leaf features are
not** individually owed a mention.

---

## Falsifiability: how legibility is certified, not trusted

This is the crux. The current wiki is checkable because it is deterministic;
LLM prose is neither deterministic nor byte-comparable, so `--check` cannot be a
byte diff of the prose. The maturity-ladder non-negotiable applies in full: *the
LLM proposes, loom certifies by mechanical roll-up; never auto-grant from LLM
judgment.* The comprehension axis is granted by four checks, three mechanical:

### 1. Coverage — mechanical

The RECORD ledger above: zero uncovered salient nodes. loom resolves each
page's `sourceFiles` to intents via IMPLEMENTS edges and checks every salient
intent is covered. Pure graph + frontmatter extraction.

### 2. Freshness — provenance stamp, not bytes, and emphatically not mtime

Each page declares the files it explains — *which are its `sourceFiles`* — and
is stamped at write time with their **content provenance**: the content-hash of
each source file (the same per-file hash `loom sync` already maintains) and the
`structure_version` of each linked intent (`structure_version` already bumps on
node/edge/lifecycle/layer change and `loom intent update` ripples a description
change one hop). The stamp lives in the page's committed frontmatter, so it
**travels with the repo and diffs in PRs**.

Freshness = recompute and compare. Match → fresh; mismatch → stale → the page
re-enters `--mode wiki`. This is **`loom sync` treating a wiki page as a grounded
node**: the same staling that flips an IMPLEMENTS edge when its file's content
changes flips a wiki page when a file it explains changes.

> **Rejected: mtime.** It does not survive `git clone` / `checkout` / CI
> checkout (checkout rewrites every mtime in write order, so after a clone "doc
> newer than code" is noise); it conflates a `touch` or formatter run with a
> semantic change; and it tracks write-order, not causality. Content-hash
> provenance answers the real question — *has the code changed since this was
> written about it?* — durably.

This is also the **honesty forcing-function**. The LLM cannot claim a page covers
`src/saga/runner.rs` without listing it in `sourceFiles`; that entry is then
(a) resolved to an intent and consistency-checked against the graph and (b)
hash-stamped so a later change re-opens the page. Honesty is not requested and
trusted — dishonesty *cannot satisfy the gate*, because coverage and freshness
are recomputed.

### 3. Consistency — the graph-aware advantage (what Qoder cannot do)

This is the gate that makes loom's wiki strictly better than Qoder's. Under
code-primary, loom resolves every file-path link to its intent and checks the
**typed edges** the graph holds:

- a file-path link to a **nonexistent or ungrounded** file → *dangling
  reference* (the file isn't in the graph or doesn't exist on disk);
- a **relational claim in prose** ("`src/a.rs` calls `src/b.rs`") → loom
  resolves both files to intents and checks for a backing CALLS / RELATES_TO /
  import edge → no edge = *fabricated relationship* (a hallucinated
  connection, caught by the graph);
- a link to a **retired** intent's files → *stale reference*;
- a high-centrality edge or hotspot **mentioned by no page** → *unexplained
  relationship* (a coverage gap, surfaced here too);
- a `vocab` term defined but **used in no page** → *dangling concept*.

**This is the gate Qoder structurally cannot do** — Qoder has no typed-edge
model, so a page can say "module X calls module Y" while the code does no such
thing, and as long as the file hashes match, nothing flags it. loom resolves
file-paths to intents and falsifies the relational claim against the graph. The
graph can prove the wiki wrong.

### 4. Prose quality — user-gated, loud, never machine-green

Whether the writing is *clear and well-organized* is the one thing no machine
can certify — so the design does not pretend to. It surfaces as a `manual_check`
with `inspected_by=human`, batched into the existing **user-gated lane** (the
same queue `ui-ux-flow-proposal` uses for visual residue, prioritized by user
presence alongside align / rulings / blocked). Honestly human-judged, loud, and
explicitly outside the mechanical green.

**The gate, in one line:** `Documented` is granted when coverage = 0 owed,
freshness = 0 stale pages, consistency = 0 fabricated/dangling links, and the
prose-quality queue is drained or consciously accepted.

---

## The artifact: manifest frontmatter + prose body

`loom wiki` does not get replaced; it gets **restructured**. The v1 emitter
(graph-primary skeleton) is revised to v2 (code-primary manifest + prose):

```
loom.wiki/                      ← wiki bundle (directory)
  index.md                      ← reading order + coverage ledger
  architecture.md               ← the layered shape, code-primary prose
  getting-started.md            ← build/run (stub until emitted)
  glossary.md                   ← the `loom vocab` registry
  decisions.md                  ← `kind=decision` notes with rationale
  flows.md                      ← user_visible intents + saga walkthroughs
  modules/                      ← one page per component intent
    cli-surface.md
    sqlite-graph-persistence.md
    self-teaching-surface.md
    ...
  log.md                        ← OKF history (optional, reserved)
```

Each page is **two layers in one file**:

- **Manifest layer (deterministic, loom-owned, byte-`--check`able).** The
  frontmatter: `type`, `title`, `sourceFiles`, `symbols`, `provenance` stamp.
  Same graph → identical frontmatter bytes, so this layer keeps the existing
  byte-`--check`. This *is* today's `render_wiki`, emitting frontmatter instead
  of graph-shaped prose.
- **Prose layer (provenance-stamped, LLM-owned).** The body under the
  frontmatter. Code-primary narrative with file-path links. Checked by coverage
  + freshness + consistency, never by bytes.

They are not two wikis competing for "source of truth" — they are two layers of
one bundle: loom guarantees the manifest and the cross-link integrity; the LLM
hangs narrative on the frame. The graph remains the single source of truth; the
bundle is downstream of it.

---

## The lane: `loom next --mode wiki`

A work-type in the existing `(rung-vector, focus, lane)` decomposition. It drains
the comprehension queue, one item with full context per call, like every other
lane:

- **write-gap** — an uncovered salient node: "explain component `<name>` (grounded
  in `<files>`); author `modules/<slug>.md` with code-primary prose citing those
  files."
- **stale-page** — a page whose provenance stamp broke: "`src/saga/runner.rs`
  changed; `modules/saga-runner.md` may be wrong — re-read and rewrite."
- **fabricated-link** — a relational claim the graph denies: "`modules/cli.md`
  claims `src/cli.rs` calls `src/db/sqlite.rs`; no CALLS edge exists. Fix the
  prose or record the edge."

Routing surfaces it only at Production-ready (the gate above); below that, the
lane reports `gated`. The free lane (`inbox` / `hypothesis` / `sync`) re-enters
it incrementally — a post-Production-ready interjection that alters an intent
stales only the pages that listed its files.

---

## Self-teaching for foreign repos: loom inits, then orders

The wiki machinery is loom-native: it works on any repo that loom has
initialized. The self-teaching loop for a foreign (non-loom) repo:

1. **Init.** `loom init .` on the target repo, then `loom import` (brownfield,
   from an existing graph) or `loom guide --mode seed` (greenfield, elicit
   intents). loom builds the intent graph — the manifest backbone.
2. **Map.** `loom sync` grounds files to intents; `loom detect` identifies the
   stack. The graph is now a complete model of the target repo's
   responsibilities.
3. **Order.** Once the code axis reaches Production-ready, `loom next --mode
   wiki` orders the LLM to author pages. The LLM reads the code (via file
   paths loom names), writes code-primary prose with file-path links.
4. **Certify.** `loom wiki --check` resolves every `sourceFiles` entry to its
   intent and runs coverage / freshness / consistency against the graph. The
   graph is the invisible manifest; the reader never sees a UUID.

This is the "loom-selfteaching machinery": loom teaches itself about a foreign
repo by building its graph, then orders wiki authoring with the graph as the
verification spine. The output is a wiki readable without loom (like Qoder's),
certified by loom's gates (unlike Qoder's).

---

## Mapping: what is REUSED vs NEW (v1 → v2)

| Concept | Status | Note |
|---|---|---|
| `loom wiki` deterministic projection | **reused** | becomes the manifest layer (frontmatter: `sourceFiles`, `symbols`, `provenance`) |
| `loom wiki --check` byte freshness | **reused** | guards the manifest layer (frontmatter bytes) |
| per-file content hash; `loom sync` staling | **reused** | the prose **freshness** mechanism (page ← sourceFiles) |
| `structure_version`; `intent update` ripple | **reused** | freshness for linked *intents* (in provenance stamp) |
| `loom vocab`, `kind=decision` notes, sagas, `loom layer order`, `loom hotspots` | **reused** | the per-page content sources + coverage denominators |
| user-gated `manual_check` queue (`ui-ux-flow`) | **reused** | prose-quality residue |
| comprehension axis in the rung-vector | **reused** | a vector entry, routing-gated behind Production-ready |
| `loom next --mode wiki` lane | **reused** | drains write-gap / stale-page / fabricated-link |
| **citation model: `intent:UUID` in prose** | **CUT** | replaced by file-path links; UUID lives only in frontmatter `provenance` |
| **structure: loom-plane-shaped topical only** | **CUT** | replaced by hybrid topical + per-component module pages |
| **manifest resolution: file → intent → edge** | **NEW (v2)** | the invisible backbone; loom resolves `sourceFiles` at check time |
| **frontmatter: `sourceFiles` + `symbols`** | **NEW (v2)** | code-native grounding (Qoder-style), replaces graph-native citation |
| **module pages per component** | **NEW (v2)** | one `modules/<slug>.md` per component intent, grounded by IMPLEMENTS files |

---

## Validation: the axis read against loom's own graph (2026-06-26, v2)

Computed read-only from loom's live graph (`loom status`, `loom vocab list`,
`loom hotspots`, `loom note list --kind decision`; the graph was not mutated).

- **The gate correctly says "not yet."** loom's live rung-vector is
  `Seeded ✓ · Realized ✓ 81/81 · Proven — · Hardened ✗ · Production-ready ✗`,
  focus **Hardened**. loom is not Production-ready, so the comprehension axis
  reads `Documented — (gated)`. The model obeys its own ordering.
- **The v1 emitter shipped (graph-primary).** `loom wiki --okf` emits a 7-file
  bundle with `intent:UUID` citations. v2 revises the emitter to code-primary:
  frontmatter `sourceFiles` + file-path prose links, module pages per component.
  The v1 → v2 cutover is an emitter rewrite + re-authoring of prose; the gates
  are structurally identical (coverage/freshness/consistency), only the citation
  resolution changes (file→intent instead of UUID→intent).
- **The denominators exist and are bounded.** Were loom green, the coverage
  ledger would owe: **21 component module pages**, topical pages citing the
  **hotspots** (`backend-neutral storage boundary` deg 73; `interface surface
  schema vocabulary` deg 68), the **9 `vocab` terms** (glossary), and the
  **50 `kind=decision` notes** (decisions page) — against the **83 leaf
  features** that are *not* individually owed. ~6 topical + 21 module pages cover
  a 105-intent graph; the count is a function of components + topics, not leaves.
- **Live vacuity demonstration.** loom's graph currently enumerates **no
  discharged `user_visible` journey** (`Proven` renders "—"). The *Flows* page
  renders an honest empty-state rather than skipping — the repo-agnostic
  vacuity handling, demonstrated on loom itself.
- **The flat wiki is hard-cut.** v2 **fully replaces** v1 — single source of
  truth, no backward compatibility. The flat `loom.wiki.md` and the graph-primary
  OKF emitter are deleted; the v2 code-primary emitter + manifest resolver
  replace them. loom's only wiki is the v2 bundle. No consumer exists yet
  (dogfood); the binary is reinstalled once v2 is proven.

---

## Risks & mitigations

- **It becomes vibes-with-extra-steps** (the non-negotiable). *Mitigation:* three
  of four gates are mechanical (coverage, provenance freshness, cross-link
  consistency); only prose *quality* is human-judged, and it is explicitly
  user-gated and loud, never machine-green. Coverage and freshness are
  *recomputed*, so dishonesty cannot satisfy them.
- **Broad pages churn** (one page links 30 files → any change re-stales it).
  *Mitigation:* module pages are per-component (bounded blast radius) + the
  Production-ready gate (code is stable when prose is written). Topical pages
  cite only their salient nodes, not every leaf.
- **The graph-aware consistency gate is weakened by code-primary.** *Mitigation:*
  the manifest resolves file→intent→edge at check time, so relational claims in
  prose are still falsified against typed graph edges. This is the gate Qoder
  cannot do; v2 preserves it by making the graph the invisible manifest, not by
  removing it.
- **OKF is v0.1 and will change.** *Mitigation:* it is deliberately minimal and
  markdown-based; pin v0.1, keep the emitter a deterministic projection, treat a
  spec bump as a re-render.
- **Scope creep into a note app** (the author's standing non-goal). *Mitigation:*
  the bundle is strictly downstream — a projection + frontmatter stamp. The graph
  stays the source of truth; nothing authored in the wiki is graph truth until it
  re-enters through the existing intake boundary.
- **Over-granular coverage** (owing a page per feature). *Mitigation:*
  altitude-calibrated salient set (component + hotspot + journey + interface +
  vocab), not leaves — the granularity contract again.

---

## Non-goals

- **New node or edge types.** A wiki page is a markdown file; its `sourceFiles`
  are resolved to intents via existing IMPLEMENTS edges; its freshness stamp
  lives in frontmatter. Projection over existing planes.
- **Backward compatibility with v1.** None. v2 fully replaces v1: the flat
  `loom.wiki.md` is deleted, the graph-primary emitter is deleted, the `--okf`
  flag's v1 semantics are replaced. Single source of truth.
- **Machine-certifying prose quality.** That residue is user-gated, by design.
- **Auto-granting the comprehension axis from LLM judgment.** Earned by coverage
  + freshness + consistency evidence, never assigned.
- **Documenting before Production-ready.** Routing gates it; below the gate it
  reads "— (gated)," not 0%.
- **Surfacing the graph's vocabulary to the reader.** The `intent:UUID` never
  appears in reader-facing prose. The graph is the manifest, not the vocabulary.

---

## Open questions

1. **The cold-reader comprehension proof (v2).** The discriminating-run
   analogue: a fresh LLM given *only* the bundle must answer falsifiable
   questions or reconstruct the intent skeleton. In scope now, or the axis's
   later ceiling?
2. **Display.** How `loom status` renders the comprehension axis compactly next
   to the rung-vector in both human and `--json`.
3. **Relational-claim extraction.** How does loom detect that prose asserts a
   *relationship* between two files (not just cites them independently)? Lean:
   the LLM tags relational sentences with a convention (e.g., a fenced annotation)
   that loom extracts and resolves; untagged prose is coverage-only, not
   consistency-checked for relationships.
4. **Module page slug derivation.** From the component intent's name (slugified),
   or from the directory its files live in? Lean: intent name — it's stable
   across file moves, and the graph owns the naming.

**Resolved during v2 revision:**
- **Citation model:** code-primary (file paths in prose, `sourceFiles` in
  frontmatter). The `intent:UUID` is invisible to the reader. The graph resolves
  file→intent at check time.
- **Structure:** hybrid — bounded topical pages + per-component module pages.
  Bounded by the salient set (components, not leaves); code-shaped (each module
  page is about a real code module).
- **Consistency gate:** graph-aware via manifest resolution. loom resolves
  file→intent→edge, so relational claims are falsified against typed edges —
  the gate Qoder cannot do, preserved.
- **Self-teaching:** loom inits the target repo's graph, then orders authoring.
  The graph is the manifest: invisible to the reader, present for the gates.
- **Freshness:** content-hash provenance over `sourceFiles`, not mtime; per-page
  stamp in frontmatter.
- **The axis:** parallel and routing-gated behind Production-ready, not a sixth
  rung.
- **The artifact:** manifest frontmatter (machine layer, byte-checkable) + prose
  body (human layer, gate-checked), co-located in one file per concept.

**Shipped (2026-06-25, v1):** `loom wiki --okf` emits the v1 graph-primary
skeleton bundle — 7 OKF files with `intent:UUID` citations and byte-`--check`.
The v2 code-primary revision (this document) is the next increment: rewrite the
emitter for `sourceFiles` frontmatter + file-path prose + module pages, then
re-author the prose layer.
