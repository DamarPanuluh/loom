# Proposal: the UI/UX flow — reaction-driven, mockup-as-CodeFile, human-verified residue

**Status:** Accepted as part of a coherent proposal set (graph decision note
`7cb29581`, 2026-06-15), with the mockup-realization amendment below folded in.
**Date:** 2026-06-15
**Scope:** how loom captures UI/UX intent before files exist, what the flow
should be when a human arrives with a vague/partial visual idea, and how UI
proof divides between machine and human. Companion to
[`intent-spectrum-proposal.md`](intent-spectrum-proposal.md) — this is the
seed-flow specialised for the visual register.

---

## Summary

UI/UX is genuinely different from backend logic — the spec is visual, the
structure is a composition DAG (not a responsibility tree), the proof is
visual/interactional, and the want is experiential. But **loom needs no UI plane
and no UI node types**: the existing planes already cover it, with three
refinements to the flow.

1. **Reaction-driven, not spec-driven.** A human cannot author a UI in the
   abstract but can *react* to one. The flow's job is to produce a concrete
   reaction surface fast, then convert the human's reaction into graph deltas.
   This is the erratic-human-flow case loom is built for: absorb partial input
   into resumable structure, and compute what only the human can answer vs what
   the LLM can discover.

2. **The HTML mockup IS a CodeFile — but it is the contract, not realization.**
   A mockup committed to the repo is a physical-plane artifact loom already
   syncs, hashes, diffs, and version-controls, so the visual contract needs
   **zero new machinery** — the screen intent `source_ref`s it (and uses it as
   criterion + visual-regression proof target). **It must NOT satisfy production
   `IMPLEMENTS`**: a mockup is evidence of the contract, not realization of the
   screen, so the production screen stays `planned` until real app code grounds
   it (see *Amendment* under Refinement 2). Images are an *input* modality
   (multimodal ingestion of the human's reference), never the persisted contract.

3. **Verification splits machine-first, human-residue-only.** Automate what is
   provable (visual-regression diff, a11y, responsive, interaction/saga); reserve
   `manual_check` (`inspected_by=human`) for the aesthetic residue, and route it
   through loom's user-presence-scarcity model as a new user-gated queue.

---

## Context

loom is driven by an LLM helping a human who arrives with a vague, partial,
wandering idea. The human does not drive loom — they give fragments, and the LLM
uses loom to figure out what to do. For UI this is the hardest case: a visual
idea is mostly aesthetic and underspecified, so it sits heavily on the "only the
human can answer" side. The flow must therefore minimise demand on the scarce
human while capturing the idea faithfully — which is exactly loom's existing
`session`/`door`/`guide` doctrine, specialised here for the visual register.

---

## What already fits (no new schema)

| UI/UX thing | loom home |
|---|---|
| A screen / page | `user_visible` Intent, `SERVES` a Persona |
| A user flow ("land → search → cart → checkout") | a **saga** Validation + `JOURNEYS` + Persona — the consumer plane is a journey engine |
| UI states (empty / loading / error / populated) | the **`aspect`** facet; `happy_path_only` becomes a **state-coverage smell** |
| Design system / a11y / responsive standards | **QualityRule** (`rule seed web-ui` / `mobile` already exist), `GOVERNS` the components |
| Components, styles, tokens, **mockups** | CodeFile, synced like anything else |

The fit is strong precisely where UI is hardest: flows land on saga/Persona/
JOURNEYS, and state coverage falls out of `aspect` + `happy_path_only`.

### The modeling trap to avoid

Do **not** HIERARCHY-nest shared components under every screen — HIERARCHY is a
single-parent tree and a `Button` has many parents. Model **atomic design**: the
design system is its own HIERARCHY subtree (atoms → molecules → organisms), and
each screen **RELATES_TO** the components it composes. Screen decomposition =
HIERARCHY; "screen uses component" = RELATES_TO.

---

## Refinement 1 — reaction-driven flow

A vague visual idea cannot be specced top-down; it must be *reacted to*. So the
loop anchors on a concrete artifact produced early:

```
human's vague idea  (words and/or a reference image)
   │  LLM ingests (multimodal), elicits just enough
   ▼
HTML mockup  (committed → a CodeFile)            ← the reaction surface
   │  human reacts: "bigger, move that up, too busy"
   ▼
graph deltas  (screens · regions · components · states · flow)
   │  regenerate mockup; repeat until the human stops reacting
   ▼
generate real code  →  visual-regression / interaction proof  →  closes back to the mockup
```

loom's role in the erratic flow is twofold and already encoded in `session.rs`:

- **Absorb fragments into resumable structure.** Every reaction lands as a
  node/edge delta (often `proposed`/`uninspected`), so wandering never loses
  work — *"land every conversational fragment before going autonomous;
  conversation residue is the failure mode."*
- **Split human-only from LLM-discoverable.** *"If code or the graph can answer
  it, don't ask — explore instead."* Aesthetic/structural choices go to the
  human (one question, recommendation-first); everything derivable from the
  mockup's DOM or the codebase, the LLM resolves itself.

---

## Refinement 2 — the mockup is an HTML CodeFile (not an external reference)

The earlier intent-spectrum proposal left open "how to reference the visual."
For UI this resolves cleanly: **commit an HTML mockup to the repo and it is a
CodeFile.** The visual contract becomes physical-plane-native — no node type, no
external pointer.

Why HTML beats the alternatives:

- **vs Figma/URL:** URLs rot, need auth, live outside the repo. An HTML mockup is
  version-controlled, travels with the graph, and loom already tracks it.
- **vs an image:** images are binary (don't diff, bloat the repo) and force
  *lossy, inferred* structure extraction from pixels. HTML's DOM **is** the
  screen→region→component tree, so extraction is *deterministic parsing*.
- **vs prose/ASCII wireframe:** underspecified and not renderable — no reaction
  surface, no proof target.

And HTML collapses three roles into one file: **seed structure** (DOM →
decomposition), **reaction surface** (renders in a browser), and **proof target**
(visual-regression renders the same artifact; the mockup can become a Storybook
story, shrinking the mockup→code gap to near zero). Design-system QualityRules
(contrast, spacing, touch targets) can be checked against the rendered mockup
*before* the real code exists.

### Images are an input modality, not a stored artifact

The human's idea often *arrives* as a screenshot of an app they like, a Figma
export, or a napkin photo. The LLM ingests it (multimodal) to bootstrap
understanding — then produces the HTML mockup as the persisted contract. Images
flow **in**; HTML is what the graph references at both ends.

### Amendment (accepted) — mockup ≠ realization

The mockup being a CodeFile must **not** let it satisfy production `IMPLEMENTS`
for a screen. The vertical spine treats a leaf as realized once it has ≥1
`IMPLEMENTS` edge; if a production screen `IMPLEMENTS` its mockup, loom would
report it **done when only a mockup exists** — false realization that poisons the
"done" signal. So:

- A production **screen intent** references the mockup via `source_ref` (the
  contract), uses it as `criterion` ("matches `<mockup file>`") and as the
  visual-regression **proof target** — and stays `lifecycle = planned` until
  **real app code** grounds it with an `IMPLEMENTS` edge.
- `IMPLEMENTS` → mockup is legitimate **only** when the intent's purpose *is* the
  artifact (an explicit prototype / mockup / Storybook-story intent). There, the
  mockup file *is* the realization.

The mockup is contract/source/proof; production code is realization. Keeping
them distinct is what preserves vertical-completeness honesty.

---

## Refinement 3 — verification splits machine-first, human-residue-only

"Human-verifiable" must not mean "only a human can verify" — that wastes the
scarce resource. Divide the proof:

- **Machine-provable (most of it):** visual-regression diff against the mockup,
  a11y rules, responsive breakpoints, interaction flows (Playwright/saga). These
  are QualityRules + `test`/`saga` Validations — no human needed.
- **Human-gated residue (the small subjective part):** "does the hierarchy read
  right / does it feel premium / is it too busy." A `manual_check` Validation,
  `inspected_by=human`.

Route the residue like every other user-gated queue in `session.rs`
(`align`/`rulings`/`blocked`): the human's presence is scarce, so **batch the
visual confirmations and surface them when the human is present**,
recommendation-first. A "visual confirm" queue is the new user-gated lane.

---

## The UI seed ladder (concrete)

Specialising the want → contract → logic → physical ladder from the
intent-spectrum proposal:

1. **Persona + Journey** — who, and the flow they take → Persona + a saga
   skeleton (`JOURNEYS`).
2. **Mockup** — generate the HTML reaction surface (from the human's words +
   any reference image); commit it → a CodeFile.
3. **Screens** — the journey's stops → `user_visible` Intents that `source_ref`
   the mockup file (the contract). They stay `lifecycle = planned` and are **not**
   `IMPLEMENTS`-linked to the mockup; production code grounds them later (see the
   *Amendment* under Refinement 2).
4. **Regions / components** — HIERARCHY children for screen-local regions;
   **RELATES_TO** the shared design-system components (composition).
5. **States** — set `aspect` per component (populated / empty / loading /
   error), so `happy_path_only` enforces state coverage.
6. **Standards** — `rule seed web-ui` / `mobile` → QualityRule, `GOVERNS` the
   components.
7. **Visual contract** — the intent's `criterion` = "matches `<mockup file>`";
   the proof = a visual-regression / interaction Validation whose `command` runs
   the check; the aesthetic residue is a `manual_check`.

The LLM never invents a node type — it walks the ladder, and the UI structure
falls out of existing primitives anchored on a repo-native mockup file.

---

## Risks & mitigations

- **Mockup counted as realization (false done).** *Mitigation:* the accepted
  amendment — a production screen `source_ref`s the mockup but never `IMPLEMENTS`
  it; it stays `planned` until real code grounds it. `IMPLEMENTS` → mockup only
  for explicit prototype/Storybook intents.
- **The mockup drifts from the real implementation framework.** *Mitigation:* the
  mockup is a *contract*, not the impl; prefer generating it in the target
  framework (a Storybook story) so the gap and the drift both shrink.
- **Reaction loop never converges (endless tweaks).** *Mitigation:* each reaction
  is a graph delta with provenance; when reactions stop changing structure, the
  remaining diffs are aesthetic residue → route to the human-confirm queue and
  freeze the contract.
- **Image ingestion over-attributes structure.** *Mitigation:* the image only
  bootstraps; the *deterministic* structure comes from the committed HTML DOM,
  not the pixels. Treat image-derived claims as `uninspected` until the mockup
  confirms them.
- **Aesthetic residue masquerades as machine-checkable.** *Mitigation:* if a
  criterion cannot be expressed as a runnable check, it is residue by definition
  → `manual_check`, never a false-green automated Validation.

---

## Non-goals

- A UI plane, UI node types, or a "design reference" node.
- Embedding mockup images (or Figma bytes) in the graph — reference a repo file
  instead.
- Treating every UI proof as human-only (automate the provable majority).
- HIERARCHY-nesting shared components (use RELATES_TO composition).
- Letting a mockup satisfy production `IMPLEMENTS` / mark a screen realized
  (mockup = contract; only explicit prototype intents `IMPLEMENTS` it).

---

## Open questions

1. **Should the seed flow auto-generate the mockup**, or only when the human's
   idea is visual? Lean auto for any `user_visible` screen intent — the reaction
   surface is cheap and high-leverage.
2. **Does the "visual confirm" queue need its own `loom next --mode`,** or does it
   ride the existing `validate`/align lanes? Probably a facet of validate filtered
   to `manual_check` + `inspected_by` unset.
3. **Mockup location convention** — a `mockups/` dir, or colocated with the
   component? Colocated keeps the `IMPLEMENTS`/`source_ref` link obvious and the
   visual-regression target adjacent.
4. **Visual-regression tooling is repo-specific** — per teach-adapt-over-hardcode,
   loom should *teach the LLM to wire it* (the Validation `command`), not hardcode
   a differ.
