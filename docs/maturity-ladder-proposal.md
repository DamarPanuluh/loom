# Proposal: the maturity ladder — one ordinal "done" replacing loom's scattered badges

**Status:** Proposal (for discussion). Companion to
[`intent-spectrum-proposal.md`](intent-spectrum-proposal.md) and
[`ui-ux-flow-proposal.md`](ui-ux-flow-proposal.md) — joins the same coherent
proposal set.
**Date:** 2026-06-22
**Scope:** **hard-cut** loom's several overlapping completion reads (vertical
spine / `phase=complete` / `fully_proven` / `loom complete`) and replace them
with ONE ordinal *maturity ladder* of five stages, defined as a re-sequencing of
the gates loom already computes — plus the LLM-cognitive spine (per-repo
criteria), a `grill-me` human-completeness gate for Seeded, a "free lane" for
mid-flight interjections, and a re-init **port** for existing loom graphs.

---

## Summary

The concern: loom's notions of "done" feel scattered. A driver sees a vertical
spine milestone, a `phase=complete` full-green, a stronger `fully_proven` badge,
and a separate `loom complete` coverage projection — four reads on **two
parallel axes** (COVERAGE × QUALITY) with no single answer to "how far along am
I?"

**Recommendation: do not add new gates. HARD-CUT the four reads and re-sequence
the gates loom already computes into ONE ordinal ladder** — **Seeded → Realized
→ Proven → Hardened → Production-ready** — ordered by loom's own honesty law,
`RECORD ≠ DISCHARGE`. Seeded is "fully RECORDED"; the four stages above it are
"progressively DISCHARGED." Each stage is **granted only by a mechanical
roll-up of evidence loom already stores** (earned, never assigned); the LLM's
cognitive role — already real as the per-repo `rubric_teaching` — becomes the
spine: it *authors the per-repo criteria* and *proposes* verdicts, loom
*confirms*.

The ladder is read as a **rung-vector** — every rung's true state — with a
**current-focus** (the lowest unmet rung, where work routes); never a single
scalar "stage N", because the axes are genuinely independent (a repo can be
Hardened before it is Proven — loom itself is).

This is a **hard cut, not a layer**: the ladder becomes loom's *sole* completion
vocabulary. It is cheap because the old reads were **derived, never stored** —
pure functions over evidence — so swapping the derivation swaps the model with
no shim. Existing loom repos move over by **re-init port** (loom's standing
migration pattern), and the rescan re-derives the stage. `grill-me` drives the
one gate loom cannot witness (did the human tell us everything?); the existing
inbox/hypothesis/`sync` loop becomes the **free lane** that lets an interjection
re-enter at any stage and re-trigger the scan.

It stays loom's **teach-adapt-over-hardcode** principle: the ladder is a
projection over existing planes, not a new schema — exactly the move
`intent-spectrum-proposal` made for the want→logic→physical register.

---

## Context

loom's first dogfood target is its own repo, so the scattering is felt
first-hand. Right now `loom status` on a finished-looking graph reports four
overlapping reads:

- `vertical ✓` — the binding spine (hierarchy tree + every implemented leaf
  grounded + every CodeFile reached; `vertically_complete`).
- `phase=complete` — full green: spine + horizontal grid explored + quality
  measured + validations passing + zero open smells (the `graph_state` cascade,
  `seed→build→fix→ground→validate→quality→complete`).
- `fully_proven` — a STRONGER badge layered over `phase=complete`: gates
  G0/G1/G4/G6/G7 + `G_INBOX` + export-freshness (`fully_proven_from_state`).
- `loom complete` — a COVERAGE projection over five canonical rubric dimensions
  (entrypoint, boundary, invariant, journey, behavioral; `rubric_teaching`).

Each is individually principled. Together they make the driver cross-reference
four surfaces to answer one question, and they layer rather than unify. The fix
is not fewer gates — every gate earns its keep — but **one ordinal narrative the
gates roll up into, with the old reads cut.**

The trunk is also not loom-specific. It must serve every codebase operation —
**greenfield, brownfield, refactor, port** — so the ladder is defined by the
*state of the intent↔code↔proof triad*, never by the operations that produce it.

---

## The reframe: two parallel axes → one ordinal ladder

Today's model is two axes that must both reach 100%:

| | COVERAGE (`loom complete`) | QUALITY (`fully_proven`) |
|---|---|---|
| **What it asks** | Does the graph CAPTURE everything the code does? | Are the captured things PROVEN well? |
| **Surfaced as** | 5 rubric dimensions | gates G0–G7 + freshness |

A driver cannot tell which axis to advance first, and the axes silently
interleave. The ladder **sequences the same checks** by the `RECORD ≠ DISCHARGE`
law — which loom already enforces inside the cognitive dimensions:

> **Seeded** = everything is RECORDED (enumerated; placeholders allowed).
> **Realized → Production-ready** = placeholders become progressively
> DISCHARGED (realized + proven graph state).

The ladder *is* the RECORD→DISCHARGE axis made ordinal. Nothing is dropped; the
two axes become the *vertical extent* (how high you've climbed) of one trunk.

---

## The hard cut: the rung-vector replaces `phase` + the badges

The old completion vocabulary is **removed**, not kept in parallel:

- `phase=complete`, `fully_proven`, and the standalone `loom complete` / vertical
  milestone reads are **cut as rival "done" concepts.** Their gate *math* is
  reused as stage roll-up inputs (see Mapping); their *surfacing* as independent
  badges is deleted.
- The `graph_state` cascade's WORK-ROUTING survives, but its `phase` string is
  re-expressed as **`(rung-vector, focus, lane)`**: the **rung-vector** shows
  every rung's true state, the **focus** is the lowest unmet rung (where work
  routes), and the **lane** is the micro work-type to climb it
  (`build|fix|validate|quality|discovery`, the existing `next_action`
  computation). `loom status` reports one line — *"Seeded ✓ · Realized ◐46/78 ·
  Proven ✗0/11 · Hardened ✓ · Production-ready ✗ → focus Realized (lane
  validate)"* — instead of four scattered badges.
- **Why the cut is cheap:** the old reads were never persisted state. They are
  functions over evidence (intents, IMPLEMENTS, verdicts, validations). Replace
  the functions and the model changes; the *evidence* is untouched.

**Implementation surface** (the hard cut touches output, not schema):
`status.rs` (badge block → stage line), `complete.rs` (fold into the stage
roll-up), `guide.rs` (`done_condition` → stage gates), `tour.rs` (terminal-state
prose), `stats.rs` (`fully_proven_from_state` + `graph_state` become the
stage-gate roll-up). No node/edge types change.

### The ladder is a rung-vector, not a scalar

Testing the gates against loom's own graph (see Validation) forced this: loom is
**Hardened ✓ while Realized is ◐ and Proven is ✗**. A scalar "you are at stage
N" would hide that the top rung is already met and a middle rung is untouched.
So the ladder is computed as a **vector of per-rung states**; the *current
focus* (what `loom next` routes to) is the **lowest unmet rung**, and satisfied
higher rungs are shown ✓ and skipped by routing. This is how an ordinal ladder
coexists with genuinely independent axes: **ordinal for routing, vector for
truth.** A rung never silently un-completes a higher one; the vector tells all.

---

## The ladder

Each stage names a falsifiable **state** (a participle of the result, never a
verb), a **mechanical gate** built from signals loom already computes, and what
is **NEW**. The climb to any rung differs by mode; the rung does not.

A dimension whose denominator is 0 is **N/A** (rendered "—"), auto-satisfied,
never a 0% block — e.g. loom's `boundary` dimension (no external-service
imports). Vacuity is honest absence, not failure.

### Stage 1 — Seeded · "all idea captured" (RECORD complete)

**Claim:** the complete vision is in loom — every responsibility enumerated,
even if not yet built or proven.

**Mechanical gate (existing signals):**
- Hierarchy well-formed (tree, one parent) — `vertical_completeness`.
- Every comprehensiveness dimension *enumerated* (the `enumerated` half of each
  ledger, placeholders OK): a claiming intent for every public symbol
  (`entrypoint`), every external-service file (`boundary`,
  `boundary_scan_from_disk`), every user-visible flow (`journey`), and a
  designed failure sibling for every happy leaf (`behavioral`).

**NEW — the `grill-me` human-completeness gate.** loom already concedes the one
thing it cannot witness: *"loom measures your SEED, not the full vision … the
badge cannot see what you never seeded."* `grill-me` is the mechanism that
closes it — a relentless interview (one question at a time, each with a
recommended answer, resolving each branch of the decision tree) driven by a
loom-authored, repo-adapted criterion set: *did we ask about personas, error
modes, scale, security surface, data lifecycle, external boundaries?* Seeded
clears when the interview reaches shared understanding; the session is logged as
a `decision` note enumerating the criteria covered, so the close is auditable
(and reopens when a new criterion is added). The seed *flow itself* is the
want→contract→logic→physical ladder of `intent-spectrum-proposal`; `grill-me`
is the completeness pressure on top of it. (Resolved: start with the
decision-note log; add a mechanical answered/declined proxy only if it leaks.)

### Stage 2 — Realized · "implemented and tested programmatically" (DISCHARGE: unit)

**Claim:** every captured leaf exists in code and is proven by a discriminating
programmatic (unit-altitude) test.

**Mechanical gate (existing):**
- Vertical spine complete — `vertically_complete` (every implemented leaf
  grounded; every CodeFile reached).
- **G1**: every realized leaf is EXECUTED-proven, not asserted-only —
  `proven_executed_leaves == realized_leaves`.
- No spec-as-built — `doc_only_realizations` empty.

This is where loom-the-repo is stuck today (**46/78**). Under the ladder the 32
asserted-only leaves are not an "optional stronger badge" — they are an
*unfinished Stage 2*.

### Stage 3 — Proven · "proven from the boundary" (DISCHARGE: interface)

**Claim:** every user-visible journey / public interface is proven by a passing
boundary test; internal leaves inherit that coverage.

**Mechanical gate (existing):**
- Journey ledger discharged — `journey_ledger_from_snapshot` (every
  `user_visible` leaf has a boundary proof that RAN passing/discriminating;
  `saga add` alone is owed, not discharged).
- Every `--boundary` intent proven by a proof entering at the interface.

**Discharge = a passing DISCRIMINATING boundary proof — automated OR human.**
An automated `saga` for what machines can drive; a human `manual_check`
(`inspected_by=human`) for the genuinely visual/interactive residue (per
`ui-ux-flow-proposal`). What is NOT allowed is a bare waiver / "trust me" note:
that re-admits the asserted-only weakness Proven exists to kill. (Behavioral may
be waived by a falsifiable `reopen-when:` note because it is a DESIGN-
completeness dimension; Proven is a PROOF dimension, so it demands an actual
verification — human or machine.)

Proven is a **boundary-altitude** gate, never per-leaf: demanding an e2e test
for a private helper is absurd, so internal leaves are discharged transitively
by a journey/public-API proof that exercises them.

### Stage 4 — Hardened · "quality, robustness, performance, security" (DISCHARGE: quality)

**Claim:** the proven code is also *good* — measured, related, robust, clean.

**Mechanical gate (existing):**
- Invariant dimension — every coded intent measured under ≥1 GOVERNS rule
  (`measured_pairs`).
- Horizontal grid explored — `horizontally_explored` (duplication, coupling,
  layering now detectable).
- Behavioral dimension discharged — `behavioral_ledger_from_snapshot` (each
  happy leaf's failure sibling is *realized + proven*, not planned).
- Zero open smells — `smell_report.open == 0` (godfiles, oversized functions,
  clones, layering violations, `nonlocal_proof`).
- Risk-surface rule packs seeded for THIS repo (`iso5055` baseline +
  `loom detect` recommendations: `service|data|concurrency|web-ui|mobile`, and
  security/perf packs where they apply).

(Resolved: behavioral is *split* — enumerate@Seeded, realize the sibling
@Realized, gate "all covered"@Hardened — reusing the existing ledger.)

### Stage 5 — Production-ready · "shippable" (DISCHARGE complete + durable + deploy-fit)

**Claim:** proven, comprehensive, durable, and fit to deploy.

**Mechanical gate:**
- All Stage 1–4 gates (the former `fully_proven` set G0/G1/G4/G6/G7 + `G_INBOX`,
  now expressed as the lower rungs) + export fresh.
- `loom complete` dimensions all DISCHARGED (not merely enumerated).
- Durable — wiki fresh (`wiki --check`), disk reconciled, **inbox drained**
  (untriaged AND triaged routed/resolved).

**NEW — the deploy-fitness dimension, repo-authored.** loom deliberately does
not claim deploy-fitness today (*"can't witness security / perf / ops"*). The
ladder keeps that honesty by making it a **sixth, repo-instantiated rubric
dimension** with a canonical skeleton (CI / release / observability) the LLM
fills per repo: CI green = a passing validation; a cut release = a checkable
artifact; a runbook/observability intent grounded + proven. loom never guesses
deploy-fitness; it checks LLM-authored, falsifiable, repo-adapted criteria — the
same contract as every other rubric dimension.

**Stage granularity (resolved): five rungs, adaptive collapse.** Proven and
Hardened render as one "Verified" rung **iff `journey.enumerated == 0`** —
purely "are there user-visible journeys to prove?", NOT a library/CLI
heuristic. loom disproves the heuristic: it is a CLI yet enumerates 11
journeys, so it does NOT collapse. The visible ladder is *derived* (five rungs
when journeys exist, four when none), never declared.

---

## Mapping: what is CUT vs REUSED as a stage-gate input

The old reads are removed as concepts; their underlying math is reused.

| Old concept | Status | Reused by |
|---|---|---|
| `seed` phase; comprehensiveness *enumeration* | reused | **Seeded** gate |
| `vertically_complete`; `build`/`fix`/`ground` phases | reused | **Realized** gate / lane |
| `validate` phase; **G1** executed-proof | reused | **Realized** gate |
| journey dimension (sagas); boundary proofs | reused | **Proven** gate |
| `quality` phase; invariant (measured); `horizontally_explored` | reused | **Hardened** gate / lane |
| behavioral dimension; open-smells audit (incl. G4) | reused | **Hardened** gate |
| `phase` STRING as a "done" read | **CUT** | replaced by `(stage, lane)` |
| `fully_proven` BADGE | **CUT** | becomes the Stage 1–5 roll-up |
| standalone `loom complete` PROJECTION | **CUT** | folded into stage gates |
| export/wiki freshness; disk reconcile | reused | **Production-ready** gate |
| *(deploy-fitness: security / perf / ops)* | NEW | **Production-ready** (repo-adapted) |

---

## The free lane: interjections re-enter at any stage

Real work is not monotonic — the human changes their mind mid-climb. The ladder
needs an orthogonal lane that injects at any rung, and loom already owns the
primitives:

- **inbox** (untriaged → triaged → deferred) — where an interjection lands.
- **hypothesis** (pre-decision plane) — an interjection that *proposes a change*,
  proven before adoption.
- **`loom sync` staling** — the re-trigger: an interjection that alters an intent
  stales the affected leaves, which **drop their rung**, re-opening exactly the
  work the change invalidated.

Two invariants keep the free lane honest:
1. **Loud, never silent.** An interjection creates *visible debt* (an untriaged
   item blocks Production-ready via `G_INBOX`); it never quietly lowers a badge.
2. **Local.** Staling and re-scan touch only the affected subtree, so an
   interjection costs work proportional to its blast radius, not a full reset.

---

## The scan: `init` proposes, roll-up grants

The LLM's cognitive role is the spine — but it **proposes**, it does not
**certify** (this is the non-negotiable that keeps loom from becoming
vibes-with-extra-steps; we are blocked on `fully_proven` *today* precisely
because an asserted "looks proven" is not trusted, only a discriminating run).

- **`loom init` runs the scan (brownfield):** an LLM-powered `detect++` that
  *proposes* the seed (intents, groundings), candidate verdicts, and a
  *suspected* stage. Every proposal then clears loom's existing evidence bar to
  count. Greenfield starts empty at Seeded-0 and climbs via `grill-me`.
- **Interjection re-runs the scan incrementally** over the `sync`-staled subtree
  only, and recomputes the stage.

The stage is always a **pure function of durable evidence**. Self-consistency
check the design must satisfy: the detector that grants "Proven" must itself be
*discriminating*, not asserted — the model obeys its own standard.

---

## Migration: existing loom repos re-init (a self-port)

The hard cut means there is **no dual-mode and no in-place upgrade** — which is
already loom's stance (`loom migrate`: *"There is no in-place upgrade step …
rebuilt by re-export, then `loom init . && loom import`"*). A repo already on the
old discipline moves over by **re-init port**, the existing pattern:

1. `loom export` from the old binary → `loom.graph.json` (the travel artifact
   carrying ALL evidence: intents, hierarchy, codefiles, IMPLEMENTS, RELATES_TO
   / GOVERNS verdicts, validations, notes).
2. `loom init .` with the ladder binary (fresh store, new schema).
3. `loom import loom.graph.json` — restores the evidence; the new-schema
   normalization simply **does not re-create the old derived badges** (there was
   nothing to migrate — they were never stored).
4. **Rescan** re-derives the ladder stage from the imported evidence, and
   re-grills for any *new* seed-criteria the ladder demands (e.g. the
   deploy-fitness dimension) — surfacing the gap as fresh, loud debt.

This is the **`port` mode applied to loom's own version** (old-discipline →
new): evidence ports losslessly; the stage is recomputed; new gates appear as
honest new debt rather than silent regressions. loom-the-repo itself migrates
this way when the ladder ships — re-init-importing its own committed graph, then
reading back **Realized 46/78**.

**Empirically validated (2026-06-22).** A dry-run of the substrate —
committed `loom.graph.json` → `loom init` → `loom import` into a *scratch* graph
(never the live one) — reproduced loom's evidence exactly: 102 intents · 116
codefiles · 123 validations · 5732 edges (2175 passing · 3532 independent · 25
stale) and the full 360° vector including `exec 46 · assert 32`. The round-trip
is lossless, so the only NEW work a real port adds is the rescan's re-derivation
of the stage + any new-dimension grilling — the evidence itself ports for free.

Open mechanic: whether step 4's rescan is folded into `loom init` (auto-detect an
old-version `loom.graph.json` on import) or stays an explicit follow-up; lean
auto-detect, mirroring how `loom migrate` already inspects the stamped version.

---

## Rearranging the self-teaching

The ladder becomes the spine of `loom guide`. Today guide teaches by *role*
(builder/analyzer/fixer/validator/quality) and lists three milestones in
`done_condition`. Under the ladder:

- `done_condition` collapses to "**you are at Stage N; here is the gate to
  N+1**."
- The role-lanes become the **lane** half of `(stage, lane)` — *how to climb a
  stage* (Realized ← builder+validator; Proven ← validator/saga; Hardened ←
  analyzer+quality).
- The greenfield/brownfield/refactor/port **modes become the *shape of the
  rung-gap***, inferred from graph state rather than declared for the repo (loom
  already routes the weakest axis in the cascade — this names why).

---

## Validation: the ladder run against loom's own graph (2026-06-22)

Computed read-only from loom's live evidence (`loom complete --json`; the graph
was not mutated). The rung-vector:

> **Seeded ✓ · Realized ◐ 46/78 · Proven ✗ 0/11 · Hardened ✓ ·
> Production-ready ✗ → focus: Realized (lane validate)**

- **Seeded** ✓ — 102 intents; entrypoint 856/856, invariant 2112/2112,
  behavioral 8/8, journey enumerated 11, boundary N/A (0/0). Everything
  RECORDED.
- **Realized** ◐ — **46/78** executed-proven (32 asserted-only) **and** 4
  doc-only realizations (specs marked built). Two distinct Stage-2 gaps, both
  surfaced. **Current focus.**
- **Proven** ✗ — **0/11**. loom has NEVER run a passing journey proof. The old
  `phase=complete` never required one, so this real gap was invisible; the
  ladder makes it a wall.
- **Hardened** ✓ — invariant 2112/2112, explored ✓, behavioral 8/8, 0 open
  smells (14 non-blocking advisories). **Met out of order** — the empirical
  proof that the axes are independent and the ladder must be a vector.
- **Production-ready** ✗ — blocked by every lower gap + the non-empty inbox.

**What the test forced into the design:** the rung-vector (Hardened-before-
Realized); the collapse trigger (`journey.enumerated==0`, since loom is a CLI
*with* journeys); doc-only as a load-bearing Realized gate (4 here); seeding
proposals as `planned` not `implemented` (3 of those 4 are sibling proposals —
a cautionary example); vacuous-dimension handling (boundary "—").

**Two honesty notes.** (1) `modeled_depth` is 2% (59/1996 symbols directly
owned) — informational, NOT a gate: loom models *responsibilities*, and
`entrypoint` (856/856) is the honest seed denominator; gating on total symbols
would force over-modeling. (2) Writing this proposal added 1 unmapped file,
dropping `loom complete` to `phase=audit` — a **live free-lane demo**: an
interjection created loud, visible debt while the stored graph stayed "as is"
(a read-time disk-scan signal, not a mutation).

The payoff: four scattered badges become one honest sentence — **"loom is at
Realized, 46/78; Proven is 0/11 above it."** That second clause is what the old
model hid.

**Any-repo cross-check (the model is not loom-specific).** The run above is the
*brownfield-with-journeys* shape. The other three were checked the same day:

- **Port / re-init** — committed JSON → `init` → `import` into a scratch graph
  reproduced loom's evidence exactly (102/116/123, 5732 edges same state
  breakdown, `exec 46 · assert 32`). Lossless; see Migration.
- **Greenfield** — `loom init` on an empty dir reads the ladder's bottom rung
  honestly: `phase=seed`, vertical ✗, every dimension "—", with the *"empty
  repo → a vision prompt"* guidance (Seeded-0, the `grill-me` entry). Vacuous
  "—" everywhere — not a despairing 0%.
- **Pure library (no journeys)** — `journey.enumerated == 0` fires the adaptive
  collapse to four rungs (Seeded → Realized → Verified → Production-ready); the
  public API is the boundary, proven by the same discriminating tests, so Proven
  folding into Hardened is correct, not a loophole.

---

## Risks & mitigations

- **An ordinal ladder hides the parallel-axis flexibility** (a repo can be
  Hardened before Proven — loom IS). *Mitigation (now empirically forced):* the
  ladder is a **rung-vector**, not a scalar — `loom status` shows every rung's
  true state, routing only *focuses* the lowest unmet one, and smells jump the
  queue regardless of stage. Ordinal for routing, vector for truth.
- **The hard cut breaks tooling/tests that read `phase` / `fully_proven`.**
  *Mitigation:* it is an output/derivation change, not a schema change; callers
  migrate to `(stage, lane)` in one cutover, and existing graphs re-init (no
  shim, no parallel vocabulary). loom's own fixture tests move with it.
- **Seeded cannot be fully mechanical** (no gate proves the human told us
  everything). *Mitigation:* `grill-me` is the best-effort closure; its output
  (intents) is checkable and its completion is an auditable decision note; the
  existing self-check stays loud.
- **The LLM scan could over-claim a stage.** *Mitigation:* the scan only
  *proposes*; the stage is granted by mechanical roll-up of evidence that has
  cleared loom's bar. Earned, not assigned.
- **Re-scan on every interjection is expensive.** *Mitigation:* incremental —
  only the `sync`-staled subtree.
- **Deploy-fitness is unwitnessable by loom.** *Mitigation:* it is a
  repo-AUTHORED criteria dimension, checked by discharge of falsifiable criteria
  (a passing CI validation, a release artifact), never by loom guessing.

---

## Non-goals

- New node or edge types. The ladder is a projection over existing planes (cf.
  `intent-spectrum-proposal`'s "no intent sub-type").
- **A parallel old/new vocabulary or an in-place upgrade.** The cut is hard;
  existing graphs re-init (port). No `phase`/`fully_proven` survives as a rival
  "done" read.
- Auto-GRANTING a stage from LLM judgment. State is earned by evidence.
- Hard-locking stage order. Routing is soft; findings jump the queue.

---

## Open questions

1. **Re-init ergonomics** — auto-detect an old-version graph inside `loom init`
   /`import` and trigger the rescan, or keep rescan an explicit step? (Lean
   auto-detect.)
2. **Deploy-fitness skeleton** — fix the canonical sub-dimensions (CI / release /
   observability), or leave them fully repo-defined with no skeleton?
3. **Lane vocabulary after the cut** — keep the five lane names
   (`build|fix|validate|quality|discovery`) verbatim, or rename any to read
   naturally as "how to climb stage N"?
4. **Stage display ergonomics** — how loom renders the rung-vector compactly in
   both human and `--json` status without losing the per-rung reasons.

Resolved during validation: adaptive-collapse trigger is `journey.enumerated
== 0` (not a library/CLI heuristic); design proposals (including this one) seed
as `--lifecycle planned --source docs/…`, never `implemented`, so they do not
become doc-only realizations.
