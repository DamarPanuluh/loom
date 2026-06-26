# Proposal: the repo-wiki ladder — an LLM-authored OKF wiki as loom's comprehension axis

**Status:** Proposal (for discussion). Companion to
[`maturity-ladder-proposal.md`](maturity-ladder-proposal.md),
[`intent-spectrum-proposal.md`](intent-spectrum-proposal.md), and
[`ui-ux-flow-proposal.md`](ui-ux-flow-proposal.md) — joins the same coherent
proposal set.
**Date:** 2026-06-25
**Scope:** add a second, orthogonal "done" axis — **comprehension** — that an LLM
discharges by authoring a human-facing repo wiki in the Open Knowledge Format
(OKF v0.1), gated to begin only when the maturity ladder is green. Keep the
existing deterministic `loom wiki` as the wiki's load-bearing **skeleton**; the
LLM fills the **prose**. Make legibility falsifiable the way loom made proof
falsifiable: the LLM *proposes*, loom *certifies* by mechanical roll-up —
coverage, provenance freshness, and cross-link↔edge consistency — never by
trusting prose. Adds **no node or edge types**.

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
`loom next --mode wiki`, and discharged by an **OKF bundle** the LLM authors.

The bundle is **two layers of one artifact, not a rival wiki**: the existing
deterministic `loom wiki` becomes the **skeleton** (the `index.md`, each
concept's frontmatter, the graph-derived facts, and the cross-links — still
byte-`--check`able); the LLM fills the **prose body** (provenance-stamped, not
byte-checked). This directly honors "don't replace the generated wiki" — the
generated part becomes the frame the prose hangs on.

The whole risk is that an LLM-written narrative collapses into
vibes-with-extra-steps — the exact failure `maturity-ladder-proposal` calls the
non-negotiable. It does not, because three of the four gates are mechanical:
**coverage** (every salient graph node is *cited* by some page), **freshness**
(every section's provenance stamp matches the current hash of the nodes/files it
links — `loom sync` applied to a wiki section), and **consistency** (every
cross-link resolves to a real graph edge; a link asserting a relationship the
graph denies is a *fabricated-relationship* finding). Only prose *quality* stays
human-judged, and it lands in a **user-gated queue** exactly like the
visual-confirm residue in `ui-ux-flow-proposal` — loud, never machine-green.

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

Two pieces of groundwork already exist for it:

- **The `links` field is OKF-ready by design.** `types.rs:1043` and
  `COMMANDS.md:540` both say the knowledge-card backlinks (`intent:`, `file:`,
  `validation:`, `hypothesis:`, `rule:`, `vocab:`, `inbox:`) exist to make
  "future wiki/OKF projection possible without turning loom into a note app."
- **OKF is a real, minimal standard.** Google Cloud's Open Knowledge Format v0.1
  (2026-06-12) represents knowledge as a *directory of markdown concept files*,
  each with YAML frontmatter (only required field: `type`) and a prose body,
  cross-linked by plain markdown links into a graph richer than the filesystem
  tree, with reserved `index.md` (progressive disclosure) and `log.md` (history).
  That is loom's data model rendered to disk: intents are concepts, edges are the
  cross-links. The whole spec "fits on a single page."

---

## The reframe: comprehension is a parallel axis, gated — not a rung above

`maturity-ladder-proposal` already established that "done" is a **rung-vector,
not a scalar," because the axes are genuinely independent (loom is *Hardened
before Proven*): "ordinal for routing, vector for truth." Comprehension is
simply another independent entry in that vector:

| | maturity (code) axis | comprehension (legibility) axis |
|---|---|---|
| **Asks** | does the artifact *behave* as specified? | can a human *rebuild the mental model* from the wiki alone? |
| **Discharge** | a discriminating test/saga that RAN | an OKF page that COVERS a node, is FRESH, and is CONSISTENT with the graph |
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
   empty; an interjection re-stales only the touched section (the free-lane
   "local, proportional to blast radius" invariant).

So the vector reads, e.g., `Code: Production-ready ✓ · Documented ◐ 7/12`. Below
Production-ready it reads `Documented — (gated)`: honest, not a 0% block.

---

## The structure: topical axes, bounded — not one doc per node

The bundle's structure is the **canonical topics a human wiki has**, *not* a
mirror of the intent tree. This is the decision that makes it scale to any repo:
**it decouples doc-count from node-count.** One doc per component would put 21
files (and counting) on loom, and an arbitrary number on a large repo — an
unbounded churn surface. A fixed set of ~a dozen topical pages is bounded and
repo-agnostic whether the graph has 12 intents or 1200.

Each canonical axis is **backed by a loom plane**, which is what keeps its
coverage checkable rather than arbitrary:

| Wiki axis (what a repo wiki usually has) | loom plane it projects / is checked against |
|---|---|
| **Overview / purpose** | `system` + `component` intents |
| **Getting started / build & run** | build/test validations, entrypoints, `loom detect` stack |
| **Architecture** | hierarchy + RELATES_TO + `loom layer order` + hotspots/centrality |
| **Key concepts / glossary** | the bounded `loom vocab` registry (near-direct projection) |
| **Data / domain model** | persistence-layer intents + schema (vacuous → "—") |
| **Flows / journeys** | sagas + `user_visible` intents (the narrative spine) |
| **Module / directory guide** | codefiles grouped by owning intent |
| **Extending / contributing** | the lane discipline; how to add an intent/feature |
| **Design decisions / "why"** | the `kind=decision` notes (reason + history already stored) |
| **Gotchas / failure modes** | `sad`/`fallback`/`edge_case` aspects + open smells |

Glossary ← vocab, rationale ← decision notes, flows ← sagas, architecture ←
layer order + hotspots: these are not invented chapters, they are projections of
planes loom already maintains. A repo with no journeys renders the *Flows* axis
"—" (auto-satisfied), exactly as the maturity ladder renders a vacuous
`boundary` dimension — that is what makes the axis set repo-agnostic.

**Markdown links to codefiles and intents are both the navigation and the
grounding.** A sentence in *Architecture* — "the saga runner
([`src/saga/runner.rs`](../src/saga/runner.rs), intent `saga runner halt-on-failure
semantics`) halts at the first failure" — gives the reader a jump-to-source *and*
gives loom a falsifiable anchor. This is OKF-native (OKF *is* a markdown
cross-link graph) and it is the hook every gate below hangs on.

---

## Coverage flips: every node is CITED, not mirrored

Because structure is topical, coverage cannot be "every component has a doc." It
becomes **"every salient node is cited by ≥1 page"** — graph-driven completeness
over human-shaped structure. The RECORD ledger (identical in shape to the
existing `journey`/`behavioral` ledgers in `comprehensiveness.rs`) is the set of
**uncited salient nodes**:

- every `component` intent,
- every node in `loom hotspots` (high centrality / tangled files — the things a
  newcomer *must* be told about),
- every discharged `user_visible` journey (owes a walkthrough),
- every `InterfaceSurface` (owes an entry in a reference/flows page),
- every `loom vocab` term (owes a glossary entry).

An uncited salient node is an *unexplained responsibility* — a `--mode wiki`
queue item. Adding a new component later does not add a *file*; it adds a
*citation obligation* to an existing page. Bounded structure, graph-backed
completeness.

Granularity is altitude-calibrated, like intent granularity itself: **system +
component + hotspots + journeys + vocab are cited; the 83 leaf features are
not** individually owed a mention — they are reachable through their component's
page and the deterministic skeleton. (Open question: exact salient set.)

---

## Falsifiability: how legibility is certified, not trusted

This is the crux. The current wiki is checkable because it is deterministic;
LLM prose is neither deterministic nor byte-comparable, so `--check` cannot be a
byte diff. The maturity-ladder non-negotiable applies in full: *the LLM
proposes, loom certifies by mechanical roll-up; never auto-grant from LLM
judgment.* The comprehension axis is granted by four checks, three mechanical:

### 1. Coverage — mechanical

The RECORD ledger above: zero uncited salient nodes. Pure graph + markdown-link
extraction.

### 2. Freshness — provenance stamp, not bytes, and emphatically not mtime

Each page (ideally each OKF section, via conventional headings) declares the
nodes/files it explains — *which are exactly its cross-links* — and is stamped at
write time with their **content provenance**: the content-hash of each linked
codefile (the same per-file hash `loom sync` already maintains — `loom status`
reports "34 drifted (content changed since last sync)") and the
`structure_version` / description-hash of each linked intent (`structure_version`
already bumps on node/edge/lifecycle/layer change and `loom intent update`
ripples a description change one hop). The stamp lives in the page's committed
frontmatter, so it **travels with the repo and diffs in PRs**.

Freshness = recompute and compare. Match → fresh; mismatch → stale → the section
re-enters `--mode wiki`. This is **`loom sync` treating a wiki section as a
grounded node**: the same staling that flips an IMPLEMENTS edge when its file's
content changes flips a wiki section when a node it explains changes. Per-section
stamps mean a change to `src/saga/runner.rs` re-opens *Architecture* at its saga
paragraph, not the whole page — staleness proportional to blast radius.

> **Rejected: mtime ("doc newer than the code it reads ⇒ fresh").** The instinct
> — infer freshness from the doc↔code relationship instead of re-reading — is
> exactly right, and it is what makes this cheap. But mtime is the wrong carrier:
> it does not survive `git clone` / `checkout` / CI checkout / archive extract
> (checkout rewrites every mtime in write order, so after a clone "doc newer than
> code" is noise — fatal for a committed, PR-diffed artifact); it conflates a
> `touch` or a formatter run with a semantic change (constant false churn — the
> reason loom's own `sync` uses mtime-delta only as a *trigger*, then re-checks
> content); and it tracks write-order, not causality (a rebase or restored older
> doc can be "newer" than code it is stale against). Content-hash provenance
> keeps the instinct and answers the real question — *has the code changed since
> this was written about it?* — durably.

This is also the **honesty forcing-function** the design needs. The LLM cannot
claim a page covers `src/saga/runner.rs` without linking it; that link is then
(a) consistency-checked against the real graph and (b) hash-stamped so a later
change re-opens the page. Honesty is not requested and trusted — dishonesty
*cannot satisfy the gate*, because coverage and freshness are recomputed.

### 3. Consistency — falsify the prose against the graph (the "write-gap" check)

Every cross-link is checked against the graph:

- a link `A → B` with **no backing graph edge / import** → *fabricated
  relationship* (a hallucinated connection, caught by the graph);
- a link to a **retired or nonexistent** intent/file → *dangling reference*;
- a high-centrality edge or hotspot **mentioned by no page** → *unexplained
  relationship* (a coverage gap, surfaced here too);
- a `vocab` term defined but **used in no page** → *dangling concept*.

This is the user's "check gaps with writing, not code," made discriminating
rather than vibes: it is literally `loom edge explore … issue` applied to a prose
claim. The graph can prove the wiki wrong.

### 4. Prose quality — user-gated, loud, never machine-green

Whether the writing is *clear and well-organized* is the one thing no machine can
certify — so the design does not pretend to. It surfaces as a `manual_check` with
`inspected_by=human`, batched into the existing **user-gated lane** (the same
queue `ui-ux-flow-proposal` uses for visual residue, prioritized by user
presence alongside align / rulings / blocked). Honestly human-judged, loud, and
explicitly outside the mechanical green.

**The gate, in one line:** `Documented` is granted when coverage = 0 owed,
freshness = 0 stale sections, consistency = 0 fabricated/dangling links, and the
prose-quality queue is drained or consciously accepted.

---

## The artifact: two layers of one OKF bundle

`loom wiki` does not get replaced; it gets **restructured into the OKF bundle's
skeleton** and grows a sibling prose layer.

```
loom.wiki/                      ← OKF bundle (directory, byte-`--check`'d per file)
  index.md                      ← skeleton: overview + reading order + axis links
  architecture.md               ← the intent hierarchy (same bytes as the flat section)
  components.md                 ← intents by domain, grounded in files
  quality.md                    ← the rule corpus by category (empty-aware)
  glossary.md                   ← the `loom vocab` registry (terms × intents tagged)
  decisions.md                  ← `kind=decision` notes with rationale, newest first
  flows.md                      ← user_visible intents + their saga proofs (vacuous-safe)
  getting-started.md            ← (stub — not yet emitted)
  log.md                        ← OKF history (optional, reserved)
```

The v1 emitter (`loom wiki --okf`, shipped alongside this proposal) emits 7 files
(index + architecture + components + quality + glossary + decisions + flows),
each with OKF v0.1 frontmatter (`type`, `title`, `tags`) + a graph-derived body.
Getting-started and the optional `log.md` are documented stubs; they are not
fake-emitted. The index links every live axis in a reading-order list.

- **Skeleton (deterministic, loom-owned).** `index.md`, every concept's
  frontmatter (`type`, the grounded-node list, the provenance stamp), the
  graph-derived fact tables, and the cross-links. Same graph → identical bytes,
  so this layer keeps the existing byte-`--check`. This *is* today's `render_wiki`,
  emitting a directory instead of one file.
- **Prose (provenance-stamped, LLM-owned).** The body under each concept's
  headings. Checked by coverage + freshness + consistency, never by bytes.

They are not two wikis competing for "source of truth" — they are two layers of
one bundle: loom guarantees the frame and the cross-link integrity; the LLM hangs
narrative on the frame. The graph remains the single source of truth; the bundle
is downstream of it. This is the literal meaning of "don't fully replace the
generated wiki — it's good to track if the codebase has finished."

---

## The lane: `loom next --mode wiki`

A new work-type in the existing `(rung-vector, focus, lane)` decomposition. It
drains the comprehension queue, one item with full context per call, like every
other lane:

- **write-gap** — an uncited salient node: "explain `<component>` (degree N,
  grounded in `<files>`); cite it in `architecture.md` or `<page>`."
- **stale-section** — a section whose provenance stamp broke: "`<file>` changed;
  the *saga* paragraph in `architecture.md` may be wrong — re-read and rewrite."
- **fabricated-link** — a cross-link the graph denies: "`architecture.md` claims
  `A → B`; no edge/import exists. Fix the prose or record the edge."

Routing surfaces it only at Production-ready (the gate above); below that, the
lane reports `gated`. The free lane (`inbox` / `hypothesis` / `sync`) re-enters
it incrementally — a post-Production-ready interjection that alters an intent
stales only the sections that linked it.

---

## OKF v0.1 conformance

- **Concepts are markdown files in a directory.** Each canonical axis page (and
  each per-journey flow page) is one concept.
- **Frontmatter.** Required `type` (e.g. `type: architecture | guide | glossary |
  flow | decision`); optional `title`, `description`, `tags`, `timestamp`. loom
  adds two non-reserved fields it owns: the grounded-node list and the provenance
  stamp.
- **Cross-links are plain markdown links** to other concepts, to `src/…` files,
  and (rendered) to intents — the `links` groundwork in `types.rs` projects
  straight into these.
- **Reserved files:** `index.md` (progressive disclosure — the reading order a
  newcomer follows) and optional `log.md` (chronological change history).

Pin v0.1 (minimal, markdown-based; a spec bump is a re-render of the skeleton).
The emitter stays under the same deterministic-projection discipline as
`loom export` / `loom wiki`.

---

## Mapping: what is REUSED vs NEW

| Concept | Status | Note |
|---|---|---|
| `loom wiki` deterministic projection | **reused** | becomes the bundle **skeleton** (directory `index.md` + frontmatter + facts + cross-links) |
| `loom wiki --check` byte freshness | **reused** | guards the skeleton layer only |
| per-file content hash; `loom sync` staling | **reused** | the prose **freshness** mechanism (section ← grounded node) |
| `structure_version`; `intent update` ripple | **reused** | freshness for linked *intents* |
| `loom vocab`, `kind=decision` notes, sagas, `loom layer order`, `loom hotspots` | **reused** | the per-axis content sources + coverage denominators (glossary, decisions, flows pages now shipped in v1 emitter) |
| `types.rs` `links` backlinks | **reused** | project into OKF cross-links |
| user-gated `manual_check` queue (`ui-ux-flow`) | **reused** | prose-quality residue |
| comprehension axis in the rung-vector | **NEW** | a vector entry, routing-gated behind Production-ready |
| `loom next --mode wiki` lane | **NEW** | drains write-gap / stale-section / fabricated-link |
## Validation: the axis read against loom's own graph (2026-06-25, updated)

Computed read-only from loom's live graph (`loom status`, `loom vocab list`,
`loom hotspots`, `loom note list --kind decision`; the graph was not mutated).

- **The gate correctly says "not yet."** loom's live rung-vector is
  `Seeded ✓ · Realized ✓ 81/81 · Proven — · Hardened ✗ · Production-ready ✗`,
  focus **Hardened** (56 rule×intent pairs unmeasured, 104 unexplored RELATES_TO
  pairs). loom is not Production-ready, so the comprehension axis reads
  `Documented — (gated)`. The model obeys its own ordering: you cannot write the
  cognitive wiki for a repo whose code axis is still red.
- **The v1 emitter is shipped.** `loom wiki --okf` now emits a 7-file bundle
  (index + architecture + components + quality + glossary + decisions + flows),
  each with OKF v0.1 frontmatter and a graph-derived body. `--check` byte-compares
  every file. The skeleton layer is complete; the prose layer (LLM-authored
  narrative) is the next increment.
- **The denominators exist and are bounded.** Were loom green, the coverage
  ledger would owe pages citing: **21 component intents**, the **hotspots**
  (`backend-neutral storage boundary` deg 73; `interface surface schema
  vocabulary` deg 68; `snapshot analysis and annotation helpers` deg 65 —
  component; tangled `src/commands/guide.rs`, 9 intents), the **9 `vocab`
  terms** (glossary — now live), and the **50 `kind=decision` notes** (rationale
  page — now live) — against the **83 leaf features** that are *not* individually
  owed. ~7 topical pages cover a 105-intent graph; the count is a function of
  topics, not nodes.
- **Live vacuity demonstration.** loom's graph currently enumerates **no
  discharged `user_visible` journey** (`Proven` renders "—"). The *Flows* page
  renders an honest empty-state message ("no saga registered yet" for each
  user_visible intent) rather than skipping the page — the repo-agnostic vacuity
  handling, demonstrated on loom itself rather than asserted.
- **The flat wiki is unchanged.** `loom.wiki.md` (77 KB) still works as before;
  `loom wiki --check` passes. The OKF bundle is additive, not a replacement.

The payoff mirrors the maturity ladder's: a second honest sentence next to the
first. Not just *"loom is at Hardened, 56 pairs unmeasured,"* but eventually
*"…and once green, the wiki owes 7 pages citing 21 components, 9 terms, 50
decisions — 0 written."* The legibility gap stops being invisible.
  directory is mechanical and changes no graph state.

The payoff mirrors the maturity ladder's: a second honest sentence next to the
first. Not just *"loom is at Hardened, 56 pairs unmeasured,"* but eventually
*"…and once green, the wiki owes 12 pages citing 21 components, 9 terms, 50
decisions — 0 written."* The legibility gap stops being invisible.

---

## Risks & mitigations

- **It becomes vibes-with-extra-steps** (the non-negotiable). *Mitigation:* three
  of four gates are mechanical (coverage, provenance freshness, cross-link
  consistency); only prose *quality* is human-judged, and it is explicitly
  user-gated and loud, never machine-green. Coverage and freshness are
  *recomputed*, so dishonesty cannot satisfy them.
- **Broad pages churn** (one page links 30 files → any change re-stales it).
  *Mitigation:* per-section provenance stamps (staleness proportional to blast
  radius) + the Production-ready gate (code is stable when prose is written).
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

- **New node or edge types.** A wiki concept is a markdown CodeFile; its links
  are extracted and checked against the graph; its freshness stamp lives in
  frontmatter. Projection over existing planes (cf. `maturity-ladder-proposal`).
- **Replacing `loom wiki`.** It becomes the skeleton layer and keeps its byte
  `--check`.
- **Machine-certifying prose quality.** That residue is user-gated, by design.
- **Auto-granting the comprehension axis from LLM judgment.** Earned by coverage
  + freshness + consistency evidence, never assigned.
- **Documenting before Production-ready.** Routing gates it; below the gate it
  reads "— (gated)," not 0%.

---

## Open questions

1. **The salient-node set.** Exactly which nodes owe a citation — component +
   hotspot + journey + interface + vocab as proposed, or a tunable centrality
   threshold for which features also get named?
2. **Stamp granularity.** Per-page or per-OKF-section provenance? (Lean
   per-section — finer staleness, but needs stable section anchors.)
3. **Skeleton/prose co-location.** One concept file = frontmatter skeleton +
   prose body (they diff together), vs. skeleton emitted separately and prose
   merged? (Lean co-located: one OKF file per concept, loom owns frontmatter +
   fact blocks, LLM owns bodies.)
4. **The cold-reader comprehension proof (v2).** The discriminating-run analogue:
   a fresh LLM given *only* the bundle must answer falsifiable questions or
   reconstruct the intent skeleton. In scope now, or the axis's later ceiling?
5. **Lane name.** `wiki` / `document` / `legible`?
6. **Display.** How `loom status` renders the comprehension axis compactly next
   to the rung-vector in both human and `--json`.

Resolved during drafting: freshness is **content-hash provenance**, not mtime
(travel/causality); structure is **topical axes with graph-driven citation
coverage**, not one-doc-per-node (bounded, repo-agnostic); the axis is **parallel
and routing-gated**, not a sixth rung; the artifact is **two layers of one OKF
bundle**, keeping `loom wiki` as the skeleton.

**Shipped (2026-06-25, same day as proposal):** `loom wiki --okf` now emits the
v1 skeleton bundle — 7 OKF files (index + architecture + components + quality +
glossary + decisions + flows), each with OKF v0.1 frontmatter and a
graph-derived body, `--check` byte-comparing every file. The Getting-started
page remains a stub (documented, not fake-emitted). The prose layer (LLM-authored
narrative on the frame) is the next increment.
