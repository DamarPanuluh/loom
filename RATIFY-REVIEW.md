# Ratification review — 22 behaviours awaiting your yes

Each is implemented, proven, and user-visible. loom is asking one question only:
**is this behaviour wanted?** Not 'does it work' — that is already proven.

Run each in your terminal. loom will prompt you to type a challenge phrase.

---

## an operator ratifies an intent as wanted

a human (solo agent) records an evidence-bearing ratification on an intent; every llm:* lane is denied the write (INV-8, fail closed); redefining a ratified int

- 3 stand on it
```
loom intent ratify 0950a847 --evidence '<why this is wanted>'
```
---

## paginated list output tells callers how to continue

Every paginated JSON list response exposes its items, total, current window, whether more results exist, and the next offset; text output gives equivalent conti

- 3 stand on it
```
loom intent ratify 09db358f --evidence '<why this is wanted>'
```
---

## loom is reachable in-band as tools, not only as a subprocess

Every capability is served over MCP through the same functions the CLI calls, so an agent pulls context and applies batches without shelling out.

- 3 stand on it
```
loom intent ratify 225041d7 --evidence '<why this is wanted>'
```
---

## external research is routed with durable advisory provenance

when current outside knowledge is needed, Analyze serves a bounded host-browsing task whose actual-page quotations, dates, freshness, and Loom-stamped fingerpri

- 4 stand on it
```
loom intent ratify 3ce98ed5 --evidence '<why this is wanted>'
```
---

## loom maintains a falsifiable graph for LLM-driven codebase work

durable, falsifiable memory an LLM drives: what the code should do, where it lives, how it is proven

- nothing stands on it
```
loom intent ratify 418b5ebb --evidence '<why this is wanted>'
```
---

## wantedness is earned from evidence, not demanded up front

A behavior the code performs, that a proof loom ran covers, and that appears in recorded usage is de-facto wanted without anyone being asked. Asserted human jud

- 4 stand on it
```
loom intent ratify 483fd508 --evidence '<why this is wanted>'
```
---

## an operator captures a topic through door and routes it from the landing menu

loom door records an inbox item and returns a landing menu; the operator picks one typed command and marks the capture routed

- 4 stand on it
```
loom intent ratify 4eefda1c --evidence '<why this is wanted>'
```
---

## proof strength is derived from the proof's shape

S0-S5 is computed from what a proof actually does — did loom run it, does it assert about output rather than exit code, does its call closure reach the behavior

- 3 stand on it
```
loom intent ratify 82b78e81 --evidence '<why this is wanted>'
```
---

## find surfaces each matched intent's grounding

loom find prints the file, locator and verdict of every implements edge under a matched intent, not just the node name — the edge a plain text search lacks

- 4 stand on it
```
loom intent ratify 873d9c05 --evidence '<why this is wanted>'
```
---

## the human is asked only where judgment and evidence disagree

Five concrete divergences — known-unwanted code still live, a promise nothing backs, meaning drifted under a yes, duplicate behaviors, and behavior nobody has s

- 3 stand on it
```
loom intent ratify 8e6ace5e --evidence '<why this is wanted>'
```
---

## loom can fail its own falsifiability check

The self-audit looks for the signatures this graph itself carried: judgments with no act recorded behind them, bursts too fast to have been made one at a time, 

- 3 stand on it
```
loom intent ratify a2fd4889 --evidence '<why this is wanted>'
```
---

## the next router serves the highest-priority asserted residue with a prompt contract

loom next returns failing, then stale, then uninspected asserted edges as a work item carrying allowed/forbidden actions and the evidence it expects

- 4 stand on it
```
loom intent ratify a665d1c5 --evidence '<why this is wanted>'
```
---

## ratified live patterns guide build and repair work

a maintainer can record a strict Pattern with reviewed live-code Exemplars, and matching build or fix packets receive bounded source-backed guidance recoverable

- 4 stand on it
```
loom intent ratify aa3acab2 --evidence '<why this is wanted>'
```
---

## loom writes the graph from the work rather than asking for it twice

Observing the difference between the tree and the graph yields proposed mutations with what loom saw stamped on them, re-checked at adoption. Where loom cannot 

- 3 stand on it
```
loom intent ratify b90a37b6 --evidence '<why this is wanted>'
```
---

## ordered steps are served in order, one readiness gate not a task list

when one behavior is declared to follow another, the build lane serves the earlier step first and reports the later one as blocked naming the step it follows — 

- 3 stand on it
```
loom intent ratify bcfca1ac --evidence '<why this is wanted>'
```
---

## loom runs proofs and reports what it observed

A command's outcome comes from loom executing it, never from a caller describing it. The record carries the exit code, output hashes, and the file hashes in for

- 3 stand on it
```
loom intent ratify be6570d7 --evidence '<why this is wanted>'
```
---

## a change re-opens exactly the claims that pointed at what changed

One re-verification pass asks every anchor whether it still holds, rather than a per-edge-kind table guessing what a change could invalidate. A symbol-scoped lo

- 3 stand on it
```
loom intent ratify c24c2964 --evidence '<why this is wanted>'
```
---

## loom answers what a change here could reach

A call graph resolved from extraction across languages, reporting exact and heuristic resolutions separately rather than blending them into a number nobody can 

- 3 stand on it
```
loom intent ratify cde73673 --evidence '<why this is wanted>'
```
---

## loom answers what other behaviors stand on this one

given a behavior, loom reports every other behavior that transitively stands on it — requires it, is a scenario of it, or decomposes into it — nearest first, ea

- 3 stand on it
```
loom intent ratify dfca4ba1 --evidence '<why this is wanted>'
```
---

## divergences route to a human-only queue

the converged rung gates on blocking divergences, not on unratified count; the compass points at loom next --mode ratify when one exists; plain loom next never 

- 3 stand on it
```
loom intent ratify e5622942 --evidence '<why this is wanted>'
```
---

## changing a file re-opens the asserted edges grounded in it

content-hash ripple: sync stales the asserted edges depending on a changed codefile; an unchanged hash (or a first-ever observation) triggers no ripple, so ther

- 5 stand on it
```
loom intent ratify f2d36098 --evidence '<why this is wanted>'
```
---

## a green graph is pointed at its weakest standing claim

Once every floor is met the ranking says what to strengthen — blast radius against proof strength against evidence age — and names one move. The queue re-orders

- 3 stand on it
```
loom intent ratify ff5d38e1 --evidence '<why this is wanted>'
```
