# loom

`loom` is a SQLite-backed CLI for maintaining a falsifiable graph of what a codebase should do, where that behavior lives, and how it is proven.

The model supplies judgment and evidence. `loom` supplies durable memory, routing, staleness, coverage, integrity checks, and a portable export that can travel with the repository.

## Why it exists

Long-running LLM codebase work fails when context is only conversational. Decisions rot, proofs go stale, files lose owners, and advisory smells turn into an undifferentiated debt wall.

`loom` makes the working model explicit:

- **Intent** — what the code is supposed to do.
- **CodeFile** — where behavior lives.
- **Validation** — how behavior is proven.
- **QualityRule** — what good looks like.
- **Edge** — the typed claim connecting those facts, with status, evidence, and confidence.

The goal is not autonomous coding. The LLM still acts. `loom` routes, remembers, invalidates, and checks the work.

## Core idea: three truth classes

`loom` separates graph facts by how they become true:

| Truth class | Owner | Example | Routed as required work? |
|---|---|---|---|
| **derived** | machine | file hashes, imports, symbols, structural findings | No; recomputed by `loom sync` |
| **asserted** | human/LLM with evidence | intent grounding, quality verdict, validation result | Yes; routed by `loom next` |
| **statistical** | heuristic | co-change or debt signal | No; advisory until promoted |

This is the v2 spine: derived facts are reproducible, asserted facts persist until invalidated, and statistical signals never become mandatory work by existing.

## Storage

A repository using `loom` has two graph artifacts:

```text
.loom/graph.sqlite   # local SQLite working store
loom.graph.json      # deterministic portable export
```

Commit `loom.graph.json` when the graph should travel with the codebase. `.loom/` is local runtime state.

## Build

```bash
cargo build
cargo run -- --help
```

Install from a checkout:

```bash
cargo install --path .
loom --help
```

## Quick start

Initialize a graph in a repository:

```bash
loom init .
```

Map code and behavior:

```bash
loom intent add \
  --name "checkout captures payment" \
  --description "payment is captured before fulfillment continues" \
  --level feature

loom codefile add 'src/**/*.rs'

loom edge implement \
  "checkout captures payment" \
  src/checkout.rs \
  --locator "fn capture_payment"
```

Attach a proof:

```bash
loom validation add \
  --name "checkout payment capture test" \
  --type test \
  --command "cargo test checkout_captures_payment" \
  --intent "checkout captures payment"
```

Drive the loop:

```bash
loom sync
loom status
loom next --all
loom validate --all
loom export
```

Use JSON for agent-facing output:

```bash
loom --json status
loom --json next --all
```

## Implemented CLI surface

The implemented command surface is the one printed by the binary:

```bash
loom --help
```

Current top-level commands:

```text
init        Initialize a graph store
intent      Intent commands
codefile    CodeFile commands
export      Write loom.graph.json
import      Restore from loom.graph.json
sync        Recompute structural facts and ripple staleness
status      Print graph identity and counts
next        Return the next routed work item
edge        Edge commands
door        Capture free-form input as an inbox item
inbox       Inbox commands
task        TaskRecord commands
session     Turn-zero offer menu
guide       Role/lane guidance
find        Keyword search over graph facts
detect      Repo language detection and quality pack recommendation
schema      Print the data model
rule        Quality rule commands
validation  Proof commands
validate    Run pending proofs
hypothesis  Hypothesis commands
surface     Interface surface commands
saga        Composition proof commands
vocab       Vocabulary commands
layer       Architecture layer-order commands
interface   Interface-plane gap report
smells      Structural smell report
debt        Statistical debt feed
finding     Derived finding adjudication
doctor      Integrity audit
coverage    Vertical-spine coverage report
ignore      Coverage exclusion commands
whoami      Acting-agent/lane report
```

`docs/commands.md` describes the full target surface and may include deferred behavior. Treat the compiled CLI help as the source of truth for what is implemented now.

## Typical agent loop

```bash
loom sync
loom --json status
loom --json next --all
```

Then follow the returned lane:

- **build** — realize planned or changed intents.
- **fix** — repair failing or stale asserted facts.
- **analyze** — inspect relationships and record evidence.
- **validate** — run or mark proofs.
- **quality** — inspect rules against intents.

After graph mutations:

```bash
loom export
loom export --check
```

If you wire this into a user-created pre-commit hook, keep the hook defensive: prefer `loom export --check`, and feature-detect any optional command before invoking it (for example `loom wiki ...` surfaces are not part of the implemented CLI listed above).

## Documentation map

Start with:

```text
docs/README.md
```

Useful design docs:

```text
docs/terminology.md      canonical vocabulary
docs/graph-model.md      nodes, edges, facets, schema, invariants
docs/state-machine.md    routing, ripple, lifecycle
docs/llm-driver.md       how loom and an LLM cooperate
docs/commands.md         full target CLI surface
docs/build-plan.md       ring sequencing and milestone plan
docs/design.md           architecture seed and locked decisions
```

## Project status

This repository is a greenfield rebuild of `loom`. The old implementation is a reference oracle for behavior and edge cases; this codebase re-derives the system cleanly around the v2 truth-class model.
