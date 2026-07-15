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
| **derived** | machine | file hashes, imports, symbols, external diagnostics, structural findings | No for the fact itself (recomputed by `loom sync` / `loom scan run`); untriaged or stale Finding nodes do route to triage, where the write is a separate asserted adjudication |
| **asserted** | human/LLM with evidence | intent grounding, quality verdict, validation result, completeness waiver | Yes; routed by `loom next` |
| **statistical** | heuristic | co-change or debt signal | No; advisory — explicit `loom debt promote` creates a separate asserted Finding |

This is the v2 spine: derived facts are reproducible, asserted facts persist until invalidated, and statistical signals stay advisory until an operator explicitly promotes a cluster into a separate asserted Finding for ordinary triage.

## Current feature spine

- **External diagnostics:** `loom scan add/list/remove/run` wraps any language's linter, type-checker, or custom diagnostic command. GCC-style `file:line[:col]: message` output works by default, as do two-line diagnostics (a bare `file:line[:col]` location line with the message on the immediately following line, as svelte-check emits); custom named-group regex maps (`file`, `line`, optional `msg`, `code`) are supported and stay strictly per-line. `--format json` consumes JSON/JSONL finding arrays instead (pulse, qualirs, and similar tools), with `field=path` lookups, dotted paths, and `items=<path>` for envelope objects. Parsed diagnostics become derived findings and converge on re-run when diagnostics disappear.
- **Pattern pre-screening:** seeded quality packs can carry regex `patterns[]`. Quality packets embed computed `pre_screened_hits` (`path`, `line`, `pattern`, `excerpt`) at packet-build time so the LLM can confirm or refute every candidate before writing the verdict.
- **Definition-of-Complete:** `loom completeness [intent]` scores user-visible feature intents across scenarios, prerequisites, boundary, proof, journey, and questions. `loom next --mode elaborate` serves the most-incomplete feature and embeds the scorecard.
- **Scenario families:** `loom intent add/set --aspect happy|sad|fallback|edge_case` plus `scenario-of` edges model happy paths, sad paths, fallbacks, and edge cases without inventing a separate scenario node type.
- **Question loop:** product questions are captured with `loom question add "..." --intent <intent>` and surface through `loom session` / `graph_state.open_questions` for batched human answers. Evidence-backed observations use `loom finding add`, not the inbox.
- **Cheap residue routing:** work packets carry `routing_hint` (`mechanical` | `judgment`) and sync grades `cheap re-confirm` vs full re-inspection; orchestrators may batch-reaffirm mechanical items via `loom apply` verdicts.
- **Cold-start assist:** `loom bootstrap suggest` drafts a Proposal of planned pillar intents from codefiles/tests/README (never auto-verdicts).
- **Find + explain:** `loom find --tag` / `--where` and `loom explain <intent>` for facet search and neighborhood briefs.
- **Calibrated structural detectors:** sync's built-in findings (`oversized_file`, `complex_symbol`, `large_symbol`, `deep_nesting`, `excess_args`) run on configurable thresholds; `loom calibrate [--write]` proposes gates from the repo's own metric distribution (worst-5% tail, floored) so detection fits the codebase instead of a universal constant.
- **Advisory debt + promotion:** `loom debt` ranks statistical clusters (`size_outlier` LOC outliers and git-history `co_change`) with stable `cluster_id`s; `loom debt promote <cluster-id> --evidence <TEXT> [--confidence <0..1>]` mints exactly one asserted Finding (`source: debt_promotion`) for ordinary finding triage while the raw feed stays advisory.
- **Portable configuration:** `loom.graph.json` carries the `config` map (`layer_order`, `ignores`, `codefile_globs`, `scan_adapters`, `thresholds`, `evidence_policy`) so imports keep the graph's routing, scan, detector, and policy setup.
- **Wiki projection:** `loom wiki plan/next/record/list/remove` tracks reader-first documentation pages as graph citizens — an agent writes the prose, the graph governs truth and freshness, and `loom sync` stales a page precisely when a documented intent, its code, or its proof drifts.
- **Federation:** `loom graph link` composes graphs across repositories via committed exports; upstream intents appear as shadow nodes that ripple staleness locally without ever entering local queues or gates. Plain `graph unlink` keeps shadows (doctor flags orphans); permanent dispose is `graph unlink --prune` or `graph prune-orphans` (`--cascade` if DependsOn edges remain).

## Storage

A repository using `loom` has two graph artifacts:

```text
.loom/graph.sqlite   # local SQLite working store
loom.graph.json      # deterministic portable export
```

Commit `loom.graph.json` when the graph should travel with the codebase. `.loom/` is local runtime state; portable setup such as layer order, ignores, codefile globs, and scan adapters lives in the export's `config` map.

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
loom validation run --all
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
welcome     Plain-English orientation (also the bare `loom` default)
init        Initialize a graph store
intent      Intent commands
codefile    CodeFile commands
export      Write loom.graph.json
import      Restore from loom.graph.json
apply       Apply one atomic batch of mutations from a JSON/YAML file
sync        Recompute structural facts and ripple staleness
status      Print graph identity and counts
mode        Show or set the graph mode (owned | observed)
next        Return the next routed work item
edge        Edge commands
door        Capture free-form input as an inbox item
inbox       Inbox commands
task        TaskRecord commands
note        Durable notes on graph nodes
session     Turn-zero offer menu
guide       Role/lane guidance
find        Keyword search over graph facts
detect      Repo language detection and quality pack recommendation
scan        External diagnostic adapters to derived findings
calibrate   Derive structural finding thresholds from the repo
threshold   Hand-set structural finding thresholds
policy      Evidence policy (review floor + human gates)
completeness  Definition-of-Complete scorecard
schema      Print the data model
rule        Quality rule commands
validation  Proof commands
hypothesis  Hypothesis commands
surface     Interface surface commands
vocab       Vocabulary commands
layer       Architecture layer-order commands
smells      Structural smell report
debt        Advisory statistical feed (size outliers, co-change) + explicit promotion
finding     Evidence-backed capture and asserted adjudication
doctor      Integrity audit
coverage    Vertical-spine coverage report
ignore      Coverage exclusion commands
whoami      Acting-agent/lane report
proposal    Proposal capture and item adoption
journey     Journey proof, coverage, and invariant commands
wiki        Reader-first wiki pages tracked as a graph projection
graph       Cross-graph federation (link/unlink/list upstreams)
```

`docs/commands.md` describes the shipped CLI surface plus explicitly marked removed/deferred names. Treat the compiled CLI help as the source of truth for what is implemented now.

## Typical agent loop

```bash
loom sync
loom --json status
loom --json next --all
```

Then follow the returned lane:

- **build** — realize planned or changed intents.
- **fix** — repair failing asserted facts at root cause (never records verdicts).
- **analyze** — inspect relationships and record evidence.
- **validate** — run or mark proofs.
- **quality** — inspect rules against intents.
- **elaborate** — complete the surroundings of user-visible feature intents: scenarios, prerequisites, proof, journey coverage, and product questions.
- **triage** — route inbox items and untriaged/stale findings (derived and asserted) for durable adjudication.

After graph mutations:

```bash
loom export
loom export --check
```

If you wire this into a user-created pre-commit hook, keep the hook defensive: prefer `loom export --check`, and feature-detect any optional command before invoking it (older loom binaries may not have newer surfaces such as `loom wiki` or `loom graph`).

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
docs/commands.md         shipped CLI surface + removed/deferred names
docs/build-plan.md       ring sequencing and shipped-surface corrections
docs/design.md           architecture seed and locked decisions
```

## Project status

This repository is a greenfield rebuild of `loom`. The old implementation is a reference oracle for behavior and edge cases; this codebase re-derives the system cleanly around the v2 truth-class model.
