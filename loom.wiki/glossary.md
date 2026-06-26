---
type: glossary
title: "Glossary"
tags:
  - vocabulary
---

## Glossary

The bounded `loom vocab` registry. Each term lists the intents that carry it.

### `corpus`

- **source corpus coverage**

### `daemon`

- **SQLite direct concurrency policy**

### `export`

- **SQLite import export parity bridge**
- **graph travel format**
- **migration cutover and rollback path**

### `migration`

- **SQLite direct concurrency policy**
- **SQLite-backed graph persistence migration**
- **backend-neutral storage boundary**
- **migration cutover and rollback path**
- **storage documentation and guide refresh**
- **storage responsibility vocabulary coverage**
- **sync ripple indexed update path**
- **validation and saga storage isolation**

### `parity`

- **SQLite import export parity bridge**
- **SQLite query and search implementation**
- **graph travel format**
- **migration cutover and rollback path**
- **storage contract regression suite**

### `query`

- **SQLite graph persistence**
- **SQLite query and search implementation**
- **backend-neutral storage boundary**
- **endpoint-constrained edge storage**
- **shared graph query snapshot layer**
- **storage contract regression suite**
- **sync ripple indexed update path**

### `schema`

- **inbox intake boundary**
- **storage responsibility vocabulary coverage**
- **typed SQLite graph schema**

### `sqlite`

- **SQLite direct concurrency policy**
- **SQLite import export parity bridge**
- **SQLite query and search implementation**
- **SQLite-backed graph persistence migration**
- **storage documentation and guide refresh**
- **typed SQLite graph schema**

### `storage`

- **SQLite graph persistence**
- **SQLite-backed graph persistence migration**
- **backend-neutral storage boundary**
- **endpoint-constrained edge storage**
- **shared graph query snapshot layer**
- **storage contract regression suite**
- **storage documentation and guide refresh**
- **storage responsibility vocabulary coverage**
- **sync ripple indexed update path**
- **typed SQLite graph schema**
- **validation and saga storage isolation**


<!-- loom:prose-start -->
## The vocabulary

The skeleton above lists every vocab term declared in the graph. The prose
below defines the terms that recur across the wiki and explains the
distinctions that matter when reading the other pages. Terms are grouped by
the component that owns them.

### Graph shape

- **Intent** — a named, criteriated, lifecycle-tracked responsibility node.
  Every intent has an abstraction level (system / component / feature / leaf),
  a visibility (internal / user_visible), a lifecycle (planned / implemented /
  deprecated), and a status (proposed / confirmed). The system intent
  [loom: living intent graph CLI](intent:6faa01a5-920a-4231-b840-f4c2057d149b) is the root.
- **CodeFile** — a registered source file with a content-hash. The
  [SQLite graph persistence](intent:01783338-7f02-4f4b-8d15-5f396ef7d47d) stores them; the
  [sync flag engine](intent:29799603-3704-4dfa-9ba4-387a7c1942f8) re-hashes them on sync.
- **Edge** — a typed relationship between two endpoints. Every edge kind is
  [endpoint-constrained edge storage](intent:29288a6c-3f0b-4762-a34d-ad4a714b5390): keyed by endpoint ids with a
  derived stable edge id, identity-stable across re-import.

### Edges

- **RELATES_TO** — intent-to-intent coupling. The discovery lane ranks
  unexplored pairs; the [priority-scored work queues](intent:47c9182c-f7a8-4a50-9281-6d05507e646c) surfaces
  them.
- **HIERARCHY** — parent/child decomposition. A component intent owns a family
  of feature intents.
- **IMPLEMENTS** — intent-to-codefile grounding with an optional locator (a
  symbol anchor in the file).
- **GOVERNS** — rule-to-intent: a QualityRule governs an intent. The quality
  lane records verdicts on GOVERNS pairs.
- **VALIDATES** — validation-to-intent: a proof object attached to an intent.
- **TARGETS** — hypothesis-to-intent: the speculation plane's edge.
- **SERVES** — interface-to-intent: the [external interface surface plane](intent:fcf2f089-6dbe-46f0-8296-d50512420ff8)'s edge.
- **JOURNEYS** — saga-to-interface: the [saga consumer plane](intent:4c752ad2-e332-4148-87e2-88340991e2a5)'s edge.

### Lanes and gates

- **Lane** — a role with write authority over a class of transitions. The
  [role lanes and evidence gates](intent:20bf582e-df31-4f96-8b37-6171c38e3478) declares the lanes and their
  evidence requirements.
- **Builder** — owns realization: seeding, grounding, realizing intents.
- **Validator** — owns proof: running validations and recording honest results.
- **Analyst** — owns exploration: discovery, smells, edge exploration.
- **Registrar** — owns clerk work: registering codefiles, closing coverage.
- **Reviewer** — owns the strategic double-check: verdicts with confidence <
  0.7 escalate here.

### Quality and proof

- **QualityRule** — a named anti-pattern with a criterion and an inspection
  effort. Seeded from a pack; governs intents.
- **Verdict** — a lane's ruling on a GOVERNS pair, with confidence and an
  evidence locator. Confidence < 0.7 escalates to the reviewer.
- **Validation** — a proof object attached to an intent: a command, an expected
  outcome, and a last result (not_run / passed / failed / blocked).
- **Asserted-only vs executed-proven** — a validation that merely asserts a
  pass vs one that runs and discriminates behaviour. The integrity axis
  ([completeness and integrity checking](intent:ab4ac603-a14d-4ae4-b68b-a4bf9dce0cb2)) flags the former.

### Speculation and surfaces

- **Hypothesis** — a pre-decision improvement proposal. The
  [hypothesis plane](intent:32b42fd0-6b6c-46a6-a9a8-97be595bddf3) is the plane; adoption
  converts a hypothesis into planned intents, never duplicates.
- **Interface surface** — an externally callable surface modelled as a graph
  node, owned by the [external interface surface plane](intent:fcf2f089-6dbe-46f0-8296-d50512420ff8).
- **Saga** — a YAML-defined call sequence over interface surfaces, validated by
  the [saga consumer plane](intent:4c752ad2-e332-4148-87e2-88340991e2a5).
- **Seed flow** — a reaction-driven realization path for a specific intent
  class. The [UI/UX visual-register seed flow](intent:c6b2b5d9-387a-4fa3-936d-e4e780021067) is the
  visual-register variant; the
  [intent-spectrum seed-flow guidance](intent:e21d2c64-f9fa-46c0-aaec-1985efbf8152) is the guidance that points at
  the right one.

### Sync and analysis

- **sync flag** — the `needs_reverification` / `not_run` flip the
  [sync flag engine](intent:29799603-3704-4dfa-9ba4-387a7c1942f8) propagates one hop from a
  changed file's intents, with a decaying ripple beyond.
- **content-hash** — FNV-1a 64-bit hash of file bytes, the change detector's
  truth (mtime alone false-flags after checkout/rebase).
- **corpus** — the source corpus the [source corpus coverage](intent:dda91659-329a-479e-9d33-42a41b5fa9b1) tracks.
- **static analysis** — the [multi-language static analysis coverage](intent:ea9c7e3e-9f95-4d58-ade9-d771fcf50cc3) plane
  that extracts imports, declarations, and layout signals per language.

### Comprehension

- **OKF** — the Open Knowledge Format: a directory of markdown concept files
  with YAML frontmatter and cross-links. This wiki is an OKF bundle.
- **provenance stamp** — the frontmatter field carrying the content-hashes of
  cited codefiles and the ids of cited intents, so prose freshness is
  mechanically checkable.
- **coverage ledger** — the set of salient intents cited by at least one prose
  page. The coverage gate fails on any uncited salient intent.

<!-- loom:prose-end -->
