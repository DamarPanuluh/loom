# loom

`loom` is a CLI that keeps a falsifiable graph of what a codebase should do, where that behavior lives, and how it is proven.

The model supplies judgment and evidence. `loom` supplies durable memory, routing, staleness, coverage, integrity checks, and a portable export that can travel with the repository. The LLM still acts. `loom` routes, remembers, invalidates, and checks the work.

Install the binary, then use it in any repository. No skill, plugin, or library dependency is required.

## Install

Do **not** run `cargo install loom`. That crate is Tokio's concurrency tester, not this program.

### Prebuilt binary

Version tags `v*` publish archives to [GitHub Releases](https://github.com/DamarPanuluh/loom/releases). Pick the archive for your machine, unpack it, and put `loom` on your `PATH`:

```bash
# macOS Apple Silicon
curl -fsSL https://github.com/DamarPanuluh/loom/releases/latest/download/loom-aarch64-apple-darwin.tar.gz | tar -xz
# macOS Intel
# curl -fsSL https://github.com/DamarPanuluh/loom/releases/latest/download/loom-x86_64-apple-darwin.tar.gz | tar -xz
# Linux x86_64
# curl -fsSL https://github.com/DamarPanuluh/loom/releases/latest/download/loom-x86_64-unknown-linux-gnu.tar.gz | tar -xz

mkdir -p "$HOME/.local/bin"
install -m 0755 loom "$HOME/.local/bin/loom"
loom --version
```

Until a `v*` tag has been pushed, those URLs will 404. Use one of the source installs below.

### From git (Rust toolchain)

```bash
cargo install --git https://github.com/DamarPanuluh/loom.git --locked
loom --help
```

### From a checkout

```bash
git clone https://github.com/DamarPanuluh/loom.git
cd loom
cargo install --path . --locked
```

`scripts/release.sh` is the maintainer cut path; it also `cargo install --path . --force` onto the cutting machine.

## Quick start

Initialize a graph in a repository:

```bash
loom init .
loom welcome
```

Author behavior first as a semantic Journey:

```yaml
schema: loom.journey/v1
id: checkout
name: Complete checkout
actor: shopper
goal: Purchase a selected product and receive an accepted order.
inputs:
  sku:
    type: string
    description: The product selected by the shopper.
preconditions:
  - The product is available to purchase.
steps:
  - id: choose-product
    name: Choose product
    action: Choose a product to purchase.
    expects: []
    produces: {}
  - id: confirm-order
    name: Confirm order
    action: Confirm the order.
    expects:
      - The shopper receives an accepted order with a stable receipt identifier.
    produces:
      receipt:
        type: string
        description: The stable receipt identifier for the accepted order.
profiles:
  proof:
    inputs:
      sku:
        template: sku-1
    workspace: {}
```

Register the authored root, then inspect the proposed technical projection:

```bash
loom journey add journeys/checkout.yaml
loom journey derive checkout
```

`derive` is read-only. Its only acceptable projection is a strict `loom.journey-derivation/v1` JSON manifest. Review that exact manifest with the human; only their exact answer authorizes `derive-accept`. After the accepted Intents are implemented and grounded, `loom journey surface checkout` emits the contract for a real CLI in the target repository. Loom does not generate that source.

Map accepted behavior to code, attach a proof, then drive the loop:

```bash
loom codefile add 'src/**/*.rs'
loom edge implement "checkout captures payment" src/checkout.rs --locator "fn capture_payment"
loom validation add \
  --name "checkout payment capture test" \
  --type test \
  --command "cargo test checkout_captures_payment" \
  --intent "checkout captures payment"

loom sync
loom status
loom next --all
```

Use JSON for agent-facing output (`loom --json status`, `loom --json next`). Treat `loom --help` as the source of truth for the implemented command surface.

## What loom counts

Every asserted fact carries a **verification level**. The level is derived from evidence loom can independently re-examine — nobody writes it as a label.

| Level | Means | Reached by |
|---|---|---|
| **verified** | loom ran something and watched | `loom validation run` / `loom journey run`; a locator that re-resolves to a live symbol; a quality rule's patterns scanned by loom itself |
| **cited** | anchored to bytes or a journal entry | a `file:line` citation, fingerprinted and re-checked when the file changes |
| **claimed** | prose only | recorded in full, and it never satisfies a rung |
| **expired** | every anchor has since broken | what a `verified` fact becomes when the code it covered moves |

The type a caller may supply has **no way to express a `Run`**. Asking loom to run something is the only route to `verified`. Prose is still recorded; it just does not count.

Two consequences: an absence can be evidence (loom can scan a rule's patterns and record finding nothing), and proofs expire on their own (a later edit of a hashed file re-opens the claim).

The working model:

- **Journey** — the authored user or operator flow that roots downstream work.
- **Intent** — what the code is supposed to do.
- **CodeFile** — where behavior lives.
- **Validation** — how behavior is proven.
- **InterfaceSurface** — the real repository seam through which a Journey is exercised.
- **QualityRule** — what good looks like.
- **Edge** — the typed claim connecting those facts.
- **Fact** — the asserted state of a claim, and the evidence anchoring it.

## Storage

A repository using `loom` has two graph artifacts:

```text
.loom/graph.sqlite   # local SQLite working store
loom.graph.json      # deterministic portable export
```

Commit `loom.graph.json` when the graph should travel with the codebase. `.loom/` is local runtime state; portable setup such as layer order, ignores, codefile globs, and scan adapters lives in the export's `config` map.

## Driving loom as an agent

An agent that has never seen a loom graph can orient in three commands:

```bash
loom --help            # the implemented surface
loom welcome           # where this repo's graph is and what is next
loom guide             # which commands a lane may and may not run
```

Then the standing loop:

```bash
loom sync
loom --json status     # compass, rung ladder, queue depths
loom --json next       # the next routed work packet with its prompt contract
```

Every served packet names `allowed_actions`, `forbidden_actions`, `required_evidence`, a `write_back` command, and a `stop_condition`. Follow the packet, run the write-back, `loom sync`, and return to `loom status`.

`loom mcp serve` speaks MCP over stdio if you would rather pull context as a tool call:

```json
{ "command": "loom", "args": ["mcp", "serve", "--graph", "/path/to/repo"] }
```

An optional `loom-driver` skill may exist in a global skill root for batch orchestration. It is never required; it assumes this binary is already on `PATH`.

## Typical agent loop

```bash
loom sync
loom --json status
loom --json next --all
```

Every lane owns exactly one maturity rung, and a rung is unmet **iff** its lane's queue is non-empty:

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

`fix` never records a verdict — it repairs at root cause and lets the owning lane re-measure. `validate` **runs** proofs; it does not accept reported outcomes.

After graph mutations, `loom export` / `loom export --check` keep the portable file honest. `loom checkpoint recommend` is read-only: it names the exact local commit loom can justify, and never stages, commits, or pushes. This repository's opt-in local CI gate is `loom hook install --pre-push scripts/local-ci.sh`.

## Documentation

Start at [`docs/README.md`](docs/README.md). The compiled CLI help outranks prose.

Living operator docs:

```text
docs/commands.md           shipped CLI surface + removed/deferred names
docs/llm-driver.md         how loom and an LLM cooperate
docs/journey-authoring.md  Journey surface authoring and lint
```

Living model docs:

```text
docs/terminology.md        canonical vocabulary
docs/graph-model.md        nodes, edges, facets, schema, invariants
docs/state-machine.md      routing, ripple, lifecycle
```

`CHANGELOG.md` is the per-release record. Architecture seeds, the original ring plan, and working logs live under `docs/` as archive — they are labeled as such in the docs index.

## License

MIT. See the `license` field in `Cargo.toml`.

## Status

Pre-1.0. The on-disk graph is schema v12+ (Journey-root). Older SQLite graphs and exports are refused; there is no compatibility migration, because translating executable proof specs into authored meaning would fabricate product judgment. Rebuild with `loom init`, register code, use `loom bootstrap suggest` for repository clues, author `loom.journey/v1` roots, then `loom journey derive` and obtain the required human decisions.

Open work is tracked in the graph itself: `loom status --json` is the honest map.
