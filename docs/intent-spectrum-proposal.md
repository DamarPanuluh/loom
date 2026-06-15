# Proposal: handling the intent spectrum (want → logic → physical) without sub-typing Intent

**Status:** Proposal (for discussion)
**Date:** 2026-06-15
**Scope:** how loom represents the full spectrum from "what the developer wants"
→ "what logic must exist to fulfill it" → "the physical code that realizes it",
and how to reduce ambiguity when an LLM populates that spectrum from day one.

---

## Summary

The concern: an "Intent" spans a wide register — a business *want*, the *logic*
chosen to satisfy it, and the *physical* code it becomes. Does loom need an
intent **sub-category** so the LLM populates each register unambiguously?

**Recommendation: no new intent sub-type.** The spectrum is already modeled —
not inside one node, but **across loom's planes and existing facets**. The
want→logic boundary is carried by `visibility` (and the consumer/proof planes);
the logic→physical boundary is a *node-type* boundary (Intent vs CodeFile via
`IMPLEMENTS`). A `kind: requirement|design|…` axis would duplicate
`abstraction_level` + `visibility`, force a brittle binary onto a genuine
continuum, and work against the **granularity contract** that already
disambiguates well-formed intents.

The real lever for "make it implicit for the LLM" is the **seed *flow***, not the
schema: stage `loom guide --mode seed` as a want → contract → logic → physical
ladder, and promote **`visibility` to a seed-time decision** so every intent is
born knowing whether it is a want or a mechanism. This is loom's standing
teach-adapt-over-hardcode principle: shape *how the LLM populates* through
guidance, don't bolt on a field it must fill everywhere.

---

## Context

loom is driven by an LLM that seeds and grounds intents from day one, across
greenfield and brownfield repos. The quality of that population depends on
whether the LLM can tell *what register* it is describing. The risk the
sub-category idea targets is real: an LLM may mix registers — recording a
mechanism as if it were a want, or minting "intents" that are really
implementation details that fail the granularity test.

The question is whether the fix is a **schema distinction** (sub-type Intent) or
a **guidance distinction** (stage the population flow). This proposal argues the
latter, and shows the schema distinction already exists — distributed across the
plane architecture.

---

## The reframe: it is not "only Intent"

loom has **10 node types across several planes**, and the want→logic→physical
spectrum is *why those planes exist*. The spectrum is spread across nodes and
facets on purpose, not crammed into one node.

| Register | Where it lives | How |
|---|---|---|
| **What the developer wants** | Persona + `user_visible` Intent + Validation | *who* wants it (Persona, `SERVES`), *what* they want (a `user_visible` Intent), the want **as a testable contract** (a Validation, `VALIDATES`) |
| **What logic must exist** | `internal` Intents + HIERARCHY + RELATES_TO | the mechanism, decomposed down the tree; **HIERARCHY *is* the want→logic translation** |
| **The physical lane** | CodeFile + IMPLEMENTS | already a separate plane; `IMPLEMENTS` is the bridge |

Two boundaries are therefore already drawn:

- **logic → physical** is a *node-type* boundary (Intent vs CodeFile). Nothing to
  add.
- **want → logic** inside the Intent family is marked by **`visibility`**:
  `user_visible` ≈ a want (the user experiences it); `internal` ≈ a mechanism
  that exists only to serve other intents. The consumer/proof planes
  (Persona/SERVES/JOURNEYS, Validation/VALIDATES) attach to the *wants*;
  `IMPLEMENTS` attaches to whatever is realized. The "why this exists" chain is
  the HIERARCHY edge — a child intent exists *because* its parent needs it.

**The spectrum is modeled between nodes and across facets, not as sub-types of
one node.**

---

## Decision: no intent sub-category

A `kind: requirement | design | mechanism | …` axis on Intent is rejected for
three reasons.

1. **It duplicates existing axes.** Want-vs-mechanism is already `visibility`;
   altitude is already `abstraction_level`; the physical register is not an
   intent kind at all — it is the CodeFile plane. The sub-type would re-derive,
   inside one node, distinctions the plane architecture already makes between
   nodes.

2. **It forces a brittle binary onto a continuum.** Most real intents are
   *partly* want and *partly* mechanism — "the CLI parses commands and
   dispatches" is both. A mandatory label makes the *classification itself* the
   new ambiguity, which is the opposite of the goal.

3. **It fights the real disambiguator.** loom's well-formedness test is the
   **granularity contract** — "can you write a falsifiable criterion for an edge
   involving this intent?" That test is *register-agnostic*: it holds whether the
   intent is a want or a mechanism. A `kind` field does not make an intent more
   falsifiable; it only adds a slot to argue over.

---

## The mechanism: stage the seed *flow*

Reducing the LLM's ambiguity is a **guidance** problem. Make
`loom guide --mode seed` walk the spectrum as a staged interview, so the
structure is implicit in the *order of questions* — the LLM never picks a
"kind", it just answers down the ladder:

1. **Want** — "Who is this for, and what do they want?"
   → a `user_visible` Intent + a Persona (`SERVES`).
2. **Contract** — "How would we know it's satisfied?"
   → a Validation (`VALIDATES`) — the want made falsifiable.
3. **Logic** — "What must exist to deliver that?"
   → decompose into `internal` child Intents (`HIERARCHY`).
4. **Physical** — "Where does each piece live?"
   → `IMPLEMENTS` to CodeFiles.

Each step uses existing axes; the spectrum falls out of the flow, and the
granularity contract keeps every rung falsifiable. The same ladder runs in
reverse for `--mode brownfield`: start from physical (the synced CodeFiles),
climb to logic (reverse-engineer responsibilities), then to want (which
user-facing behavior does this serve, and what proves it).

This is consistent with how loom already separates judgment from mechanism: the
tool computes the *flow and the prompts*; the LLM supplies the *content*.

---

## Enhancement: promote `visibility` to a seed-time decision

Today `visibility` is "unset = untriaged; the align interview triages it" — i.e.
the want/mechanism distinction is retrofitted later. Make capturing
`user_visible` vs `internal` a **first-class step of the seed flow** (it is the
natural by-product of step 1 vs step 3 above), so every intent is born knowing
whether it is a want or a mechanism.

This delivers exactly the disambiguation the sub-category idea was reaching for —
using a field that **already exists** — instead of inventing a new axis. It also
strengthens downstream signals that already read `visibility` (the align
interview, the consumer-plane coverage), because they no longer start from
untriaged.

---

## The one honest gap

loom has no explicit "**this requirement is stable; this design choice is
replaceable**" marker that survives a *design change*. When the mechanism changes
but the want does not (e.g. swap password hashing from SHA-256 to Argon2), loom
expresses both as intents but does not first-class the stability difference.

The existing answer is the **Validation/Intent split**: the Validation is the
stable contract (the want, made falsifiable), the Intent is the replaceable
mechanism. Before adding anything new, lean on that split — a design change
re-grounds the Intent and re-runs the Validation; the contract is what stays
fixed. Only if this distinction keeps surfacing in practice should a dedicated
marker be considered, and even then it likely belongs on the *edge state*
(re-verification) rather than as an intent sub-type.

---

## Risks & mitigations

- **The seed ladder feels rigid for cross-cutting intents** (error handling,
  JSON output) that have no single persona or clean parent. *Mitigation:* the
  ladder is a default path, not a gate — `cross_cutting` intents enter at the
  logic rung and may skip the persona step; the granularity contract is still
  the only hard requirement.
- **Brownfield climbs the ladder backwards and may over-attribute wants.**
  *Mitigation:* keep want-attribution a *claim* (a `user_visible` flag + a
  Persona `SERVES` edge that is `uninspected` until grounded), never an
  assumption — same discipline as imports (signal, not auto-edge).
- **Visibility-at-birth adds a question to every seed step.** *Mitigation:* it is
  inferred from which rung produced the intent (step 1 → user_visible, step 3 →
  internal), so it is a default the LLM confirms, not a separate quiz.

---

## Non-goals

- A `kind`/sub-type axis on Intent.
- Collapsing want, logic, and physical into intent metadata (they are
  cross-plane by design).
- Auto-classifying register from text without the falsifiable-criterion check.
- Replacing the granularity contract as the well-formedness test.

---

## Open questions

1. **Does brownfield need its own visibility-inference heuristic?** Climbing from
   code, "is this user_visible?" is harder to default than greenfield's
   top-down ask. Possibly route ambiguous cases to the align interview rather
   than guessing.
2. **Should the seed flow emit the Validation stub automatically** at step 2
   (a not_run contract), or only prompt for it? Auto-stub keeps the want
   falsifiable-by-default; prompting avoids empty contracts. Lean auto-stub,
   mirroring the hypothesis-adoption pattern (predicted outcome → not_run
   Validation).
3. **Is `cross_cutting` enough** for intents that are pure mechanism with no
   want, or do they want their own entry rung? Probably enough; revisit only if
   seeding them proves awkward.
