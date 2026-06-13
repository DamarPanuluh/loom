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

The graph spans five planes, connected by edges:

| Plane | Node | Meaning |
|---|---|---|
| Semantic | `Intent` | what the system is *supposed* to do (system → component → feature) |
| Physical | `CodeFile` | what actually exists on disk |
| Normative | `QualityRule` | what *good* looks like (e.g. the bundled ISO 5055 pack) |
| Proof | `Validation` | runnable evidence an intent is fulfilled (tests, benchmarks, manual checks, consumer sagas) |
| Pre-decision | `Hypothesis` | improvement proposals — proven against the code *before* they become work |

Six edge types: `HIERARCHY` (the intent tree), `IMPLEMENTS` (intent → file, with a symbol-level locator), `RELATES_TO` (intent ↔ intent, the inspectable grid), `GOVERNS` (rule → intent, the quality gate), `VALIDATES` (proof → intent), `TARGETS` (hypothesis → the intents it would touch).

**Done** is mechanical, not felt: the *vertical spine* must hold — the hierarchy is a well-formed tree, every implemented leaf intent is grounded in code, every file is reached by an intent, and `loom coverage` reports nothing unaccounted. Closing the horizontal intent×intent grid is optional deep-understanding work. And green carries an **audit gate**: once every queue is dry, the compass stays at `phase=audit` until `loom smells` returns zero open findings — every suspicion the graph computed must be resolved or *refuted* (an `independent` verdict or a decision note counts exactly as much as a fix).

Progress toward done is a counted **360° coverage vector**, printed under every status/next footer: `grounded · realized · explored · measured · proven` — files explained, leaves coded, the grid inspected, quality rules held against coded intents, validations passed. The compass always points at the weakest axis.

## Quick start

```bash
cargo install --path .        # pure Rust, embedded graph DB, no server

cd your-repo
loom init .
loom guide                    # the full driving protocol, self-taught by the binary
loom detect                   # greenfield or brownfield? + which quality packs fit this repo

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
4. `loom smells` surfaces what nobody asserted: twin intents, overlapping ownership, scattered responsibilities (clustered by directory), tangled files, undeclared coupling (imports the graph doesn't explain), layering violations (imports pointing UP the declared `loom layer order` over intent `--layer` labels — a recorded relationship doesn't excuse direction), recurrent trouble, happy-path-only features (no failure behavior declared), duplicated responsibility (tag collisions across unrelated code, with a weaker lexical fallback for under-tagged coded pairs), vocab drift, unjourneyed surface (a user-visible behavior no consumer journey ever exercises end-to-end). Each finding carries its exact remedy command — and the redesign-shaped ones emit a `loom hypothesis add`, so a redesign gets *proven* before it becomes work. Open findings gate green: `phase=complete` requires zero, and adjudications (decision notes) re-open automatically when the structure changes under them.
5. Seed the quality packs `loom detect` recommends — `iso5055` (baseline, any code), `mobile`, `web-ui`, `service`, `data`, `concurrency` — and `loom next --mode quality` serves every rule × coded-intent pair never measured. One `loom rule verdict` resolves each (it creates the edge; a verdict at component altitude covers descendants; `independent` = measured, doesn't apply).
6. Close out with `loom next --all`, prove intents with `loom validate`.

## The user drifts too: seeding & alignment

`loom sync` catches intent↔code drift mechanically — but a graph can be perfectly green and perfectly wrong, faithfully describing a product the user no longer wants. That third axis (user↔intent) can't be hashed; it's caught by **interview**, and loom makes the interview mechanical too:

- `loom guide --mode seed` teaches the elicitation protocol: calibrate altitude to the user's fluency (a vague user gets *proposals to react to*, never "enumerate your features"), one question per landing (every crystallized answer immediately becomes a `planned` intent, a vocab term, or a decision note), and **terminate on completeness, not exhaustion** — every question must close a gap the graph can enumerate; no open gap, no more questions. An empty graph's compass starts here (`phase=seed`).
- `loom intent confirm` stamps "the user re-affirmed this meaning as of now"; `loom next --mode align` ranks intents by **churn-since-confirm × centrality × staleness** — code that moved under a meaning nobody re-affirmed — and serves the top suspect as one interview move with exactly four outcomes: `confirm` (resets the clock), `intent update` (evolved), `retire --replaced-by` (superseded), `add` (revealed gap).
- `loom intent update` is design **evolution** in place — same node, same history. A description change is a *redefinition* and ripples one hop, the semantic twin of `sync`: every verdict earned against the old wording (including the IMPLEMENTS grounding — "does the code still do what this *now* says?") goes `needs_reverification`, linked proofs go `not_run`, and the old wording is preserved in a decision note. Iterate freely while intents are `planned` — there's nothing downstream to stale; after grounding, every meaning change costs re-verification, which is the point.
- `loom door "<utterance>"` is the **entrance**: users don't speak in graph nouns, and their process isn't linear — a story, a complaint, a norm, a question, in any order. The door never interprets; it assembles the routing context mechanically (what every plane already knows about the topic: intents by BM25, vocab/sagas/rules by token overlap, plus the compass pulse) and returns the **landing menu** — the total enumeration of ways an utterance becomes a graph noun, each an existing command. The door advises, never blocks: state lives in the graph, not the conversation, so any noun lands at any time and the queues re-derive.
- `loom session` is **turn zero before any utterance** — the door's complement: the user said "use loom" and stopped. It returns the ask-the-user playbook: ONE question ("what do you want from this session?") plus a state-aware **offer menu** — each offer backed by a live queue and its count (re-check alignment? rule on proven proposals? unblock proofs? keep building? propose a saga? close gaps? or "just get to work") — with exactly one marked recommended. The order encodes the scarcity of the user's presence: queues only the user can answer outrank everything the agent drains alone. Works even before `loom init` (restore the committed export › map the code › interview).
- The conversation oscillates between interactive and autonomous work, so every queue carries a **gate**: `autonomous` (an agent drains it alone) or `human` (align drift suspects, adopt/reject rulings, blocked proofs). `loom status` and `loom next --all` total the human-gated remainder — the agent drains autonomous queues now and batches what needs the user into *one* agenda for the next conversation window, instead of dribbling questions.

## Prove it from the outside: consumer sagas

Everything above grounds claims by *reading* code. A **saga** proves intents compose by *executing* them — an ordered chain of endpoint invocations that consumes the system the way a real consumer will, with values captured from one response threading into the next request. The engine is built in and pure Rust (reqwest on rustls — no libcurl, loom stays one static binary):

```yaml
saga: checkout-flow
base: "{{ env.BASE_URL }}"
steps:
  - name: create cart
    intent: cart-creation              # every step names the intent it proves
    request: { method: POST, url: /carts, json: { items: [] } }
    expect:  { status: 201 }
    capture: { cart_id: "$.id" }       # JSONPath → variable for later steps
  - name: capture payment
    intent: payment-capture
    request: { method: POST, url: "/carts/{{ cart_id }}/payment" }
    expect:  { status: 200, body: { "$.state": paid } }
```

`loom saga add checkout.saga.yaml` declares the proof; `loom saga run checkout-flow` executes it and stamps the result into the graph with honest failure semantics: consecutive passing steps mark their `RELATES_TO` edge **passing with runtime evidence**; the boundary into a failing step goes **failing with the exact broken expectation** (`expected 200, got 502`) and lands in the fix queue; steps after the failure stay untouched — *never reached* is not *failing*. `loom sync` re-queues the saga whenever code behind a step intent changes, and the spec travels verbatim across a port — it speaks HTTP, not the implementation language.

The relation is **bidirectional**: journeys prove intents, and journeys *create* intents. With `loom saga add <spec> --spawn-missing [--under <parent>]`, a step may name an intent that doesn't exist yet — it spawns as a planned, user-visible feature: the user narrates the story, the steps become the design, the build queue realizes them, and the saga is their acceptance test. The converse direction has teeth too: the `unjourneyed_surface` smell flags any user-visible intent no journey exercises.

## Ideas are not work: the hypothesis plane

An improvement idea must survive contact with the code before it costs anything. `loom hypothesis add` records a falsifiable **claim** (what's wrong *now*), a **proposal**, and a **predicted outcome**; a *different* agent proves or refutes the claim against the code (`loom next --mode prove` ranks proposals by blast radius); only then does a builder **adopt** it into planned intents — and the predicted outcome becomes a real validation, so every adopted improvement is later checked for whether it actually *delivered* (`confirmed`). Unproven ideas die honestly (`rejected`) instead of becoming speculative refactors. Hypotheses are invisible to coverage and completeness until adopted — speculation never counts as the state of the world.

## Multi-agent by design

Every schema field declares its owning **role** — builder, analyzer, fixer, validator, quality — and each role has its own `loom next --mode …` queue. Declare a role (`LOOM_AGENT=llm:analyzer`) and loom **enforces the lane**: a builder cannot green-light its own work; verdicts recorded out-of-lane are hard errors, and `loom doctor` audits provenance after the fact. Bare `llm` is solo mode — one agent drives every lane, all gates still apply.

Capability is tiered, honestly: every work item carries `effort: low|mid|high` — a statement about the *work* (loom never names models; the harness maps tiers). Cheap agents drive the bulk and record honest confidence; any verdict below 0.7 automatically feeds `loom next --mode review`, where a stronger agent independently re-inspects (own hypothesis first, then the recorded evidence) and confirms or overturns. Confidence is the coordination channel between tiers — no agent ever messages another.

Topology is yours: one agent switching hats, sequential handoffs, or parallel agents per lane. Handoff happens **through the graph**, not through chat.

The CLI itself is built for that driver: every state-changing command ends with the next runnable command plus a one-line graph pulse (same fields in `--json` as `next_step` + `graph_state`), every list is bounded (`--limit`, default 50) with explicit `+N more — <command>` markers, and every error names its corrective command — so an agent recovering from a compacted context can re-orient from any single output.

## The graph travels with the repo

`.loom/` is a local binary cache (gitignore it). The committed artifact is `loom.graph.json` — a **deterministic export** (same graph → identical bytes), so graph changes are diffable in PRs and `loom export --check` can gate CI/pre-commit: it exits non-zero whenever the committed export drifts from the live graph. Rebuild anywhere with `loom init . && loom import loom.graph.json && loom sync`. Import is **two-phase**: a corrupted export is rejected loudly and never leaves a partial graph.

**Porting** is the travel format's second job: `loom import <source-export> --as-planned` adopts another repo's graph as a *design* — intents, hierarchy, criteria, rules, and notes travel; groundings, verdicts, and proof results don't (they were earned against the old code). Every intent arrives `planned`, every proof `not_run`, and the build queue drives re-realization in the new language — the criteria written for the old code become the acceptance contract for the new. `loom guide --mode port` teaches the loop.

Commands target the current directory's graph by default; scripts and orchestrators can **pin** one explicitly — `--graph <path>` or `export LOOM_GRAPH=<path>` — so a stray `cd` can never land a mutation in the wrong repo's graph.

## Honesty primitives

A few states exist specifically so the graph never lies by omission:

- `independent` — "we looked; there is no relationship" (verified absence, not silence)
- `needs_change` — a known issue/refactor, flagged without faking a verdict
- `blocked` — a proof that *can't* run yet (live target down, missing credential), recorded with a reason; out of the work queue but visible in `loom report`, and a code change doesn't quietly reset it
- `loom ignore add <glob> --reason` — coverage exclusions live *in the graph*, with a recorded why
- `loom vocab` — a bounded tag vocabulary (≤3 tags per intent, registered terms only). Open prose rarely collides; a small shared keyspace does — so two intents tagged `retry` in *unrelated* files surface as `duplicated_responsibility` even when no file, import, or wording connects them. Collisions are rarity-weighted (spammed broad terms decay to zero), tagging is optional at write time (untagged is honest; a wrong tag lies), and drift converges with `loom vocab merge` instead of being prevented by a closed list. Untagged coded pairs still get a stricter lexical fallback, but tags remain the high-signal detector; `loom smells` discloses how many coded intents are untagged and emits `duplicate_detection_unarmed` when coverage is under-armed, so a quiet report is never mistaken for proof of no duplication.

## Monorepos & federation

Graphs compose. Every graph has an identity (`loom init --name grid`; the id travels in its export), and a root graph can **delegate** service subtrees to their own looms:

```bash
# at the monorepo root
loom delegate add 'services/grid/**' --to services/grid/loom.graph.json
```

Root-level `loom coverage` then buckets those files as *delegated* — covered by the child, verified against its committed export (a missing export is flagged, never silently trusted). Ground the root's *seam* intents (service↔service contracts) in the children's exports and `loom sync` ripples cross-service automatically: when a child re-exports, every seam claim grounded in it goes stale. Data flows **up only** — children export, the parent observes; a parent never writes into a child's graph.

For code you *don't* own (a vendor SDK, an upstream dependency, another team's service), map it with `loom init --observed`: discovery, quality measurement, and validations all work, but the **custody gate** disables build/fix lanes — an observer records findings, never fixes — and its export is marked as observer testimony.

## Building from source

```bash
cargo build          # first build is slow (grafeo statically linked); incremental is seconds
cargo test           # query-layer regression suite, in-memory DB
.claude/skills/run-loom/driver.sh   # full lifecycle smoke: map → ripple → quality → export/import
```

Requires only the Rust toolchain. The graph store is [grafeo](https://crates.io/crates/grafeo), embedded and pure Rust.

## Project status

Actively developed and dogfooded — loom's own intent graph is maintained by LLM agents driving this CLI, and several of its features (the DB-lock release during validations, the O(N²) discovery-count fix, the evidence gates) came out of loom flagging issues in itself. See [`CLAUDE.md`](CLAUDE.md) for the full design document and command reference.
