# loom

**A living intent graph for your codebase — externalized, falsifiable memory that an LLM agent can drive.**

loom is a CLI that helps LLM agents (and the humans steering them) systematically understand, build, and clean up codebases. It maintains a graph of *intents* — what each piece of code is supposed to do — grounded in real files, where **every relationship carries a verification status and evidence**. The graph is the durable memory; the agent's context window is just the working set.

There is no server and no GUI. The interaction surface is the CLI itself, and every command supports `--json`. The first dogfood target is loom's own codebase: the graph ships in this repo as [`loom.graph.json`](loom.graph.json).

## Why

LLM agents are good at local reasoning and terrible at remembering what they verified last week. Code review tools tell you what changed; nothing tracks *which understandings a change invalidated*. loom makes that mechanical:

- Claims about code are **edges with a state machine** (`uninspected → passing | failing | independent ↔ needs_reverification`), not vibes.
- When code changes, `loom sync` flips exactly the affected claims back to stale — **the graph structure is the impact analysis**.
- "Looks green" is not enough: verdicts require a falsifiable criterion, substantive evidence, a confidence, and provenance. `loom doctor` audits all of it after the fact.

## The mental model

The graph spans three planes, connected by edges:

| Plane | Node | Meaning |
|---|---|---|
| Semantic | `Intent` | what the system is *supposed* to do (system → component → feature) |
| Physical | `CodeFile` | what actually exists on disk |
| Normative | `QualityRule` | what *good* looks like (e.g. the bundled ISO 5055 pack) |

Plus `Validation` nodes — runnable proof objects (tests, benchmarks, manual checks) attached to intents.

Five edge types: `HIERARCHY` (the intent tree), `IMPLEMENTS` (intent → file, with a symbol-level locator), `RELATES_TO` (intent ↔ intent, the inspectable grid), `GOVERNS` (rule → intent, the quality gate), `VALIDATES` (proof → intent).

**Done** is mechanical, not felt: the *vertical spine* must hold — the hierarchy is a well-formed tree, every implemented leaf intent is grounded in code, every file is reached by an intent, and `loom coverage` reports nothing unaccounted. Closing the horizontal intent×intent grid is optional deep-understanding work.

## Quick start

```bash
cargo install --path .        # pure Rust, embedded graph DB, no server

cd your-repo
loom init .
loom guide                    # the full driving protocol, self-taught by the binary
loom detect                   # greenfield or brownfield?

# map: seed intents, link the tree, ground to files
loom intent add --name "request routing" --description "..." --level component
loom codefile add 'src/**/*.rs'
loom edge implement "request routing" src/router.rs --locator "fn route"

# then loop:
loom status                   # compass: where you are + the next action
loom next                     # the single highest-priority work item, full context
loom sync                     # after ANY code change: flips stale claims
loom next --all               # closeout: every queue, every gap, one list
```

Everything is addressable by id, exact name, or unique name fragment. Ambiguity is an error, never a guess.

## The loop

1. `loom next` hands you one edge with both intents, the code locations, prior notes, and a suggested action.
2. Work it Socratically: form a hypothesis, read the actual code, then record the verdict — `ground` (passing, with criterion), `issue` (failing, with evidence), or `independent` (verified *no* relationship — as valuable as a pass).
3. After any code change: `loom sync`. Change detection is **content-hash based** (checkout/rebase mtime churn never false-flags), and every invalidated edge gets a note naming the file that staled it.
4. `loom smells` surfaces what nobody asserted: twin intents, overlapping ownership, scattered responsibilities (clustered by directory), tangled files, undeclared coupling (imports the graph doesn't explain), recurrent trouble. Each finding carries its exact remedy command.
5. Close out with `loom next --all`, prove intents with `loom validate`, earn quality green with `loom rule verdict`.

## Multi-agent by design

Every schema field declares its owning **role** — builder, analyzer, fixer, validator, quality — and each role has its own `loom next --mode …` queue. Declare a role (`LOOM_AGENT=llm:analyzer`) and loom **enforces the lane**: a builder cannot green-light its own work; verdicts recorded out-of-lane are hard errors, and `loom doctor` audits provenance after the fact. Bare `llm` is solo mode — one agent drives every lane, all gates still apply.

Topology is yours: one agent switching hats, sequential handoffs, or parallel agents per lane. Handoff happens **through the graph**, not through chat.

## The graph travels with the repo

`.loom/` is a local binary cache (gitignore it). The committed artifact is `loom.graph.json` — a **deterministic export** (same graph → identical bytes), so graph changes are diffable in PRs and `loom export --check` can gate CI/pre-commit: it exits non-zero whenever the committed export drifts from the live graph. Rebuild anywhere with `loom init . && loom import loom.graph.json && loom sync`.

## Honesty primitives

A few states exist specifically so the graph never lies by omission:

- `independent` — "we looked; there is no relationship" (verified absence, not silence)
- `needs_change` — a known issue/refactor, flagged without faking a verdict
- `blocked` — a proof that *can't* run yet (live target down, missing credential), recorded with a reason; out of the work queue but visible in `loom report`, and a code change doesn't quietly reset it
- `loom ignore add <glob> --reason` — coverage exclusions live *in the graph*, with a recorded why

## Building from source

```bash
cargo build          # first build is slow (grafeo statically linked); incremental is seconds
cargo test           # query-layer regression suite, in-memory DB
.claude/skills/run-loom/driver.sh   # full lifecycle smoke: map → ripple → quality → export/import
```

Requires only the Rust toolchain. The graph store is [grafeo](https://crates.io/crates/grafeo), embedded and pure Rust.

## Project status

Actively developed and dogfooded — loom's own intent graph is maintained by LLM agents driving this CLI, and several of its features (the DB-lock release during validations, the O(N²) discovery-count fix, the evidence gates) came out of loom flagging issues in itself. See [`CLAUDE.md`](CLAUDE.md) for the full design document and command reference.
