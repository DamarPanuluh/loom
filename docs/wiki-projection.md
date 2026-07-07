# loom v2 — Wiki Projection

Status: **historical design reference.** The wiki shipped in v0.20.0 with a deliberately smaller surface than this doc designs: `loom wiki plan/next/record/list/remove` (see `commands.md`), where an agent writes the prose and loom governs only truth and freshness — there is no generate/verify/publish run pipeline, no `.loom/wiki-runs/` isolation, and no WikiManifest. The **Core rule** below (the wiki is a projection; the graph is the source) is what shipped and remains canonical; the run/manifest/citation machinery in the rest of this doc is the fuller design it was distilled from, kept as reference for a future deepening.

---

## Core rule

```text
Wiki docs explain the graph. They never replace it.
```

The graph is the canonical source of truth. The wiki is a **projection** — a rendered view derived from graph facts. A wiki claim that cannot be traced to a graph fact is either wrong, outdated, or a proposal that needs to enter the graph.

Implications:

- Editing a wiki page does not change product meaning.
- A wiki prose change that contradicts the graph is a graph question, routed through `InboxItem` normalization.
- A stale wiki page is not a lie — it is a signal that the graph changed and the page has not caught up.
- The wiki can be regenerated entirely from the graph at any time.

---

## Layer split

```text
.loom/graph.sqlite          runtime graph + operational state
loom.graph.json             committed portable graph export
.loom/wiki-runs/<run-id>/   isolated preview runs (never published until verified)
docs/loom/**                published human/agent wiki
AGENTS.md / skills          tool entrypoints — point to docs, never duplicate them
```

The wiki never competes with the graph as a fact layer. There is no `ai/source-of-truth/` YAML layer sitting between the graph and the wiki. The graph is the source.

---

## WikiManifest

Every wiki run produces a manifest alongside the pages.

```text
WikiManifest
  run_id
  graph_export_hash       hash of loom.graph.json at generation time
  git_commit              repo commit at generation time
  generated_at
  pages[]                 ordered page list
  nav_tree[]              hierarchical navigation structure

WikiPage
  page_id
  title
  slug
  page_type:              overview | architecture | intent_map | interface | validation |
                          quality | operations | domain | module | decisions | index
  output_path
  parent_page_id
  child_page_ids[]
  depends_on[]            typed graph/code dependency refs
  citations[]             evidence locators used in generated prose
  last_verified_at
  stale:                  bool
```

The manifest is stored in SQLite for incremental queries and emitted as `wiki-manifest.json` alongside the pages.

---

## Document center structure

The wiki is a document center, not a module dump. Pages are organized for reader comprehension, not for code directory structure.

```text
docs/loom/
  index.md                reading order, quick orientation
  overview.md             what this repo is, problem, capabilities, how to navigate
  architecture.md         system layers, main seams, core flows, Mermaid diagrams
  intent-map.md           intent hierarchy, domains, gaps, atomic intents
  interfaces.md           API/CLI/UI/message surfaces, consumers, contracts
  validation.md           proof matrix, coverage, blocked/failing proofs
  quality.md              quality rules, hardening status, unresolved risks
  operations.md           build/test/run/release facts, deployment notes
  decisions.md            decision notes, retired/superseded intents

  domains/
    <domain>/             topic section for a business domain or capability area
      index.md

  modules/
    <module>.md           per-file or per-code-area drilldown

  wiki-manifest.json      machine-readable page plan and nav tree
```

### Page roles

| Page | Role | Prose-first? |
|---|---|---|
| `overview.md` | What this repo is, entry point for all readers | Yes |
| `architecture.md` | System design, layers, seams, main flows | Yes + Mermaid |
| `intent-map.md` | Behavior hierarchy, domains, gaps | Hierarchy + table |
| `interfaces.md` | Public surfaces, consumers, contracts | Table + prose |
| `validation.md` | Proof matrix, coverage status | Table + gaps |
| `quality.md` | Rule verdicts, risks, unresolved | Table + prose |
| `operations.md` | Build/run/deploy facts | Code + prose |
| `decisions.md` | Decision log, history | Append-only |
| `domains/<d>/` | Reader task flow for one domain/capability | Yes |
| `modules/<m>.md` | Code drilldown, locators, verdicts | Reference |

`overview.md` and `architecture.md` must be **prose-first**: paragraphs, not lists. They should answer:

```text
what is this system?
what problem does it solve?
what are the main flows?
where should a reader go next?
```

Module pages are drilldown. They are not the front door.

---

## Citations

Every non-trivial factual claim in a wiki page must cite a graph fact.

```text
citation
  target_id:    node id, edge id, or codefile path
  target_type:  intent | edge | codefile | validation | rule | surface | note
  locator:      optional file:line for code-level citation
  anchor_text:  the claim in the page that this evidence backs
```

### Wrong

```text
The authentication system is robust and handles all failure cases.
```

### Right

```text
Authentication is modeled as three intents: password login, session creation, and
remember-me restoration. Password login is realized in `src/auth/password.rs`
[Intent: user can log in with password → implements(role=realizes) src/auth/password.rs]. Session
creation is validated by `login_session_test` [Validation: login_session_test → passing].
Remember-me restoration currently lacks a browser-restart journey [Intent: remember-me
token restores session → validates: none].
```

The citation model is:

```text
graph fact → cited sentence / table row / diagram node
```

Not:

```text
LLM writes plausible architecture prose from memory
```

---

## Page dependency tracking

Each wiki page records its graph/code dependencies. After `loom sync` or a graph change, only pages whose dependencies changed need regeneration.

```text
depends_on entry types:

  {"intent": "<id>"}               stale when intent meaning/lifecycle changes
  {"edge": "<id>"}                 stale when edge status or evidence changes
  {"codefile": "<path>"}           stale when file hash changes
  {"validation": "<id>"}           stale when Validation.last_result changes
  {"rule_verdict": "<edge-id>"}    stale when governs edge status changes
  {"surface": "<id>"}              stale when InterfaceSurface identity/contract changes
  {"note": "<id>"}                 stale when note text changes
  {"graph_export_hash": "<hash>"}  provenance record only — does not by itself mark a page stale;
                                   a manifest-level hash mismatch triggers a warning and recommends
                                   a full rebuild, but incremental page staleness is evaluated
                                   via each page's typed depends_on refs
```

### Incremental invalidation examples

```text
src/auth/session.rs changed
  → all pages depending on {"codefile": "src/auth/session.rs"} → stale

Intent: remember-me restores session (description changed)
  → pages depending on {"intent": "<id>"} → stale

Validation: login_session_test result changed (passed → failed)
  → pages depending on {"validation": "<id>"} → stale

governs verdict for service-auth-at-boundary changed
  → pages depending on {"rule_verdict": "<edge-id>"} → stale
```

Regenerate only stale pages. Full rebuild is always available as fallback.

---

## Preview and publish flow

Never write directly to `docs/loom/**` during generation. Use an isolated run first.

### Preview

```text
loom wiki plan
  → build WikiManifest: page plan, nav tree, depends_on for each page

loom wiki generate --preview [--run-id <id>]
  → render pages with citations to .loom/wiki-runs/<run-id>/content/
  → write manifest to .loom/wiki-runs/<run-id>/wiki-manifest.json
  → write verify result stub to .loom/wiki-runs/<run-id>/verify.json
```

### Verify

```text
loom wiki verify --run <run-id>
  → run hard and soft checks (see below)
  → emit .loom/wiki-runs/<run-id>/verify.json with pass/warn/fail per check
```

### Publish

```text
loom wiki publish --run <run-id>
  → only if verify passes hard checks
  → copy content to docs/loom/**
  → update wiki-manifest.json in docs/loom/
  → record published run in operational SQLite
```

### Update (incremental)

```text
loom wiki update
  → identify stale pages from dependency tracking
  → regenerate only stale pages into a new preview run
  → verify
  → publish
```

---

## Verify gates

### Hard checks (must pass before publish)

```text
required pages exist
  overview, architecture, intent-map, interfaces, validation, quality, decisions, index

manifest paths resolve
  every page_id in nav_tree has a file in content/

markdown links resolve
  internal links point to existing pages
  anchor links point to existing headings

citations resolve
  every citation target_id exists in the graph (not deleted/missing) — hard fail
  citations to deprecated nodes are allowed when: the page type is decisions/history,
  or the citation explicitly marks the target as deprecated; warn but do not hard-fail
  citations to deleted/missing nodes are always a hard failure

page not stale
  determined per-page from its depends_on refs and their fingerprints;
  a graph_export_hash mismatch between manifest and current export is a manifest warning
  (recommend full rebuild) but does not override per-page freshness evaluation

no fabricated facts
  no claim in prose references a graph entity that does not exist
  (checked via citation coverage: every factual claim has at least one citation)

adapter files point correctly
  AGENTS.md / skills reference real docs/loom/** paths
```

### Soft checks (warn but do not block publish)

```text
overview and architecture are prose-first
  minimum prose paragraph count per page

API/interface pages aggregate by domain
  not a raw endpoint dump

module pages link up to domain and intent pages
  navigation from drilldown back to higher-level context

Mermaid diagrams present
  at least one diagram in architecture.md

nav has top-down path
  every page reachable from index.md via ≤ 3 hops

prose density above threshold
  no page is >80% lists with <20% prose

no empty pages
  no page with only a heading and a placeholder
```

---

## Adapter files

Tool entrypoints (AGENTS.md, skills, CLAUDE.md) should **point to docs, not duplicate them**.

Good:

```text
AGENTS.md
  Read docs/loom/overview.md for context.
  Read docs/loom/architecture.md before changing system design.
  Use loom status to orient.
  Use loom find <topic> to locate behavior.
  Use loom next for queued work.
  See docs/loom/wiki-manifest.json for full navigation.
```

Bad:

```text
AGENTS.md contains a copy of architecture prose
  → diverges from docs immediately on next graph change
```

The adapter file is a reading-order pointer and a loom command cheat sheet. It is not a wiki page.

---

## Viewer (deferred)

A static local viewer with tree navigation and Mermaid rendering is useful but not MVP.

When built, the viewer must:

- load `wiki-manifest.json`, not scrape directories
- support collapsible nav tree from `nav_tree`
- render Mermaid diagrams
- support heading anchor navigation
- open historical snapshots from `.loom/wiki-runs/`

Not MVP. Include it only after the graph model, state machine, LLM driver, and wiki projection are stable and dogfooded.

---

## Design rules

1. **Wiki docs explain the graph; they never replace it.**
2. **Every factual claim cites a graph fact.** Uncited claims are either prose connective tissue or fabrications.
3. **Semantic wiki changes enter the graph through InboxItem normalization.** Never directly.
4. **Stale pages are honest.** They signal graph/code changed, not that loom lied.
5. **Publish requires verify.** No bypass.
6. **Adapters point; they do not duplicate.** AGENTS.md/skills are reading-order pointers.
7. **Page count follows graph shape.** No arbitrary target page count.
