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
- **Edge** — the typed claim connecting those facts.
- **Fact** — the asserted state of a claim, and the evidence anchoring it.

The goal is not autonomous coding. The LLM still acts. `loom` routes, remembers, invalidates, and checks the work.

## Core idea: loom only counts what it can re-check

Every asserted fact carries a **verification level**, and the level is not a
label anyone writes — it is derived from evidence loom can independently
re-examine:

| Level | Means | Reached by |
|---|---|---|
| **verified** | loom ran something and watched | `loom validation run` / `loom journey run`; a locator that re-resolves to a live symbol; a quality rule's patterns scanned by loom itself |
| **cited** | anchored to bytes or a journal entry | a `file:line` citation, fingerprinted and re-checked when the file changes |
| **claimed** | prose only | recorded in full, and it never satisfies a rung |
| **expired** | every anchor has since broken | what a `verified` fact becomes when the code it covered moves |

The type a caller may supply has **no way to express a `Run`**. Asking loom to
run something is the only route to `verified` — "mark it passed without running
it" is a compile error, not a policy.

Prose is still recorded, always. It just does not count. A claim justified only
by a sentence never settles, so its lane keeps serving it.

Two consequences worth knowing before you use this:

- **An absence can be evidence.** "No hardcoded secrets in the code realizing X"
  has nothing to cite — but loom can scan the rule's patterns itself and record
  finding nothing. A later hit refuses a contradicting passing verdict, and
  prints the lines it found.
- **Proofs expire on their own.** A run records the file hashes in force when it
  ran. Edit one, and the proof stops counting and its claim re-opens — no one
  has to remember to re-run it.

Facts are still classed by how they become true (`derived` recomputed by sync,
`asserted` persisted until invalidated, `statistical` advisory and never gating).
Verification is the second axis: not *who said it*, but *what would show it
false*.

## Current feature spine

- **External diagnostics:** `loom scan add/list/remove/run` wraps any language's linter, type-checker, or custom diagnostic command. GCC-style `file:line[:col]: message` output works by default, as do two-line diagnostics (a bare `file:line[:col]` location line with the message on the immediately following line, as svelte-check emits); custom named-group regex maps (`file`, `line`, optional `msg`, `code`) are supported and stay strictly per-line. `--format json` consumes JSON/JSONL finding arrays instead (pulse, qualirs, and similar tools), with `field=path` lookups, dotted paths, and `items=<path>` for envelope objects. Parsed diagnostics become derived findings and converge on re-run when diagnostics disappear.
- **Pattern pre-screening:** seeded quality packs can carry regex `patterns[]`. Quality packets embed computed `pre_screened_hits` (`path`, `line`, `pattern`, `excerpt`) at packet-build time so the LLM can confirm or refute every candidate before writing the verdict.
- **Definition-of-Complete:** `loom completeness [intent]` scores user-visible feature intents across scenarios, prerequisites, boundary, proof, journey, and questions. `loom next --mode elaborate` serves the most-incomplete feature and embeds the scorecard.
- **Scenario families:** `loom intent add/set --aspect happy|sad|fallback|edge_case` plus `scenario-of` edges model happy paths, sad paths, fallbacks, and edge cases without inventing a separate scenario node type.
- **Question loop:** product questions are captured with `loom question add "..." --intent <intent>` and surface through `loom session` / `graph_state.open_questions` for batched human answers. Evidence-backed observations use `loom finding add`, not the inbox.
- **Cheap residue routing:** work packets carry `routing_hint` (`mechanical` | `judgment`) and sync grades `cheap re-confirm` vs full re-inspection; orchestrators may batch-reaffirm mechanical items via `loom apply` verdicts.
- **Cold-start assist:** `loom bootstrap suggest` drafts a Proposal of planned pillar intents from codefiles/tests/README (never auto-verdicts).
- **Find + explain:** `loom find --tag` / `--where` and `loom explain <intent>` for facet search and neighborhood briefs.
- **Calibrated structural detectors:** sync's built-in findings (`oversized_file`, `complex_symbol`, `large_symbol`, `deep_nesting`, `excess_args`) run on configurable thresholds; `loom calibrate [--write]` fits gates to the repo's own distribution. Calibration only ever RELAXES: the default says "this is bad in any codebase", the repo's distribution says "our normal runs looser than that", and a gate fitted *below* the default would flag code that is fine by any absolute standard for sitting above its neighbours — which is how a percentile-based detector manufactures a debt wall in proportion to repo size.
- **Call graph:** extraction records call sites per file (Rust, Python, Go, JS, TS); `loom impact` resolves them on demand and reports `exact` and `heuristic` matches separately.
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
calibrate   Relax structural thresholds to fit this repo (never tightens)
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
impact      What a change here could reach (callers, intents at risk)
mcp         Serve loom in-band over MCP (stdio JSON-RPC)
```

## In-band delivery

`loom mcp serve` speaks MCP over stdio, so an agent pulls context as a tool call
instead of shelling out. Register it with any MCP client:

```json
{ "command": "loom", "args": ["mcp", "serve", "--graph", "/path/to/repo"] }
```

Tools: `loom_status` (ladder, compass, queue depths), `loom_next` (the next work
packet with its prompt contract), `loom_context` (read-only context for an
intent, a file, or a query). Each is a thin wrapper over the same function the
CLI calls — a tool and its CLI twin cannot report different numbers.

Every served packet carries a `packet_id` and appends one `packet_served` entry
to the append-only journal. That record is what makes "did loom's context
actually change the outcome?" a measurable question rather than a claim.

`docs/commands.md` describes the shipped CLI surface plus explicitly marked removed/deferred names. Treat the compiled CLI help as the source of truth for what is implemented now.

## Typical agent loop

```bash
loom sync
loom --json status
loom --json next --all
```

Then follow the returned lane. Every lane owns exactly one maturity rung, and a
rung is unmet **iff** its lane's queue is non-empty — so the compass can never
point at a lane that would hand you nothing:

| Lane | Rung | Closes |
|---|---|---|
| seed | `seeded` | nothing is named yet |
| fix | `repaired` | a claim that was true has broken |
| build | `grounded` | intents with no code behind them |
| coverage | `covered` | registered files no intent owns |
| validate | `proven` | implemented behavior with no passing proof |
| quality | `measured` | rules never measured against the code |
| analyze | `inspected` | relationship claims nobody has judged |
| review | `reviewed` | verdicts recorded below the confidence floor |
| triage | `triaged` | findings awaiting a durable decision |
| prove | `investigated` | hypotheses nobody has tested |
| elaborate | `elaborated` | user-visible ideas only half-described |
| ratify | `converged` | where evidence and human judgment disagree |
| audit | `sound` | graph-integrity issues and open smells |
| export | `published` | the committed graph is behind the live one |

`fix` never records a verdict — it repairs at root cause and lets the owning lane
re-measure. `validate` **runs** proofs; it does not accept reported outcomes.

## Knowing what a change would break

```bash
loom impact <symbol|file> [--depth 3]
```

Walks callers backwards from the target and names the intents at risk plus how
well each is proven — "42 callers" is trivia unless it says what could silently
break. Resolution confidence is reported, never blended: a name matched by
exactly one definition is `exact`; several definitions means the nearest by file
proximity, marked `heuristic`. Calls into std or third-party code are counted as
unresolved rather than guessed at.

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

This repository is a greenfield rebuild of `loom`, mid-way through a hardcut to
the evidence spine described above.

**Working and exercised on codebases loom had never seen** (a 2.4G polyglot
monorepo, a 270-file Rust workspace, a Rust+Svelte service): cold-start
registration with glob suggestions, symbol extraction across five languages,
candidate-file proposal from a single sentence of intent, the call graph and
`loom impact`, proof runs that loom performs itself, pattern scans that make an
absence checkable, and evidence expiry that re-opens claims when code moves.

**Not yet done.** Three of six anchor floors are live (proof, quality, and the
locator probe behind grounding); the grounding, adjudication, relationship and
ratification floors are staged with their reasoning recorded in `src/anchor.rs`.
Derived proof strength, the ratification inversion, the `deepen`/`audit` lanes,
`loom absorb`, and the sync-ripple rewrite are designed but unbuilt — sync still
runs the old per-edge-kind matrix alongside the uniform `reverify_all` pass.
Sync ripple under real edits, the quality lane, and journeys have not been
exercised on foreign code.

Treat the staged constants and the module headers as the honest map: each says
what it does today and what it is waiting on.
