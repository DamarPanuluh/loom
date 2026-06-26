---
type: flow
title: "Journeys & flows"
tags:
  - journey
---

## Journeys & flows

Every `user_visible` intent with its saga proofs. A saga that hasn't run
is an enumerated-but-not-discharged journey (see `docs/maturity-ladder-proposal.md`).

### `external interface surface plane`

> Represent externally callable surfaces as first-class graph nodes so ownership, journey coverage, quality rules, and implementation grounding can address an interface independently of the saga YAML that calls it.

_(no saga registered yet)_

### `hypothesis plane`

> The pre-decision plane: improvement hypotheses that any lane can propose, an analyzer proves against current code, and a builder adopts into planned intents. Speculation stays invisible to coverage and completeness until adoption converts it into the existing lifecycle.

_(no saga registered yet)_

### `loom: living intent graph CLI`

> Builds and maintains a falsifiable intent graph of a codebase (semantic/physical/normative planes) that any LLM drives via structured CLI commands; the graph is durable memory, the context window is the working set

_(no saga registered yet)_


<!-- loom:prose-start -->
## How a request travels

This page traces a representative `loom` invocation end-to-end, naming every
component intent it touches. The skeleton above lists the journey coverage
from the saga plane; the prose below is the narrative that ties the components
into a single path.

### Flow 1 — `loom next --mode build` (the work-selection path)

1. **Resolve the graph.** `loom` resolves the target graph via `--graph` flag
   > `LOOM_GRAPH` env > cwd, through `resolve_root()` in
   [`src/db/mod.rs`](../src/db/mod.rs). This is the graph-targeting pin that prevents
   the cd-fallback incident class.
2. **Dispatch.** The [CLI surface and dispatch](intent:a1a8eb10-bc4c-43d7-a4ec-b7d1fb8d26ae) parses the
   args and the dispatch table in [`src/commands/mod.rs`](../src/commands/mod.rs) routes
   `next` to [`src/commands/next.rs`](../src/commands/next.rs).
3. **Load the snapshot.** The handler opens the SQLite store via
   [`src/db/sqlite.rs`](../src/db/sqlite.rs) and loads a single shared snapshot through
   the [snapshot analysis and annotation helpers](intent:e2d64ed4-b10f-48fd-995b-f533a6250a18) — the same snapshot that
   `loom smells` and `loom coverage` would use, loaded once.
4. **Rank.** The [priority-scored work queues](intent:47c9182c-f7a8-4a50-9281-6d05507e646c) scores every
   build-eligible intent via [`src/db/queries/scoring.rs`](../src/db/queries/scoring.rs) and
   returns the top item with its lane, evidence requirements, and the exact
   next invocation.
5. **Render.** The [dual-mode output](intent:bb8ee237-2d84-46ee-b254-c8bc39c16fc1) renders the
   item as human text (default) or `--json` (for scripting), with the
   graph_state pulse, through [`src/output.rs`](../src/output.rs).

### Flow 2 — `loom sync` (the change-detection path)

1. **Resolve + dispatch** as above, routing to [`src/commands/sync.rs`](../src/commands/sync.rs).
2. **Re-hash.** The [sync flag engine](intent:29799603-3704-4dfa-9ba4-387a7c1942f8) re-hashes
   every file under the corpus using the FNV-1a content-hash in
   [`src/repo.rs`](../src/repo.rs). A changed hash (not a changed mtime) is the
   trigger — mtime alone false-flags after checkout/rebase.
3. **Propagate.** For each changed file's intents, the one-hop RELATES_TO
   neighbours flip to `needs_reverification`; passing GOVERNS go
   `needs_reverification`; linked validations go `not_run`. Two- and three-hop
   neighbours receive a decaying priority bump that does not change their
   inspection_status.
4. **Report.** Files missing on disk are reported; the
   [completeness and integrity checking](intent:ab4ac603-a14d-4ae4-b68b-a4bf9dce0cb2) is the axis that catches a
   graph drifting from the code.

### Flow 3 — `loom wiki --okf --prose-check` (the comprehension path)

1. **Resolve + dispatch** as above, routing to [`src/commands/wiki.rs`](../src/commands/wiki.rs).
2. **Load.** The handler loads the snapshot and renders the deterministic
   skeleton pages (the byte-checked frame).
3. **Extract prose.** For each page, the prose between the
   `<!-- loom:prose-start -->` and `
<!-- loom:prose-end -->
