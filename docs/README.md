# loom docs

Operator and architecture reference for the shipped `loom` binary. Start here after the [root README](../README.md).

The compiled CLI (`loom --help`, `loom <command> --help`) outranks these pages when they disagree.

---

## Operator (start here)

1. [`commands.md`](commands.md) — shipped CLI surface plus explicitly marked removed/deferred names.
2. [`llm-driver.md`](llm-driver.md) — how loom and an LLM cooperate: WorkItem, PromptContract, role mindsets, write-back requirements, stop conditions, human gates.
3. [`journey-authoring.md`](journey-authoring.md) — Journey surface authoring, the `journey lint` JSON contract, and versioned blocker/advisory policy.

## Model

4. [`terminology.md`](terminology.md) — canonical vocabulary. Defines stable terms, aliases to avoid, and naming rules used across every other doc.
5. [`graph-model.md`](graph-model.md) — nodes, edges, truth classes, facets, schema, enums, and invariants.
6. [`state-machine.md`](state-machine.md) — event/ripple engine, fact ownership, ripple rules, entry modes, queue routing.

## Archive (not the shipped contract)

These files are kept so the design history is not lost. Do not treat them as current operator instructions.

7. [`design.md`](design.md) — original architecture seed. Still canonical for §2 (truth-class planes), §4 (data model forks), §6 (invariants), §9 (build sequencing), §10 (locked decisions). The rest describes the rebuild as it was planned, not as it shipped.
8. [`build-plan.md`](build-plan.md) — MVP ring sequencing and what each ring had to prove. Rings 1–12 shipped; `CHANGELOG.md` is the later record.
9. [`wiki-projection.md`](wiki-projection.md) — historical wiki design. The wiki shipped in v0.20.0 with a simpler surface; `commands.md` describes what is current.
10. [`rethink-lived-graph.md`](rethink-lived-graph.md) — 2026-07-18 rethink; partially landed. Not operator documentation.
11. [`scratchpad.md`](scratchpad.md) — raw design staging area and working log.
12. [`proposals/pattern-library.md`](proposals/pattern-library.md) — Pattern library implementation authority (B0–B5), already implemented.

---

## Document roles

| File | Role | Canonical? |
|---|---|---|
| `commands.md` | Shipped CLI surface plus removed/deferred names | Yes |
| `llm-driver.md` | WorkItem, PromptContract, role mindsets, write-back | Yes |
| `journey-authoring.md` | Journey surface authoring and versioned lint/acceptance policy | Yes |
| `terminology.md` | Stable language + aliases to avoid drift | Yes |
| `graph-model.md` | Node/edge schema, truth classes, invariants | Yes |
| `state-machine.md` | Event/ripple engine, queue routing, entry modes | Yes |
| `design.md` | Original architecture seed (truth-class spine, locked decisions) | Yes, partial |
| `build-plan.md` | Historical ring sequencing | Archive |
| `wiki-projection.md` | Historical wiki design; shipped surface is simpler | Archive |
| `rethink-lived-graph.md` | Partial rethink; not operator docs | Archive |
| `scratchpad.md` | Raw design staging area and working log | Archive |
| `proposals/pattern-library.md` | Implemented Pattern contract | Yes |

---

## Promotion rule

New terms land in `terminology.md` before use in canonical docs. New operator surface lands in `commands.md` (and the compiled CLI). Architecture decisions that still need a home go to the living model docs above, not to `scratchpad.md`.

`scratchpad.md` is no longer the inbox.
