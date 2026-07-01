# loom v2 docs

Design source for the loom v2 rebuild. Start here.

---

## Reading order

1. [`terminology.md`](terminology.md) — canonical vocabulary. Read this first. Defines all stable terms, aliases to avoid, and naming rules used across every other doc.
2. [`graph-model.md`](graph-model.md) — nodes, edges, truth classes, facets, schema, enums, and invariants. The data model everything else is built on.
3. [`state-machine.md`](state-machine.md) — event/ripple engine, fact ownership, ripple rules, entry modes (greenfield/brownfield/refactor/port/external), queue routing.
4. [`llm-driver.md`](llm-driver.md) — how loom and an LLM cooperate: WorkItem, PromptContract, role mindsets, write-back requirements, stop conditions, human gates.
5. [`wiki-projection.md`](wiki-projection.md) — WikiManifest, document center structure, citations, page dependency tracking, incremental invalidation, verify gates, preview/publish flow.
6. [`commands.md`](commands.md) — full CLI surface; MVP vs deferred sections labelled.
7. [`build-plan.md`](build-plan.md) — MVP ring sequencing (rings 1–7), milestone 2 scope, test invariants, dogfood milestone.
8. [`design.md`](design.md) — original architecture seed. Still canonical for §2 (truth-class planes), §4 (data model forks), §6 (invariants), §9 (build sequencing), §10 (locked decisions).

---

## Document roles

| File | Role | Canonical? |
|---|---|---|
| `terminology.md` | Stable language + aliases to avoid drift | Yes |
| `graph-model.md` | Node/edge schema, truth classes, invariants | Yes |
| `state-machine.md` | Event/ripple engine, queue routing, entry modes | Yes |
| `llm-driver.md` | WorkItem, PromptContract, role mindsets, write-back | Yes |
| `wiki-projection.md` | WikiManifest, citations, verify gates, publish flow | Yes |
| `commands.md` | Full CLI surface; MVP vs deferred labelled | Yes |
| `build-plan.md` | Ring sequencing, invariants, dogfood milestone | Yes |
| `design.md` | Original architecture seed (truth-class spine, locked decisions) | Yes, partial |
| `scratchpad.md` | Raw design staging area and working log | No |

---

## Promotion rule

```text
scratchpad.md
  → terminology.md   (stable terms)
  → graph-model.md / state-machine.md / llm-driver.md / wiki-projection.md / commands.md
  → design.md        (consolidated architecture)
```

New terms must land in `terminology.md` before use in canonical docs. New design decisions must be captured in `scratchpad.md` before being promoted.

---

## All docs present

All planned docs exist. New design decisions go to `scratchpad.md` first, then promote in the order above.
