# loom v2 — Commands

Status: **shipped CLI surface** — this page follows the compiled `target/debug/loom --help` tree. Names listed under "Removed / deferred names" are intentionally not current commands.

---

## Orientation

```text
loom welcome [--json]
```

Plain-English orientation: what loom is plus the one thing to do next. This is also the default — a bare `loom` with no subcommand routes here.

```text
loom status [--json]
```

Graph identity, integrity, maturity ladder, queue counts, validation summary, code ownership, and the compass. Doctor issues make integrity `INVALID`, block every non-audit rung, and route the compass to audit before any other work. Code ownership always reports the full registered denominator split into owned, unowned, excluded, and observed files; exclusions are grouped by recorded reason and never presented as owned coverage. The Review lane has two autonomous variants: `graph_state.low_confidence` counts verdicts below the configured confidence floor, while `graph_state.adversarial_review` counts unchallenged verdict revisions in the bounded risk frontier. `inconclusive_challenges` is historical/current review residue rather than new work; `review_independence_warnings` is non-blocking profile-attribution debt. `graph_state.open_questions` is the count of open first-class `Question` nodes.

`loom status` prints a true per-queue backlog line, including Journey `derive` and `surface` work alongside `fix`, `validate`, `build`, `coverage`, `quality`, `analyze`, `prove`, `triage`, `review`, and `elaborate`. Counts come from the same partition that `loom next` serves. In JSON mode the output gains a `queues` object with the same counts, including human-only `ratify` work.

```text
loom session [--json]
```

Turn-zero entry when the user says "use loom" without a specific task. Returns an offer menu backed by live queue counts, open questions, and one recommended command.

```text
loom next [--mode <queue>] [--all] [--json]
```

Highest-priority `WorkItem` + `PromptContract` for the current queue. Without `--mode`, routes by compass priority. The `ratify` queue is human-decision work and is NEVER served by plain `loom next`, so an autonomous loop is not interrupted by a product question. `loom next --mode ratify` returns a structured host gate: Keep, Remove, or Revise, plus recommendation guidance and exact write-backs. The LLM presents and recommends, waits for the human, then records that answer.

Closure invariant (uniform adjudicability): every served packet's `write_back` names the runnable loom command(s) that close it, and — for every lane whose closure is a graph write — that command accepts the packet's own target (id, short-id prefix, name, or edge endpoints). `fix` and `audit` packets close through state re-reads (`loom sync` / `loom audit --json`), so their closeout names the command without a target argument. An item whose closure cannot be named is a loom defect, not work: plain `loom next` skips it, `loom next --mode <m>` refuses with the defect named, and either way it is journaled as `unservable_packet` (mode, target, problem, write_back) — grep the journal for that kind to find contracts that need repair.

```text
--mode: derive | surface | build | coverage | fix | analyze/discovery | validate | quality | prove | triage | review | elaborate | rectify | ratify
--all:  closeout view — the top item of every queue at once
--mode <m> --all:  the FULL depth of one queue — every item it would serve, in
                   priority order (entry 1 is what `loom next --mode <m>` serves),
                   as lightweight rows (target + reason + effort, no packet). Use
                   it to page a queue that `loom status` reports as hundreds deep;
                   work an item with the singular `loom next --mode <m>`.
```

Queue partition is deliberately disjoint:

- `derive`: authored Journey steps without a current accepted technical mapping, plus unrooted non-exempt Intents. The packet proposes a hash-bound manifest and stops at the human gate.
- `surface`: a Journey whose accepted Intents are implemented and realizing-grounded but which lacks a current complete CLI surface. The builder writes real repository source and accepts structured operation bindings.
- `fix`: every failing asserted edge — strictly root-cause repair. A fix packet never carries verdict authority: repair the source, run `loom sync`, and the owning lane re-measures.
- `analyze`: uninspected and stale non-`governs`/non-`validates` asserted claims, plus open research TaskRecords. Stale claims are served first; bounded external research follows before ordinary uninspected claims. Two ownership rules ride on top of the kind list. Compiler-owned Journey proof topology is excluded whatever its kind: a `proves`/`validates`/`calls`/`exercises` edge out of a Validation that `journey compile` created is inspected only by `journey compile/run`, so serving it here would name a `loom edge verdict` write-back the CLI refuses — those route to `validate`. An ordinary `exercises` edge is evidence provenance rather than a claim while it stands, so an uninspected one is not queued; once sync invalidates it (a drifted locator) it stops counting toward proof strength and no other lane can settle it — a `validation run` records verdicts on `validates` edges alone — so a stale one IS analyze work, closed by `loom edge verdict` on the edge. Sync detects that drift even on never-inspected provenance: a generic `exercises` locator that stops resolving (symbol or anchor form) is moved to `needs_reverification` with its cause, so the repair reaches a lane instead of the proof re-running forever.
- `quality`: uninspected or stale `governs` only. Failing `governs` routes to `fix`.
- `validate`: uninspected or stale `validates`, plus every uninspected or stale edge of a compiler-owned Journey proof closure (`proves`/`calls`/`exercises` included) folded into one work unit per Validation, whose write-back is `loom journey compile/run`. Failing `validates` routes to `fix`.
- `coverage`: registered codefiles with no live realizing owner. One intent may realize in many files (sibling slices): if this file implements a slice of an existing criterion, ground `--role realizes` here. `consumes` / `configures` / `verifies` never own the file. If a distinct criterion lives here and no intent names it, record `discovered_behavior` and stop — do not mint in coverage. If the file is missing from disk, the packet is a dedicated missing-file contract: re-ground any successors, then unregister the dead registration — do not attempt to read a ghost.
- `review`: first serves asserted `passing` or `independent` verdicts with `0 < confidence < policy.review_confidence_floor`, lowest confidence first. It then serves the unclosed portion of a fixed, policy-bounded adversarial frontier over otherwise-green Verdict revisions. The frontier is selected before challenged rows are removed, so the driver cannot silently walk the whole graph. Adversarial packets carry `review.variant=adversarial`, the exact target Verdict fact id, risk score, and the prior executor profile to avoid when known. The reviewer forms a falsification hypothesis before reading prior evidence and records exactly one `Challenge` outcome (`survived`, `counterexample`, or `inconclusive`) for that revision; a changed Verdict snapshot reopens the edge. A counterexample atomically creates an untriaged Finding and never rewrites the Verdict directly.
- `elaborate`: the most-incomplete user-visible feature intent by Definition-of-Complete scorecard. The packet tells the LLM to proactively explain that a partial idea is enough, fill technical/repository-derivable gaps, and translate a true product decision into ONE plain-language question. Evidence of unnamed wantedness (a sad path, missing gate, or unauthored rule the code already enforces) is offered as one Keep / Decline / Revise question; mint or ratify only after the human answers. Completeness surroundings of an already-wanted idea may still be minted as planned intents. It records the Question, asks the user directly, waits rather than inferring consent, records the answer, then resumes. The packet also routes missing scenarios, prerequisites, proofs, and Journey ancestry. An Intent is either rooted by a current accepted derivation or deliberately Journey-exempt through a separate human decision.
- `rectify`: live, re-derived structural friction before human ratify. Duplicate-intent clears are pair decisions tied to the content hash of both descriptions, so unrelated writes do not resurrect them and rewording either intent does. Discovered-behavior entries remain observations of the current graph: a structural write can create a new witness and legitimately raise the count mid-drain. Treat the queue as live work, not a fixed settle snapshot.

Fixer lane safety: fix the source and run `loom sync`; sync re-opens the claim (`needs_reverification` plus any `stale_cause` facet), and the owning lane re-measures it. When the failing claim is compiler-owned Journey proof topology, sync alone cannot re-measure it — the packet also names `loom journey compile/run` for that profile, still without granting the fixer verdict authority.

Quality fallback: if no `governs` edge needs work, `loom next --mode quality` proposes the first never-measured `(QualityRule × leaf implemented Intent)` pair. Roll-up parents and scenario children are excluded because they are not independent code-bearing quality surfaces. Recording the verdict creates the `governs` edge, so seeding a pack creates actionable work.

`loom next --json` serializes as `NextOutput`. Abbreviated shape (see `llm-driver.md` for the full WorkItem, TruthGap, and GraphState fields):

```json
{
  "work_item": {
    "mode": "quality",
    "owner_role": "quality",
    "effort": "mid",
    "reason": "...",
    "target": { "kind": "rule_intent_pair", "id": "...", "name": "...", "from": "...", "to": "..." },
    "stale_causes": ["..."],
    "prompt_contract": { "role": "quality", "allowed_actions": [], "write_back": "..." },
    "context": {
      "purpose": "...",
      "linked_entities": [{ "role": "target", "kind": "intent", "id": "...", "name": "...", "description": "..." }],
      "suggested_reads": [{ "reason": "...", "command": "loom rule show ..." }],
      "read_set": [{ "path": "src/lib.rs", "locator": "symbol", "why": "..." }]
    },
    "truth_gap": { "axis": "verdict", "missing_form": "...", "correct_when": "..." },
    "next_step": "after recording the verdict, run `loom status`"
  },
  "graph_state": { "planned": 0, "stale": 0, "uninspected": 0, "low_confidence": 0, "adversarial_review": 0, "inconclusive_challenges": 0, "review_independence_warnings": 0, "open_questions": 0 }
}
```

```text
loom find [--limit N] [--exact] [--tag <term>] [--where KEY=VALUE] ["<query>"] [--json]
loom explain <intent> [--json]
loom context <file|intent|query> [--json]
```

`find` searches intents/codefiles/quality rules by keyword (fuzzy) or `--exact` whole-name match. `--tag` and repeatable `--where KEY=VALUE` filter by vocabulary tag and allowlisted facets (`visibility`, `level`, `aspect`, `origin`, `ratification` — also listed in `loom schema`). Filters AND together; query may be omitted when filters alone select the set. `ratification=unratified` also finds intents with no ratification facet, because absence fails closed as unratified.

`explain` is a read-only neighborhood brief for one intent: description, facets/tags, groundings, 1-hop related intents, validations, completeness scorecard, open questions. It is **not** a `loom next` work lane.

`context` is the compact, read-only packet for an operator about to work on a file or behavior. It resolves an intent first, then an exact registered codefile path, then the closest intents using the same keyword scoring as `loom door`. It reports criteria/lifecycle/ratification, groundings and locators, validation and quality-rule state, notes and open questions, completeness (for an intent), and plainly labelled stale or failing edges.

Keyword-substring search over intents, codefiles, and quality rules. It is not BM25. Fuzzy hits that match the query as a whole name (case-insensitive) are tagged `(exact)` so an existence check never rests on reading a score. `--exact` restricts output to those whole-name matches only — the reliable "does a node named exactly this exist?" check, and it lists every id when duplicates share a name.

```text
loom door "<utterance>" [--json]
```

Capture-first entry for free-form human/LLM language. Creates an `InboxItem` and returns a landing menu: closest intents by keyword score, compass pulse, prefilled landing commands (`existing_intent`, `new_intent`, `hypothesis`, `spike`, `dismiss`), and the closing `loom inbox mark <id> routed` step. The `new_intent` landing includes `--visibility user_visible`, `--aspect happy`, and an `after` hint to run `loom next --mode elaborate` so the first idea grows its forgotten surroundings.

```text
loom guide [--role builder|analyzer|fixer|validator|quality|monitor] [--json]
```

Self-contained driving protocol. `--json` includes `operator_loops`, `truth_axes` (each with `correct_when`, the falsifiable criterion for that form of truth), and `intake` — the capture-routing rule: human/external input → `loom door`; evidence-backed code/tool observations → `loom finding add`; product decisions → `loom question add`; structured plan/RFC → `loom proposal add`; falsifiable design claim → `loom hypothesis add`; timeboxed activity → `loom task add`. `--role` adds the lane's mindset, allowed/forbidden writes, evidence requirements, and the same truth-axis honesty line.

```text
loom schema [--json]
```

Node types, edge kinds, property registry, tag vocabulary, state machine, lifecycle model, and valid value enums.

```text
loom checkpoint recommend --intent <intent> [--intent <intent> ...] [--json]
```

Read-only semantic checkpoint inspection. Every selected Intent must be
implemented and ratified; a bundle must share an accepted Journey or form one
connected `requires`/`hierarchy` subgraph. Loom checks the exact Git diff,
current relevant validations, read-only sync freshness, `doctor`, and
`loom.graph.json` freshness. Ready output contains scope and rationale, every
included and excluded path with a reason, every check and exact validation
command, deterministic message, and driver policy. Blocked output contains no
recommendation and names every blocker. Readiness is semantic and never uses a
fixed file/change count.

The command opens the graph read-only and never stages, commits, or pushes. An
acting LLM may autonomously create or defer the exact local commit, staging
only the included paths and never `git add -A`; ambiguity or user-owned overlap
means defer. Push is a separate external action requiring a current explicit
human answer bound to the exact repository, remote, branch, and commit. Silence,
refusal, or drift leaves the commit local and requires no question merely to
create the local checkpoint.

---

## Graph init and travel

## Pattern library (manual lookup and automatic coding guidance)

```text
loom pattern add --name N --rationale TEXT --when-to-use TEXT --when-not-to-use TEXT [--path GLOB]... [--intent-tag TAG]...
loom pattern update <key> [normative fields/selectors] [--name N] --reason TEXT
loom pattern show|list
loom pattern lookup [--path PATH]... [--intent-tag TAG]... [--offset N]
loom pattern ratify <key> --evidence TEXT [--human-decision "<exact human answer>"]
loom pattern retire|remove <key> --reason TEXT
loom pattern exemplar add <pattern> <codefile> --locator SYMBOL
loom pattern exemplar verdict <edge> ground|issue|independent --criterion TEXT --evidence TEXT
loom pattern exemplar remove <edge> --reason TEXT
```

Patterns are strict human-ratified guidance. Lookup serves only live `routable`
patterns and uses OR within each selector family and AND between path and exact
tag families. The same matcher automatically enriches build/fix packets after
their read set is complete (maximum 5 exemplars and 12 KiB excerpt text, with
matched/included/omitted counts and an exact lookup command). Excerpts are live,
explicitly clipped when necessary, and never stored/exported.
No selectors means manual-only.

```text
loom init [<path>] [--name <graph-name>] [--observed] [--json]
```

Creates `.loom/` and initializes `graph.sqlite`. `--observed` maps code the driver does not own (discovery-only; build/fix lanes disabled).

```text
loom mode [owned|observed] [--json]
```

Show or set the graph **mode**. `observed` maps code the driver does not own — discovery/quality/validation only, with the build/fix/coverage/elaborate lanes disabled; `owned` is the normal build-and-prove mode. Omit the argument to print the current mode. This is the post-init counterpart to `init --observed`: a graph created one way can be switched later. `observed` is a mode, **not** a "has been scanned" flag — `loom sync` never changes it (scanning files says nothing about who owns them).

```text
loom sync [--json]
```

Runs a discovery pass then recomputes the structural plane from disk. Discovery expands remembered codefile globs, respects `loom ignore`, and registers new files; structural recomputation uses content hashes, so mtime churn never false-flags. Sync re-verifies evidence anchors, records precise `stale_cause` facets, resets affected ordinary validations, invalidates compiler-owned Journey proof state when its semantic, derivation, surface, compiler, or covered-code hashes drift, and reopens realizing `implements` groundings **symbol-scoped**. A resolving locator whose symbol body did not change is spared; missing, ambiguous, or unresolvable locators fail closed to file scope. Directional relationships stay settled only when their stamped citations cover the changed endpoint and those bytes remain intact. If `loom.graph.json` already exists, sync refreshes it only when the export has drifted; it never creates an untracked export. JSON output reports newly discovered files plus structural counts such as `edges_staled` and `edges_spared`.

```text
loom export [--check] [--json]
```

Writes deterministic `loom.graph.json`. `--check` exits non-zero if committed export drifts from the live graph. The export includes a portable `config` map for `layer_order`, `ignores`, `codefile_globs`, and `scan_adapters`, so import no longer silently loses layer/ignore/glob/adapter setup.

```text
loom import <file> [--repair-orphans] [--json]
```

Restores an export into a fresh store. Import is validate-then-write and never leaves a partial graph. A facet/tag whose target node/edge is absent from the export is rejected — with one exception: an asserted `adjudication` verdict on a derived Finding id is a valid soft reference (the finding re-materializes on the next `sync`), so it is kept, and an export carrying only such references round-trips cleanly. `--repair-orphans` is the recovery path for a legacy or cross-version export with genuinely dangling references: it drops the orphan facets/tags (never the soft-ref verdicts) and reports each one dropped.

### Cross-graph federation

```text
loom graph link <path-to-loom.graph.json> [--name <alias>] [--json]
loom graph unlink <alias-or-graph-id> [--prune] [--cascade] [--json]
loom graph prune-orphans [--alias <alias>] [--cascade] [--json]
loom graph list [--json]
```

Link an upstream graph via its committed `loom.graph.json` export. `link` reads the export, registers the upstream in portable config (`upstream_graphs` meta key), and creates `UpstreamIntent` shadow nodes for each intent in the upstream graph. The `--name` alias defaults to the upstream graph's name; aliases must be unique. Shadow nodes are named `upstream/<alias>/<intent-name>`.

`UpstreamIntent` is a distinct node type — invisible to all local intent queries (status counts, maturity ladder, completeness scorecards, work queues, coverage gates). It follows the CodeFile truth-class pattern: the node itself is asserted (created by `graph link`, body carries provenance `{graph_id, node_id, alias}`), while live upstream state (`upstream_description`, `upstream_status`, `upstream_content_hash`) lives as derived facets rebuilt every sync from the upstream export. `wipe_derived + sync` converges (INV-2).

`loom sync` runs a federation pass after the codefile discovery pass: for each linked upstream, it reads the export file, compares a content hash against a cached value, and on change parses the export, diffs against shadow nodes, creates new shadows for new upstream intents, updates derived facets on changed ones, marks deleted upstream intents with `upstream_missing=true`, and stales all `DependsOn` edges whose upstream target changed. An unchanged upstream adds only one `stat()` + hash comparison — negligible overhead.

**Unlink vs permanent dispose.** `unlink` removes the upstream registration and, by default, **keeps** shadow nodes orphaned so a mistaken unlink can re-link and reattach (same shadow names). `loom doctor` hard-fails on each orphan (`orphaned_upstream_intent`) until they are disposed — this alone blocks the maturity `hardened` rung. That is intentional: orphans are recoverable state, not silent deletion.

Cleanup path when the upstream is **permanently gone** (vendored/inlined, registry empty):

1. Prefer at unlink time: `loom graph unlink <alias> --prune` — drops that alias's shadows in the same step.
2. After a plain unlink already happened: `loom graph prune-orphans` (optional `--alias <alias>`).
3. If local intents still assert `DependsOn` → those shadows, prune **refuses** those nodes and lists the blocked edges/intents. Either `loom edge remove <edge-id> --reason '…'` for each claim you no longer hold, or re-run with `--cascade` (on `unlink --prune --cascade` or `prune-orphans --cascade`) to hard-delete the shadows and cascade-delete the DependsOn edges.

`intent remove` does **not** apply to `UpstreamIntent` — use the prune path above, never store surgery or export-filter-reimport.

```text
loom edge depends-on <intent> <upstream-shadow> [--json]
```

Declare that a local intent depends on an upstream (federated) intent. Creates an asserted `DependsOn` edge (Intent → UpstreamIntent). When the upstream intent changes and sync stales the edge, the local intent's dependents are re-flagged for verification.

```text
loom apply <file> [--json]
```

Applies one atomic batch of mutations from a JSON (default) or YAML (`.yaml`/`.yml`) file, collapsing the per-mutation call storm of a work session (intent add ×N, edge implement ×N, edge verdict ×N, edge relate) into a single call. Every mutation goes through the same write boundary the individual commands use — the intent gates (symbol-name rejection, level/lifecycle/visibility/aspect), the edge-kind registry and lane gate, and the evidence gates (INV-4/6) plus the asserted/derived wall (INV-5) — so a batch can never accept what the per-verb command would reject. The whole batch is one transaction: any rejected item rolls every prior mutation in the batch back (the two-phase-import discipline), and output is emitted only after commit. Like `sync`, a tracked+drifted `loom.graph.json` is refreshed as a byproduct.

Sections (all optional, applied in dependency order — `vocab` first, then `intents`, then `groundings`/`relationships`/`verdicts`/`adjudications`, and `tags` last — so a later section may reference an intent or term the same batch created):

```jsonc
{
  "intents":       [ { "name": "...", "description": "...", "level": "feature", "lifecycle": "planned",
                       "visibility": null, "layer": null, "aspect": null, "allow_symbol_name": false } ],
  "groundings":    [ { "intent": "<name/key>", "codefile": "<path/key>", "locator": "sym", "role": "realizes",
                       "verdict": { "verdict": "ground", "criterion": "...", "evidence": "...", "confidence": 0.9 } } ],
  "relationships": [ { "kind": "requires", "from": "<intent>", "to": "<intent>",
                       "verdict": { "verdict": "ground", "criterion": "...", "evidence": "..." } } ],
  "verdicts":      [ { "edge": "<edge id or prefix>", "verdict": "ground", "criterion": "...", "evidence": "..." } ],
  "adjudications": [ { "finding": "<finding id or prefix>", "verdict": "justified", "reason": "..." } ],
  "vocab":         [ { "term": "payments", "why": "..." } ],
  "tags":          [ { "intent": "<name/key>", "terms": ["payments"] } ]
}
```

`verdict` verbs match `loom edge verdict`: `ground` | `issue` | `independent`. Groundings and relationships are find-or-create (idempotent — an existing edge is reused, never duplicated); intent creation is create-only (re-declaring an existing name is rejected, and the atomic rollback leaves the graph unchanged). A re-recorded identical verdict is a boundary-level no-op, so re-applying an unchanged batch does not churn exported timestamps.

**Mechanical reconfirm:** when `loom next --mode analyze --all` shows `routing_hint: mechanical` / `cause_class: cheap`, an orchestrator may batch-reaffirm those edges through `verdicts[]` (reuse the prior criterion; cite intact evidence) instead of opening each full packet. Judgment items stay one-at-a-time via `loom next`.

`adjudications` records a durable finding verdict (`needed` | `justified` | `rejected` | `deferred` | `blocked` | `duplicate` | `resolved` with a substantive reason) — the same gate as `loom finding verdict`, on a finding materialized by a prior `sync`. Use `resolved` only when the finding was true and the repair has now been observed; `rejected` means the finding itself was false or below threshold. `vocab` registers terms (idempotent) and `tags` tags an intent with registered terms (same gate as `loom intent tag add`); list a term under `vocab` earlier in the same batch to register and apply it in one call — collapsing the per-intent "arm the duplicate detector" churn, just as `adjudications` collapses per-finding triage.

### Concurrency

Read commands (`loom status`, `loom next`) open the graph under a **shared** advisory lock with SQLite `query_only`, so several agents can query one graph at the same time and never block each other. Writers take the lock exclusive. Every lock acquisition is bounded (see `loom limits`): competing writers remain fail-fast at `lock_wait_ms=2000`, while read-only observers wait up to `read_lock_wait_ms=10000` for an in-flight writer to finish. A contender that outlasts its budget exits 75 (`EX_TEMPFAIL`) with a `loom-lock-contention` error that names the recorded holder — pid, access mode, start time, and command line — so a hang is never the failure mode and the diagnosis travels with the refusal. `loom scan` runs its external adapter commands with **no** lock held — reading adapter config under a shared lock, executing the subprocesses lock-free, then reopening for a brief exclusive write to reconcile findings — so one long scan no longer freezes every other agent for the duration of its subprocesses.

### Role leases (multi-driver coordination)

```text
loom role claim <builder|analyzer|fixer|validator|quality|rectify> [--take-stale]
loom role release <role>
loom role list [--json]
```

Several LLM drivers coordinate on one graph by each claiming a distinct role. A lease is **advisory operational state** under `.loom/leases/` — it grants no write authority (the lane gate on `LOOM_AGENT` does that, INV-7) and names its holder by the self-declared, unverified `LOOM_AGENT_PROFILE`. It exists so honest drivers partition themselves: one profile per role keeps queues disjoint and keeps each actor's write rate inside its own audit bucket.

The lease is a **heartbeat**, not a held lock: drivers are many short-lived processes, so nothing lives long enough to hold a flock for a session. `claim` requires the matching lane authority (`LOOM_AGENT=llm:<role>`) plus a profile, and every later loom command run under that identity stamps the lease's `last_seen_ms` at store open. A lease not refreshed within `role_lease_ttl_ms` (see `loom limits`) reads as **stale** — a crashed driver frees its role by silence, with no cleanup step. Claiming a **fresh** foreign lease refuses with exit 75 and a `loom-role-contention` error naming the holder; taking over a **stale** one requires the deliberate `--take-stale`. Release is holder-only. Claims, releases, and takeovers land in the journal (`role_claimed` / `role_released`); the heartbeat itself is journal-silent.

`loom role list` (and the `roles` block in `loom status --json` / `loom session --json`) announces every claimable role with its holder, freshness, actual per-role queue depths, and their sum as `debt` — a joining driver picks the free role with the most debt behind it. Review debt is attributed to the challenged edge kind's registry owner instead of being copied onto every review-capable role. The Review packet prefers a different `LOOM_AGENT_PROFILE` from the profile that recorded the target Verdict; using the same or an unavailable profile remains possible but is surfaced by status and audit as a non-blocking warning. Solo operators need none of this: solo drives every lane and `claim` refuses it. `loom next` cooperates: when the served packet's owning role is freshly leased to a different profile, the output carries a `lease_conflict` warning naming the holder — the packet is still served (the lease stays advisory), so a collision is chosen, never accidental.

**Orchestrated sub-drivers (within one lane).** A master driver that holds a role's lease may fan the lane's targets out to coordinated sub-drivers: each exports the same lane authority (`LOOM_AGENT=llm:<role>`) with its **own** `LOOM_AGENT_PROFILE`, and works an explicit disjoint slice handed to it by the master (sub-drivers never race `loom next`). The judgment-burst audit budgets asserted writes per (actor, profile, minute) and every fact records `asserted_profile`, so each declared profile is one independently budgeted, fully attributed judging mind — parallel speed stays defensible because the attribution stays visible. Proof execution remains serial regardless: the harness lock admits one executor and refuses the second with exit 75.

```text
loom detect [--json]
```

Detects repo languages and recommends seedable quality packs only. Available packs are: `iso5055`, `service`, `web-ui`, `data`, `concurrency`, `docker` (29 rules total across the shipped pack set).

```text
loom bootstrap suggest [--json]
```

Cold-start assist when the graph has registered codefiles and no authored Journey roots. It scans derived signals (top-level `src/` modules from registered codefiles, `tests/*.rs`, README `##` headings) and writes a **Proposal** of non-authoritative Journey clues. Review those clues against product evidence, then author `loom.journey/v1` artifacts and register them with `loom journey add <spec>`. Do not adopt inferred repository structure directly as product meaning.

Hard rules: never creates Journey roots, Intents, `implements`/`governs`/`validates` verdicts, or `implemented` lifecycle; refuses when authored Journeys already exist. `loom session` offers this command when code is registered but no Journey root exists.

```text
loom scan add <name> "<command>" [--map <map>] [--format lines|json] [--json]
loom scan list [--json]
loom scan update <name> [--command "<cmd>"] [--map <map>] [--format lines|json] [--json]
loom scan remove <name> [--json]
loom scan run [<name>] [--json]
```

External diagnostic adapters can wrap any language's linter, type-checker, static analyzer, or bespoke script. `scan add` stores the adapter command in graph config; `scan list` shows registered adapters; `scan remove` deletes one; `scan run [<name>]` runs one adapter or all adapters and converts parsed diagnostics into derived `Finding` nodes for ordinary `triage`.

Under the default `--format lines`, the parse map is GCC-style `file:line[:col]: message`. The default parser also pairs a bare `file:line[:col]` location line with the message on the immediately following line (svelte-check-style two-line output; a blank line in between drops the pair). `--map` accepts a custom regex with named groups `file` and `line`, plus optional `msg` and `code`; a custom map is strictly per-line.

`--format json` parses the output as a JSON array of finding objects (JSONL also works, and noise before/after the document is tolerated). The default field lookups are `file`, `line`, `message`, `code`; `--map` renames them as comma-separated `field=path` entries with dotted paths for nested objects, plus `items=<path>` when the array lives inside an envelope object. Examples: pulse (`loom scan add pulse "pulse check -a --json" --format json --map "line=start_line,msg=detail"`), qualirs (`--map "items=smells,file=location.file,line=location.line_start,msg=message"`). A number or numeric string works as `line`; a missing/null line records a whole-file diagnostic (line 0).

In both formats, only diagnostics whose `file` resolves to a registered `CodeFile` become findings. Re-running an adapter converges: findings for diagnostics still present stay active, new diagnostics create findings, and findings whose diagnostics disappeared are resolved. Scan adapters travel with `loom export` in `config.scan_adapters`.

```text
loom calibrate [--write] [--json]
```

Derives structural finding thresholds (`oversized_file`, `complex_symbol`, `large_symbol`, `deep_nesting`, `excess_args`) from the repo's own distribution: each gate is proposed at the worst-5% quantile of the registered codefiles' metrics, rounded up and clamped to sane floors, so sync flags today's tail without flooding triage. Default is a preview (current vs proposed); `--write` persists the proposal to graph config. Thresholds travel with `loom export` in `config.thresholds`; absent config means the shipped defaults (file loc 600, symbol complexity 20, symbol loc 120, nesting 5, args 6). Every gate is a strict `>` bound. Ownership smells are not count-gated: `tangled_file` fires when ≥2 realizing owners of a file do not form one connected neighborhood via relationship edges (relates / hierarchy / scenario-of / …). A parent-plus-scenarios star stays silent; disconnected co-owners fire. Legacy `max_file_owners` in an old export is ignored on load.

```text
loom threshold list [--json]
loom threshold set <gate> <value>
loom threshold reset [<gate>]
```

The manual counterpart to `calibrate`: hand-set a single gate instead of fitting the whole set from the distribution. `<gate>` is one of the `config.thresholds` keys (`max_file_loc`, `max_symbol_complexity`, `max_symbol_loc`, `max_nesting`, `max_args`); `<value>` must be ≥ 1. `set` persists to `config.thresholds` (portable — travels in the export). `reset <gate>` restores one gate to its shipped default; `reset` with no gate drops the whole `thresholds` config so every gate reverts to "absent = shipped default" (not a pinned snapshot — a later change to the defaults still takes effect). `max_file_owners` is retired (`tangled_file` uses graph connectedness); setting it errors.

```text
loom policy show [--json]
loom policy set-floor <fraction> [--json]
loom policy set-adversarial-frontier <count> [--json]
loom policy gate-add <lane> [--json]
loom policy gate-remove <lane> [--json]
loom policy reset [--json]
```

Read or set the evidence policy. `set-floor` sets the review-confidence floor (a fraction in `[0.0, 1.0]`) below which a recorded verdict is routed to `loom next --mode review`. `set-adversarial-frontier` sets the fixed risk frontier size (`0` disables it, shipped default `5`, maximum `100`). `gate-add`/`gate-remove` move an owner lane (`builder | analyzer | fixer | validator | quality`) in or out of the human-gated set described in `llm-driver.md`. The policy persists to portable `config.evidence_policy` and travels with the export; absent config means the shipped defaults, and `reset` drops the config to restore them.

```text
loom completeness [<intent>] [--json]
```

Definition-of-Complete scorecard: per-intent axes met/open/waived. Omit the key for all feature intents. The axes are `scenarios`, `prerequisites`, `boundary`, `proof`, `journey`, and `questions`. `scenarios` is satisfied by a family of `scenario-of` intents with `--aspect happy|sad|fallback|edge_case`; `questions` is driven by first-class `Question` nodes (`loom question add "..." --intent <intent>`) and closes when those questions are answered or closed as withdrawn/duplicate/deferred, not by a waiver.

---

## Intent commands

```text
loom intent add --name "<name>"
  [--description "<desc>"]
  [--level system|component|feature|cross_cutting]
  [--lifecycle planned|implemented|needs_change]
  [--visibility user_visible|internal]
  [--layer <layer>]
  [--aspect happy|sad|fallback|edge_case]
  [--allow-symbol-name]
  [--json]
```

**Atomization guard:** if the intent name matches a symbol pattern (for example snake_case with no spaces), the command is rejected unless `--allow-symbol-name` and a behavioral `--description` are both provided. Functions and symbols are locators on `implements` edges, not intents.

**Provenance + ratification:** every minted intent is stamped with `origin` (`human` for a solo agent, `llm` for a declared `llm:*` lane) and a `ratification` state. A solo mint is born `ratified` — the minting act is the ratification. A lane mint is born `unratified`: first-class in the graph (groundable, provable, queryable) but the `wanted` maturity rung stays unmet until a human ratifies it. Absent ratification always reads as `unratified` (wantedness is never presumed).

```text
loom intent update <intent> --reason "<why>"
  [--description "<new>"] [--reword]
  [--name <new-name>]
  [--level system|component|feature|cross_cutting]
  [--visibility user_visible|internal]
  [--aspect happy|sad|fallback|edge_case]
  [--lifecycle planned|implemented|needs_change]
  [--rectify escalated|clear]
  [--json]
```

`update` is the single mutation verb. The ripple rule lives in the fields, not in command choice: a `--description` change is a redefinition and ripples one hop (passing/independent edges become `needs_reverification`, linked validations reset, completeness waivers are cleared so waived axes re-open, and old wording is preserved in decision notes); `--reword` is same meaning, clearer words, no ripple. `--name`, `--level`, `--visibility`, `--aspect`, and `--lifecycle` never ripple. `--rectify escalated` moves a discovered behavior to human ratify. On a live duplicate-intent item, `--rectify clear` records that pair as distinct against the content hash of both descriptions; unrelated writes do not resurrect it, while changing either description reopens the comparison. With no duplicate pair, `clear` removes the discovery escalation. Every update records `--reason`.

```text
loom intent ratify <intent> --evidence "<why wanted>" [--human-decision "<exact human answer>"] [--json]
loom intent ratify --all --evidence "<why wanted>" [--human-decision "<exact human answer>"] [--json]
loom intent reject <intent> --reason "<why unwanted>" [--human-decision "<exact human answer>"] [--json]
loom intent confirm <intent> [--json]
loom intent retire <intent> --reason "<why>" [--replaced-by <intent>] [--json]
loom intent remove <intent> --reason "<why>" [--json]   (mistakes only; refuses intents that still have hierarchy children)
loom intent reactivate <intent> --reason "<why>" [--json]
loom intent waive <intent> scenarios|prerequisites|boundary|proof|journey --reason "<why>" [--json]
loom intent show <intent> [--json]
loom intent list [--limit N] [--offset N] [--json]
loom intent tag add <intent> <term> [--json]
loom intent tag remove <intent> <term> [--json]
loom intent journey-exempt <intent> --kind <stable-class> --reason "<why>"
  [--human-decision "<exact human answer>"] [--json]
loom intent journey-require <intent> --reason "<why ancestry is required again>"
  [--human-decision "<exact human answer>"] [--json]
```

`ratify` and `reject` are human-authorized decisions (INV-8), but the human no longer has to execute the CLI write. In a host conversation, the LLM summarizes the packet, recommends Keep / Remove / Revise with reasons, asks the human, and waits. After the reply it may execute the selected command with `--human-decision` containing the human's exact answer. Loom records `ratified_by=human` while the journal separately retains the executing `llm:*` actor and the mediated response. Without `--human-decision`, every `llm:*` direct write is rejected; a solo terminal retains the exact typed challenge. This is mediation, not policy delegation: silence, a placeholder, or an LLM-generated answer grants no authority. Redefining a ratified intent (`update --description` without `--reword`) stales its ratification to `needs_reconfirmation` and the ratify queue re-serves it. `confirm` re-affirms meaning (a note, not a ratification). `retire` sets status to deprecated and removes the intent from active computation while preserving history. `waive` records a reasoned waiver for a non-question completeness axis (`scenarios`, `prerequisites`, `boundary`, `proof`, `journey`); if the intent is later redefined through `intent update --description`, waiver facets are cleared and those axes are scored again. Open questions must be answered with `loom question answer` or closed with `loom question close`.

`journey-exempt` is a separate human product decision: it records why an Intent deliberately has no authored Journey ancestry. It is not a shortcut for incomplete derivation. `journey-require` withdraws that exemption when the behavior becomes user-reachable. Both operations require the exact human answer when executed by an `llm:*` actor. Rewording preserves the exemption; changing the behavior's criterion invalidates it.

---

## Edge commands

```text
loom edge implement <intent> <codefile> [--role realizes|consumes|configures|verifies] [--locator "<symbol>"] [--json]
loom edge exercises <validation> <codefile> [--locator "<entry-symbol>"] [--json]
loom edge call <validation> <surface> [--json]
loom edge remove <edge-id> [--reason "<why>"] [--json]
loom edge set-locator <edge-id> <locator> [--json]
loom edge set-role <edge-id> realizes|consumes|configures|verifies --reason "<why>" [--json]
loom edge rehome <edge-id> --to "<successor intent>" --reason "<why>" [--json]
loom edge retarget <edge-id> --to "<successor node>" --reason "<why>" [--json]
loom edge show <edge-id> [--json]
loom edge list [--intent <intent>] [--codefile <codefile>] [--limit N] [--offset N] [--json]
loom edge depends-on <intent> <upstream-shadow> [--json]
```

`edge implement` defaults to `--role realizes`; only realizing groundings own coverage. An intent may realize in several files when each file holds a slice of the same criterion (sibling slices) — do not mint a second intent for that behavior. Use `consumes` when a file calls behavior across a seam, `configures` when it supplies configuration, and `verifies` when it checks behavior elsewhere; those roles never close coverage. If the file's living criterion is not named by any intent, that is a distinct behavior: discover it in coverage, mint outside the coverage lane, then realize. An exact replay is idempotent, and an uninspected same-role grounding may be re-grounded to a corrected locator. If the `(intent, codefile)` pair already exists with a different role—or an inspected edge is given a different locator—creation refuses and names the edge; use `edge set-role`, `edge set-locator`, or remove it explicitly instead of silently rewriting a settled claim. `apply` enforces the same collision boundary atomically. `edge set-role` records a decision note and reopens a settled edge with `stale_cause=role_changed...` when the role changes. `edge rehome` supersedes the old grounding with a `superseded_by` facet, creates or reuses the successor grounding with the old locator and role, and reopens it with `stale_cause=rehomed...`. `edge show` prints edge facets; JSON includes a `facets` object. `edge remove` refuses derived edges. `edge retarget` re-points an asserted edge's target at a successor node IN PLACE — the recorded operation of correcting an edge whose endpoint was wrong — and refuses a target that would duplicate an existing edge. It preserves the edge id, verdict history, notes, facets, timestamps, role, and locator; stales the edge with `retargeted: <old> -> <new>; <reason>`; and resets validation status when retargeting a `validates` edge. `retarget` cannot change endpoint node types; use the right edge family instead.

`edge list` can filter to edges incident to one Intent and/or CodeFile. Human and JSON rows include endpoint names plus grounding `role` and `locator`, so deciding ownership never requires a lossy whole-graph dump followed by separate edge reads.

`edge exercises` records validation-specific proof entry evidence (`Validation -> CodeFile`). It owns no implementation coverage and is not a substitute for `implements`: it says this proof enters through this file/symbol. Use it when command derivation cannot map a custom runner. Only a **locator-bound** entry (`--locator <entry-symbol>`) is S3-eligible; a bare file claim is diagnostic-only and cannot earn a call witness. Editing that CodeFile resets the validation on sync. Intent-level `implements --role verifies` remains useful as an intent-wide verification surface, but it is only a visible legacy fallback for strength and cannot earn S3 for a validation by itself. Compiler-owned Journey validations refuse this command — declare downstream entries on the surface operation's `exercises` array and recompile.

`edge implement` and `edge set-locator` lint proof-strength dead ends at write time (warnings, never refusals — JSON carries them as `lints`): a realizing locator that names a symbol the call graph cannot treat as callable (a struct, type, const, binding — anything but a function/method) caps every proof below S3, and a `--role verifies` file exposing zero indexable symbols (an unsupported language, or declarations but no callable) caps it the same way. Each lint says what WOULD be indexable: the file's callable symbols, or the indexable languages.

```text
loom edge relate <kind> <from-intent> <to-intent> [--json]
```

`<kind>` is one of: `hierarchy`, `requires`, `scenario-of`, `variant-of`, `triggers`, `sequence`, `relates`.

```text
loom edge verdict <edge-id> <ground|issue|independent>
  [--criterion "<falsifiable claim>"]
  [--evidence "<what was found>"]
  [--confidence <0-1>]
  [--json]

loom edge explore <intent-a> <intent-b> <ground|issue|independent>
  [--criterion "<falsifiable claim>"]
  [--evidence "<what was found>"]
  [--confidence <0-1>]
  [--json]
```

Verdict commands inspect relationship/grounding claims. `independent` means measured and not related/applicable; it requires real evidence. **Evidence anchoring:** every verdict-recording command (`edge verdict`, `edge explore`, `rule verdict`, `validation verdict`, `apply` batches) parses citations out of `--evidence`; each citation that resolves to a real file under the graph root is stamped with a fingerprint of the cited lines (asserted `evidence_spans` edge facet) so sync can later grade a re-open as "cited span intact" vs "rewritten". Three citation forms: `file:line[-line]` (explicit span — an end beyond EOF rejects), `file:line-` (open range, clamped to EOF), and `file:@symbol` (the span is resolved server-side from the symbol's declaration; an unknown or ambiguous symbol rejects). Citing an existing file at lines that do not exist rejects the verdict — evidence must describe bytes someone can read. More than 16 distinct resolvable citations also rejects the verdict rather than silently dropping dependency evidence. Citations that resolve to nothing (URLs, tool output, deleted paths) are ignored, never guessed at. A stamped span anchors to its content and enclosing symbol, not its line position: a body that moves intact is re-anchored and journaled (`evidence_reanchor`) on sync, and the verdict stands.

### Adversarial challenge commands

```text
loom challenge record <edge> <survived|counterexample|inconclusive>
  --hypothesis "<falsifiable attack>"
  --evidence "<attempt and observation with file:line or journal:id>"
  [--impact "<required for counterexample>"]
  [--confidence <0.0-1.0>]
  [--json]
loom challenge show <edge> [--json]
loom challenge list [--state survived|counterexample|inconclusive] [--limit N] [--offset N] [--json]
```

`challenge record` is the write-back for an adversarial Review packet. It targets the current Verdict fact through an automatically minted semantic snapshot, allows only one attempt per edge and Verdict revision, and is replay-idempotent. `survived` and `inconclusive` close that exact revision without changing its Verdict. `counterexample` requires an impact statement and creates the corresponding asserted Finding in the same database transaction; Triage alone decides whether the claim needs repair. A changed Verdict or Verdict evidence invalidates the snapshot on `loom sync` and reopens the candidate. Reviewer-profile equality or missing profile attribution is recorded as a non-blocking audit warning, not a reason to drop the observation.

---

## CodeFile and coverage exclusion commands

```text
loom codefile add <path-or-glob> [--observed] [--json]
loom codefile anchor <path> --at-line <one-based-line> [--json]
loom codefile rescan [--json]
loom codefile remove <path-or-key> [--successor <path-or-key>] [--json]
loom codefile show <path-or-key> [--json]
loom codefile list [--limit N] [--offset N] [--json]
```

`codefile anchor` is read-only: it issues and prints the exact stable marker and
locator for the smallest supported declaration containing `--at-line`, without
editing source or graph state. It fails closed when the path is unregistered,
the line is outside a supported declaration, or an equivalent marker would be
ambiguous. After the caller inserts the issued marker, anchor syntax,
attachment, and cardinality are owned by the locator module; sync and checkpoint
freshness consume that same resolver instead of re-parsing marker text.

`codefile remove` is refactor-safe: with live asserted edges pointing at the file it REFUSES and lists every blocker with its `loom edge retarget <id> --to …` remedy (no silent orphaning, no ghost registration). With `--successor <file>` (register the successor first), a rename/split is one recorded operation: each live edge is retargeted in place — verdict history kept — then the node is removed and an `edge_retargeted`/`node_removed` journal pair records the move. Live edges originating FROM the file can never be auto-cascaded and block either way.

`--observed` registers files the graph monitors but does not own (vendored or upstream code): `loom sync` scans them and surface/contract staleness still ripples, but they carry no ownership, coverage, or build obligations — the per-file counterpart of the graph-level observed mode. Re-adding an already-registered file with `--observed` marks it observed. A glob added with `--observed` is remembered, so `codefile rescan` and `loom sync` register files that appear under it later as observed too; a file matched by both an owned and an observed glob registers as owned.

Glob-based registration (`codefile add '<glob>'`, `codefile rescan`, and the discovery pass inside `loom sync`) respects `loom ignore` exclusions: a file matching an ignore glob is silently skipped during glob expansion. Explicit literal adds (`codefile add path/to/file.rs`) always go through — explicit intent overrides ignore. Files already registered before an ignore glob is added stay registered (ignore never deletes nodes; it only gates future discovery).

`show` returns ownership, locators, imports/symbols/metrics, governing rules, findings, and stale-edge context.

```text
loom ignore add '<glob>' --reason "<why>" [--json]
loom ignore remove '<glob>' [--json]
loom ignore list [--json]
```

Coverage exclusions live in the graph with a recorded reason. `loom coverage` honors them, and glob-based codefile discovery (rescan / sync) skips files matched by ignore globs. Status and coverage retain the full registered denominator, report excluded counts and percentages separately, and group exclusions by reason. `codefile show` identifies matching ignore rules instead of calling an excluded file a coverage gap.

A coverage work packet is a triage packet, not permission to mint graph truth. It embeds the existing ignore taxonomy and neighboring-file dispositions, requires those precedents to be reviewed first, and permits only reusing an existing owner, applying an established exclusion, unregistering a mistaken file, or recording distinct absent behavior as a finding for triage. New Intents are never created directly from the coverage lane.

---

## Validation and proof commands

```text
loom validation add --name "<name>" --intent <intent>
  [--type test|assertion|benchmark|manual_check|journey|scenario|contract]
  [--command "<cmd>"]
  [--json]
```

The `journey` validation type is reserved for compiler output. Although it remains a stored enum value, do not use it to create a Journey proof through `validation add`; use `loom journey compile`.

Proof strength is derived (S0–S5), never authored. S3 is validation-specific: Loom first reads an explicit `edge exercises` entry — the `--locator <entry-symbol>` form is S3-eligible, a bare file claim is not — then derives entry points from that validation's registered operation, test target, binary, or script path and walks the call graph to the Intent's realizing symbol. Compiler-owned Journey proofs are the exception: their `Exercises` closure comes only from `journey compile`, including optional operation-scoped downstream entries whose `observed_by` assertion must have passed on the compiled run. That passage is structured evidence minted only by a local compiler-owned `journey run` (compiler version 6) of the canonical accepted-surface proof; deserialized, imported, caller-authored compiled proofs, or generic command runs cannot supply it. Compiler-v5 Journey proofs are not current and must be recompiled and rerun; schema v12 graphs do not need rebuilding. `validation show` records the grounded symbol plus `call_evidence` (source, file, entry symbol, and for Journey operation exercises also operation/exercise/`observed_by`). An Intent-wide `verifies` grounding is displayed as a non-eligible fallback and cannot strengthen a sibling proof. Unknown execution shapes fail closed at S2 until an explicit locator-bound `edge exercises` is attached. Never use `edge exercises` to mutate a compiler-owned Journey validation.

```text
loom validation verdict <validation> passed|failed|blocked
  [--evidence "<observed proof>"]
  [--reason "<blocker>"]
  [--json]

loom validation update <validation> [--type <type>] [--command "<cmd>"] [--json]
loom validation unlink <validation> <intent> [--json]
loom validation remove <validation> [--json]
loom validation show <validation> [--json]
loom validation list [--limit N] [--offset N] [--json]
loom validation run [<intent-or-validation>] [--all] [--json]
```

`loom validation run` executes stored commands without holding the graph lock while the command executes. Settled verdicts are not re-run unless made pending by sync or command changes.

Proof execution is serialized by an advisory harness lock (`.loom/harness.lock`), taken by `loom validation run`, `loom observe`, and compiled Journey execution — because two concurrent runs may share repository resources and mint false failing verdicts. A second executor refuses immediately (exit 75) with the holder's agent, pid, purpose, and operation rather than racing it. `journey diagnose` observes the selected compiled profile without settling its proof; `journey run` records the observed result.

### Finding triage commands

```text
loom finding add "<claim>" --source code_audit|wiki|validation|llm --kind <kind> \
  --evidence "<observed fact>" --impact "<why it matters>" --confidence <0.0-1.0> \
  (--file <registered-codefile> | --link <ref>) [--json]
loom finding list [--kind <kind>] [--state untriaged|stale|needed|justified|rejected|deferred|blocked|duplicate|resolved] [--json]
loom finding verdict <id> needed|justified|rejected|deferred|blocked|duplicate|resolved --reason "<why>" [--json]
```

`Finding` is the one node type for evidence-backed observations. Programmatic producers (`sync` detectors, `scan run` diagnostics, materialized graph-shape smells) create derived findings; LLM/tool observations enter as asserted findings through `loom finding add`. Both share listing, triage, staleness display, and `loom finding verdict`; verdicts adjudicate signals, they do not fix code.

Resolving adjudications (`justified` | `rejected` | `deferred` | `duplicate` | `resolved`) stay settled across content-hash churn unless the finding's metric worsens past a band (~10% or 50 absolute, whichever is larger). Open work (`needed` | `blocked`) still reopens on any flagged-codefile hash change. Use `resolved` for an observed repair, not for a false positive. Use `loom calibrate --write` so structural gates fit the repo before mass triage.

---

## Journey commands

An authored Journey is the root of delivery, not an executable proof specification. Its strict `loom.journey/v1` artifact contains stable IDs, typed inputs and outputs, declarative preconditions, ordered actor/action steps, expectations attached to steps, and optional profiles. It contains no implementation Intent references, endpoints, or executable operations.

```text
loom journey lint [<journey>] --json
loom journey add <spec.json|spec.yaml> [--json]
loom journey show <journey> [--json]
loom journey list [--limit N] [--offset N] [--json]
loom journey remove <journey> [--json]
loom journey map [--json]
loom journey derive <journey> [--json]
loom journey derive-accept <journey> --manifest <derivation.json>
  --human-decision "<exact human answer>" [--json]
loom journey surface <journey> [--json]
loom journey surface-accept <journey> --manifest <surface.json> [--json]
loom journey compile <journey> [--profile <profile>] [--json]
loom journey run <journey> [--profile <profile>] [--json]
loom journey resume <token> --choice <option-id>
  --human-decision "<exact human answer>" [--free-form "<revision>"] [--json]
loom journey diagnose <journey> [--profile <profile>] [--input <key=json>]... [--json]
loom journey rehearse-cold <journey> --json
loom journey freeze <journey> [--profile <profile>] [--json]
loom journey drift [<journey>] [--json]
```

`lint` is the read-only authored-surface check. With a Journey it scans one
registered current manifest; without one it scans every registered Journey in
name order. `--json` emits `loom.journey-lint/v1`, including deterministic
findings and counts. A blocker report is still printed and then exits non-zero;
advisory-only reports pass. The canonical authoring rules and examples live in
[`journey-authoring.md`](journey-authoring.md).

`resume` consumes the opaque token returned by a pending host-mediated run.
The `--choice` must be an offered stable option ID, `--human-decision` must be
the human's exact substantive answer, and `--free-form` is used only when that
option requires a revision. Continuations are one-shot and bound to the same
canonical graph root, authored semantics, exact compiled proof/profile and
surface, gate step, and (when present) current subject. Root, projection, or
subject drift rejects the resume; a claimed continuation is destroyed after
the attempt and cannot be replayed.

`rehearse-cold` requires `--json`. It is not a cheap target-repository pre-flight: it currently assumes loom's own repository shape (surface manifests at `journeys/surfaces/`, reserved inventory components, ignored build output confined there). A target repository should use `loom journey lint` and `loom journey diagnose` instead. When the layout matches, it runs exactly one registered non-release Journey's `proof` profile in a detached, freshly initialized/imported candidate made from the release source inventory. The Journey must have a current confined artifact and accepted surface and may not contain a human-decision, release, or nested cold-rehearsal operation. It does not settle live proof and reports candidate, source-inventory, cache, runtime, and caller-change attestations.

`add` registers only the authored `Journey` node and its semantic hash. Re-adding the same semantics is idempotent. A semantic change invalidates hash-bound `Derives` and `Surfaces` projections; it never silently carries old technical meaning onto the new Journey. `show` reads one root and `map` shows rooted and unrooted non-exempt Intents.

`derive` is read-only. It emits the current Journey hash, authored steps, existing mappings, unrooted Intents, and the contract for a strict `loom.journey-derivation/v1` manifest. `derive-accept` applies exactly that hash-bound manifest only after a human authorizes one conversational hash-table batch. Silence, name similarity, an LLM-authored approval, a non-null `unresolved_question`, duplicate entries/relationships, or a `requires`/`hierarchy` cycle are rejected.

```json
{
  "schema": "loom.journey-derivation/v1",
  "journey_id": "checkout",
  "journey_hash": "<current semantic hash>",
  "proposal_id": "checkout-derivation-v1",
  "proposal_rationale": "This is the smallest technical projection of the current checkout steps.",
  "intents": [
    {
      "id": "capture-payment",
      "operation": "create",
      "name": "checkout captures authorized payment",
      "criterion": "A valid confirmation records exactly one authorized payment before order acceptance.",
      "level": "feature",
      "visibility": "internal",
      "rationale": "Payment capture is independently falsifiable and covers confirmation.",
      "step_ids": ["confirm-order"]
    },
    {
      "id": "existing-cart",
      "operation": "reuse",
      "intent_id": "<existing intent id>",
      "level": "feature",
      "visibility": "internal",
      "rationale": "The existing cart criterion exactly covers product selection.",
      "step_ids": ["choose-product"]
    }
  ],
  "relationships": [
    {
      "id": "payment-requires-cart",
      "kind": "requires",
      "from": "capture-payment",
      "to": "existing-cart",
      "rationale": "An order confirmation requires a selected cart."
    }
  ],
  "unresolved_question": null
}
```

Every authored step must be covered by at least one Intent entry. `operation:create` requires `name` and `criterion` and must not carry `intent_id`; `operation:reuse` requires `intent_id` and must not restate the existing name or criterion. Every entry needs `rationale`; every relationship needs stable `id`, `kind`, `from`, `to`, and `rationale`, and its endpoints must name included entry IDs. Only `requires` and `hierarchy` are allowed. Loom rejects duplicate IDs or resolved relationships, self-links and cycles, unresolved questions, stale Journey hashes, and reuse that does not resolve to the claimed existing Intent.

Before acceptance, present the human one conversational hash-table batch containing the proposal ID, Journey hash, manifest hash, each create/reuse row, criteria, rationales, covered step IDs, and relationships. On acceptance Loom creates or updates the adopted Proposal and reconciles its `Derives`, `requires`, and `hierarchy` projection. Replaying byte-identical accepted content is idempotent; using an already-adopted `proposal_id` with a different manifest is rejected rather than silently changing the authorized decision.

`surface` is also read-only. Once every current derivation is accepted, implemented, and realizing-grounded, it emits the contract for a stable, production-owned black-box CLI in the target repository. Prefer one unified consumer/administrative CLI over the same application, API, or service boundary as the public behavior. It may be operator-only, but a feature-gated proof binary, test fixture, mock-only path, or privileged shortcut around production behavior is not the Journey surface. The builder writes that source in the repository's language and idiom. A `loom.journey.surface/v1` manifest binds every Journey step to a reusable operation on a `loom.interface-surface/v1`; operations use structured argv, typed arguments, and JSON output. `surface-accept` records the hash-bound surface, its operation bindings, and the exposed registered CodeFile. Loom does not template-generate application source.

Machine-operation timeouts resolve from an optional positive surface-operation
`timeout_seconds` override, otherwise from the selected profile's positive
`timeout_seconds` (default 2700 seconds). Human-decision steps deliberately
have no execution timeout. A timeout kills the operation's process group and
blocks the run with `<label> exceeded the execution timeout`. Profile timeout
is execution policy and is excluded from the authored Journey semantic hash;
the resolved operation timeout remains part of the compiled proof.

### Release trust boundary

```text
loom release authorize-derivations --manifest-dir <review-manifests-dir>
  --human-decision "<exact human answer>" --json
loom release rehearse --phase isolated-dogfood --json
loom release rehearse --phase fresh-fixpoint --json
loom release rehearse --phase gated-preparation --json
```

`authorize-derivations` reads the reviewed `loom.journey-derivation/v1` files
in the supplied directory, requires exactly one current, adopted, canonical
manifest for every registered Journey, and seals the exact sorted batch after
host-mediated human approval. It does not mutate the graph. Its JSON grant
contains a one-shot `rda1_…` token and the exact next command, which hands the
token through `LOOM_RELEASE_DERIVATION_AUTHORITY` to one outer
`journey run release-workflow --profile proof`. Claiming atomically consumes
the token and binds its reviewed manifests to that outer run/proof and to the
phase's candidate permits. Replays and detached/unbound permits fail closed.
The release flow copies those reviewed manifests through the reserved
`review-manifests` path and reauthorizes only the bound projections in each
candidate.

The source-controlled `loom.release-inventory/v3` is the exact cut line. It
declares these ordered argv gates—no generic ecosystem detection or extra gate
inference:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --quiet -- --test-threads=1
cargo build --quiet
```

It also declares exactly `CARGO_HOME`, then `RUSTUP_HOME`, as cache-root
environment names. Each must resolve to an existing absolute non-symlink
directory outside and non-overlapping with the repository and the other root.
Loom hashes their declared artifact trees before and after offline rehearsal
and blocks if they change; it does not infer other package-manager caches.

The rehearsal phases are:

- `isolated-dogfood`: verify once in one detached fresh-v12 candidate.
- `fresh-fixpoint`: verify in two independent empty candidates and require
  equal candidate and semantic result hashes, not equal build-directory bytes.
- `gated-preparation`: run both readiness gates, require equal attestations,
  and record `mutation: skipped_rehearsal`; it never releases, installs,
  commits, or pushes.

Each phase emits `loom.release-rehearsal/v1`; policy/readiness failures use
`status: blocked`, print the report, and exit non-zero. The semantic result hash
includes the candidate tree, canonical reviewed-manifest attestations, outer
Journey/profile, schema version, and sorted deterministic summaries for each
executed Journey (`journey_id`, profile, Journey hash, surface hash, verdict).
Thus independent runs compare behavior-bearing summaries rather than mutable
runtime details or cache/build bytes.

The authored format is semantic:

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

`compile` owns the proof topology. For the selected profile (default `proof`), it resolves the accepted surface operations and creates or refreshes the Validation-specific `Proves`, `Validates`, `Calls`, and `Exercises` closure. Bound operations may declare optional `exercises` entries that name downstream code entries reached through the public operation; those become compiler-owned `Exercises` edges with provenance facets and never additional surface owners. `run` executes only that compiled profile and records what Loom observes, including which typed assertions passed. A current passing proof counts as S3 only when an exercised entry — the public adapter or a declared downstream entry whose `observed_by` assertion passed — reaches realizing code. Do not create a sibling Journey Validation by hand, and do not use `loom edge exercises` on compiler-owned Journey validations.

`diagnose` uses the same compiled profile but does not settle proof. It alone accepts repeatable typed input overrides as `--input key=json`. `freeze` records the current observed result as the selected profile's baseline. `drift` reports semantic, derivation, surface, compiler, or baseline hashes that no longer agree.

Schema v12 is a deliberate rebuild boundary. Older graphs and exports are refused untouched because an executable proof specification cannot be translated into authored user meaning without inventing judgment. Rebuild in this order: initialize a new graph; register repository code; use `loom bootstrap suggest` for clues; author and add `loom.journey/v1` roots; derive technical Intents; obtain the human decision for each exact manifest; implement and ground those Intents; build and accept the real CLI surface; then compile and run the proof profile.

---

## Quality commands

### Seeding packs

```text
loom rule seed <pack> [--json]
```

Seedable packs: `iso5055`, `service`, `web-ui`, `data`, `concurrency`, `docker`. Seeded rules ship with `inspection_guide`, `detection_hints`, `evidence_template`, and passing/failing few-shot examples; `loom detect` recommends only from this seedable list.

### Custom rules

```text
loom rule add --name "<name>" [--description "<desc>"] [--category "<category>"] [--json]
loom rule update <rule> --reason "<why>"
  [--description "<desc>"] [--category "<category>"] [--severity <severity>] [--effort <effort>]
  [--guide "<inspection_guide>"] [--hint "<detection_hint>"] [--pattern "<regex>"]
  [--json]
loom rule remove <rule> [--json]
loom rule unlink <rule> <intent> [--json]
loom rule list [--limit N] [--offset N] [--json]
loom rule show <rule> [--json]
loom rule suppress <rule> --excerpt "<matched text>" --reason "<why>" [--json]
loom rule unsuppress <rule> --key <hash-prefix-or-excerpt> [--json]
loom rule suppressions [<rule>] [--json]
```

Custom-rule creation is intentionally small in the current binary. Rich guidance fields are provided by seeded packs and visible through `rule show`.

`rule suppress` is hit-level adjudication: it judges one pre-screen hit as not-what-the-rule-means, keyed by the content hash of the matched text — never its position. Judged once, the suppression answers the same matched text on every future scan (any rule×intent pair, any shifted line, any moved file): the passing-verdict gate counts it answered and quality packets stop re-serving it (a `suppressed` count stays visible). When the matched text itself changes, the hash no longer matches and the hit re-opens automatically — invalidation is the key, not a sweep. `rule suppressions` is the auditable ledger of these judgments; suppressions are journaled (`hit_suppressed`/`hit_unsuppressed`).

### Recording verdicts

```text
loom rule verdict <rule> <intent> passing|failing|independent
  [--criterion "<what compliance means here>"]
  [--evidence "<what inspection found>"]
  [--confidence <0-1>]
  [--json]
```

A verdict at component altitude covers descendants unless a leaf needs its own verdict. `independent` means measured and not applicable; it requires evidence.

Quality `PromptContract`s embed rule metadata in the real serialized shape:

- `prompt_contract.evidence_template`
- `prompt_contract.examples`
- detection hints folded into `prompt_contract.allowed_actions` as `hint: ...`
- prefilled `write_back` with single-quoted rule and intent names

---

## Hypothesis commands

```text
loom hypothesis add --name "<name>" --claim "<what is wrong now>" --target <intent>
  [--proposal "<the change>"]
  [--predicted-outcome "<measurable result>"]
  [--json]

loom hypothesis update <hypothesis> --reason "<why>"
  [--claim "<new claim>"] [--proposal "<new proposal>"] [--predicted-outcome "<new outcome>"]
  [--json]
loom hypothesis prove <hypothesis> supported|refuted [--evidence "<what code showed>"] [--json]
loom hypothesis adopt <hypothesis> [--spawned <planned-intent>] [--json]
loom hypothesis reject <hypothesis> --reason "<why>" [--json]
loom hypothesis remove <hypothesis> [--json]
loom hypothesis show <hypothesis> [--json]
loom hypothesis list [--limit N] [--offset N] [--json]
```

Hypotheses are invisible to coverage and maturity until adopted. Speculation never counts as graph truth.

---

## Inbox commands

```text
loom inbox add "<raw text>" [--source <source>] [--link <ref>] [--json]
loom inbox list [--status new|routed|rejected|duplicate|deferred] [--limit N] [--offset N] [--json]
loom inbox show <key> [--json]
loom inbox mark <key> routed --reason "<destination-kind>:<stable-node-id>" [--json]
loom inbox mark <key> rejected|duplicate|deferred --reason "<why>" [--json]
loom inbox remove <key> [--json]
```

The single free-form input boundary. Raw text enters as `InboxItem`; allowed sources are `human|external|support|import`. Evidence-backed observations belong in `loom finding add`; product decisions belong in `loom question add`. A routed disposition must name one supported typed landing—`existing_journey`, `new_journey`, `existing_intent`, `hypothesis`, `spike`, or `external_research`—and the exact stable node ID returned by its creation or lookup command. Other dispositions take a concrete prose reason.

---

## Question commands

```text
loom question add "<question>" --intent <intent> [--json]
loom question list [--status open|answered|withdrawn|duplicate|deferred] [--limit N] [--offset N] [--json]
loom question show <key> [--json]
loom question answer <key> --answer "<answer>" [--json]
loom question close <key> withdrawn|duplicate|deferred --reason "<why>" [--json]
loom question remove <key> [--json]
```

Questions are first-class `Question` nodes linked to intents by `questions` edges. `open` questions keep the completeness questions axis open; `answered`, `withdrawn`, `duplicate`, and `deferred` close it.

---

## TaskRecord commands

```text
loom task add "<title>" [--kind spike|investigation|experiment|review|chore|research] [--target <intent>] [--why-external "<reason>"] [--preferred-source "<guidance>"]... [--json]
loom task source-add <task-id> --url <actual-page> --title "<title>" --publisher "<publisher>" --source-kind official_docs|standard|regulation|maintainer|primary|secondary --quote "<substantive exact quote>" [--published-at <RFC3339>] [--fresh-until <RFC3339>] [--json]
loom task start <task> [--json]
loom task close <task> --result "<summary>" [--json]
loom task abandon <task> --reason "<why>" [--json]
loom task remove <task> [--json]
loom task show <task> [--json]
loom task list [--limit N] [--offset N] [--json]
```

TaskRecords guide work but do not certify truth. New governed research records carry
`kind=research,research_schema=1`; an unmarked legacy `kind=research` record remains
a generic TaskRecord. Governed research requires
`--why-external`; preferred-source guidance may repeat. The host LLM browses—Loom
contains no browser client. Search results are discovery only: `source-add`
accepts strict provenance for actual pages read, stamps retrieval using Loom's clock,
computes an exact-quote `fnv:` fingerprint, and deterministically ignores a duplicate
URL+quote fingerprint. Research closes when at least one source is currently usable.
Its targeted outcome is a reference note; work packets resolve the immutable TaskRecord
and render its dated provenance dynamically, suppressing stale recommendations and
offering successor research. Sources never become Fact evidence or verification.
Results may honestly be conflicting, inconclusive, or require expert review.

---

## Proposal commands

```text
loom proposal add --title "<title>" (--file <path> | --text "<raw proposal>") [--json]
loom proposal list [--limit N] [--offset N] [--json]
loom proposal show <proposal> [--json]
loom proposal remove <proposal> [--json]

loom proposal item add <proposal> --text "<item>" [--kind <kind>] [--json]
loom proposal item adopt <proposal> <number> [--as <intent|task>] [--name "<spawned name>"] [--description "<spawned description>"] [--json]
loom proposal item defer <proposal> <number> --reason "<why>" [--json]
loom proposal item reject <proposal> <number> --reason "<why>" [--json]
```

Proposals are durable plan/RFC artifacts. Adoption is a one-way transition that can optionally spawn ordinary Loom work.

---

## Judgment inbox commands

```text
loom judgment propose <ratify|reject|redefine> <intent> --evidence "<why the judgment holds>" [--description "<replacement statement>"] [--json]
loom judgment digest [--all] [--json]
loom judgment confirm <id> [--human-decision "<exact human answer>"] [--json]
loom judgment withdraw <id> --reason "<why>" [--json]
```

The inbox for human-only judgments (INV-8). An LLM that discovers a candidate — a junk intent that should be rejected, an intent ready to ratify, a statement that no longer matches the code — STAGES it with the evidence a human will review. Staging is ungated (recommending is not deciding) and deduplicated: one live proposal per (kind, intent). `digest` is the human's review surface, oldest first, each entry printing its exact confirm command. `confirm` executes the SAME gated write the direct command demands: ratify/reject require the human's answer (mediated via `--human-decision`, or the typed challenge at a solo terminal) and land through the identical `ratify_intent_from_human`/`reject_intent_from_human` chokepoints; redefine applies the staged statement with the normal ripple (ratification stales to `needs_reconfirmation`). If the gate refuses, the proposal stays staged and nothing else moved; a decided proposal can never be re-confirmed. `withdraw` retires a wrong candidate with a substantive reason. `loom status` notes the staged count next to the queue line.

---

## InterfaceSurface commands

```text
loom surface add --name "<name>" [--kind http|cli|ui_route|message_topic|sdk_method|internal_module|storage]
  [--identity "<method+path, command, topic, symbol>"]
  [--codefile <codefile>]
  [--json]
loom surface show <surface> [--json]
loom surface update <surface> [--kind <kind>] [--identity "<identity>"] [--codefile <codefile>] [--json]
loom surface remove <surface> [--json]
loom surface list [--limit N] [--offset N] [--json]
```

```text
loom surface gaps [--json]
```

Surface-plane gaps: declared surfaces that expose no codefile (`unexposed_surface`) and surfaces never exercised by a validation `calls` edge (`uncalled_surface`). Reports `armed: false` when no surfaces are declared.

---

## Audit and integrity

```text
loom coverage [--json]
loom completeness [<intent>] [--json]
loom scan run [<name>] [--json]   (adapters are registered in "Graph init and travel")
loom doctor [--json]
loom audit [--json]
loom audit incident accept <actor@YYYY-MM-DDTHH:MM> --claim ratification|adjudication \
  --reason "<why this historical integrity exception is accepted>" \
  [--human-decision "<the human's exact answer>"] [--json]
loom audit incident list [--json]
loom audit incident show <actor@YYYY-MM-DDTHH:MM> --claim ratification|adjudication [--json]
loom smells [--json]
loom debt [--json]
loom debt promote <cluster-id> --evidence <TEXT> [--confidence <0..1>] [--json]
loom whoami [--json]
loom limits [--json]
```

- `limits`: every enforced resource limit with its value, scope, and remedy. Violation errors name the same limit (e.g. `killed: exceeded timeout_secs=300`, `evidence exceeds max_spans=16`, `graph lock exceeded lock_wait_ms=2000`, `graph lock exceeded read_lock_wait_ms=10000`), so a failure message plus this list always yields the threshold and the way to change the outcome. Ordinary validation execution uses its registered timeout policy; Journey execution uses the compiled profile and surfaced operation policy.
- `coverage`: vertical spine — intent tree shape, leaf grounding, file ownership by live realizing `implements` edges, unaccounted files after ignores.
- `completeness`: Definition-of-Complete scorecard for one intent or all feature intents; non-question axes can be waived through `loom intent waive` and re-open on intent redefinition.
- `scan`: external diagnostic adapters; `run` turns registered-codefile diagnostics into derived findings for triage, and disappeared diagnostics resolve on the next run.
- `doctor`: schema conformance, provenance, evidence vacuity, role-gate audit; exits non-zero on any issue. Includes `consumes_without_seam` when a settled `consumes` grounding has neither a locator nor a criterion naming a seam.
- `audit`: the self-fabrication detector. A historical judgment burst with no
  contemporaneous authorization may be accepted only through the human-gated
  `audit incident accept` command. The disposition binds the exact live burst
  digest and remains permanently visible through `list`/`show`; it does not
  stamp batch IDs, rewrite timestamps, or retrospectively authorize the
  judgments. Imported disposition records are disclosed as history but never
  suppress the local audit finding; the local human must accept independently.
  Includes `writes_during_proof`: compiled Journey execution brackets its
  window in the journal (`proof_execution_started`/`_ended`), and an asserted
  fact written by `solo` inside that window is flagged — journey children run
  env-scrubbed, so a child loom reads as solo with full authority, and a solo
  write mid-proof looks like a proof writing the graph it is proving. Lane
  writes (`llm:<role>`, parallel drivers) and post-window settlement writes
  are not flagged. A run that died mid-execution leaves its window unclosed:
  no fact is indicted (an open-ended window would indict everything after it),
  but once the recorded pid is gone the window itself surfaces as
  `unclosed_proof_window` — unauditable is a finding, not a blind spot. An
  open window whose process is still alive is a run in flight and stays quiet.
- `smells`: structural signals from graph shape, each with a remedy. Sync materializes every smell as a derived Finding (content-addressed by its subject ids), so smells are served by the triage queue and adjudicated with `loom finding verdict <id> <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason "…"`; the adjudication is durable across syncs and shown by `loom smells`. Includes `pack_drift` when a seeded/builtin rule body differs from the shipped pack definition (remedy: `loom rule seed <pack>` to re-baseline, or adjudicate the customization `justified` or `deferred`) and `consumer_owned_file` when a file's sole realizing owner is an intent whose other realizing files live in a different top-level directory cluster; inspect sibling slice vs mis-owned consumer — the remedy names the edge. Includes `vague_intent` when an active intent's description leans on a hedge term (`handles`, `properly`, `correctly`, `robustly`, …) and names no observable outcome (no action verb, digits, literals, paths, or "by <doing>") — a falsifiability lint on the intent plane: every verdict against a mushy description is judgment theater, so either reword it with `loom intent update --description --reword` or adjudicate the finding `justified` for a deliberate summary-level intent.
- `debt`: advisory statistical cluster feed (`size_outlier` LOC outliers + git-history `co_change` when available) with stable `cluster_id`, deterministic order (impact desc, kind asc, id asc), and git-less degradation to size outliers only; never required work. Explicit `loom debt promote <cluster-id> --evidence <TEXT> [--confidence <0..1>]` creates exactly one asserted Finding (`source: debt_promotion`) that enters ordinary finding triage; identical evidence/confidence is idempotent, conflicting re-promotes error.
- `whoami`: acting authorization identity, executor profile, and lane enforcement. Set
  `LOOM_AGENT=llm:<role>` for write authority and, independently,
  `LOOM_AGENT_PROFILE=<worker-profile>` (for example `loom-auditor`) for
  attribution. Profiles use 1–128 ASCII identifier characters and grant no lane
  permissions. JSON reports their source and verification status; environment
  profiles are self-declared and therefore `verified: false`. Unset or explicit
  `LOOM_AGENT=solo` is solo; bare/empty `llm`, bare role names, and unknown values
  are rejected. Identity is resolved once before locking and reused by every
  provenance write in the invocation.

---

## Note commands

```text
loom note add <target> --text "<text>" [--kind decision|context|warning] [--json]
loom note list [<target>] [--limit N] [--offset N] [--json]
loom note remove <id> [--json]
```

Durable notes attach to any node (by name, id, or unique fragment) or any edge (by id or unique id prefix) — adjudications attach to claims, and claims live on edges too. On a key that could name both, the node wins.

---

## Vocab and layer commands

```text
loom vocab add <term> [--why "<contrastive definition>"] [--json]
loom vocab remove <term> [--json]
loom vocab rename <from> <to> --reason "<why>" [--json]
loom vocab list [--json]
```

```text
loom layer order [<top> <next> ... <bottom>] [--json]
loom layer list [--json]
loom layer clear [--json]
```

Layer order arms layering-violation detection. Vocab terms support duplicated-responsibility and vocabulary-drift signals.

---

## Wiki commands

```text
loom wiki plan <title> --path <path> [--covers <intent>]... [--json]
loom wiki next [--json]
loom wiki record <title> [--json]
loom wiki list [--json]
loom wiki remove <title> [--json]
```

Reader-first wiki pages tracked as a projection of the graph: the graph governs **truth and freshness**, never layout, and an agent (not loom) writes the prose. `plan` creates or re-grounds a draft page and the intents it documents (`Documents` edges); `next` emits a verified brief — the documented intents' descriptions, groundings, and proof status — for the next page that needs writing (a draft, or a stale page whose documented scope drifted); `record` marks an authored page fresh by stamping the scope fingerprint of everything it documents (gated on the prose actually existing at the page's path). `sync` stales a page precisely when a documented intent, its code, or its proof drifts.

---

## Federation commands

```text
loom graph link <path-to-loom.graph.json> [--name <alias>] [--json]
loom graph unlink <alias-or-graph-id> [--prune] [--cascade] [--json]
loom graph prune-orphans [--alias <alias>] [--cascade] [--json]
loom graph list [--json]
```

Cross-graph federation over committed exports; see "Graph init and travel" for the `UpstreamIntent` shadow-node model, permanent-unlink cleanup (`--prune` / `prune-orphans`), and `loom edge depends-on` for cross-graph claims.

---

## Removed / deferred names

These are **not** current shipped commands or flags. Do not emit them from prompts or examples unless explicitly discussing absence:

- removed/deferred from `next`: `--take`, `--compact`, `--slice`
- removed/deferred command families: impact preview, hotspots, dig
- removed/deferred subcommands: intent context, edge unimplement, vocab merge, inbox normalize
- removed/deferred flags: `guide --mode`, `import --as-planned`
- shipped since this list was written (do **not** treat as deferred): batch writes (`loom apply`), wiki projection (`loom wiki`, with the verb set above — the older `generate/verify/publish/update` design in `wiki-projection.md` was superseded), and federation (`loom graph link/unlink/list`, `graph unlink --prune`, `graph prune-orphans`, `loom edge depends-on`)
- removed legacy (grammar convergence): top-level `loom validate` (→ `loom validation run`), `validation mark --result` (→ `validation verdict <outcome>`), `rule verdict --status` (→ positional outcome), `hypothesis prove --verdict` (→ positional outcome), `validation delete`/`surface delete` (→ `remove`), `rule ungovern` (→ `rule unlink`), the `loom saga` alias, the `saga` validation type, and the `saga:` spec name key
- removed by the schema-v12 Journey-root break: executable Journey artifact fields and transport-specific steps; Journey metadata flags on `validation add`; `journey coverage`, `journey invariant`, and `journey prompt`; `journey run <artifact>` and transport override flags. Use the authored-root lifecycle documented above.

---

## Output conventions

Mutating commands support `--json`. In JSON mode they emit one object containing the command payload plus at least these pulse fields (GraphState has the full fields shown in `llm-driver.md`):

```json
{
  "next_step": "loom status",
  "graph_state": { "planned": 0, "stale": 0, "uninspected": 0, "low_confidence": 0, "adversarial_review": 0, "inconclusive_challenges": 0, "review_independence_warnings": 0, "open_questions": 0 }
}
```

In text mode the human summary ends with:

```text
next: <step>
```

List commands bound output with `--limit` and page with `--offset` (0-based) where the binary exposes it (`intent`, `codefile`, `edge`, `rule`, `validation`, `hypothesis`, `surface`, `proposal`, `task`, `note`, `inbox`, `question`, `wiki`, `journey`). Text output prints an explicit footer — `… showing N–M of TOTAL. More items exist; rerun this list command with --offset M to see the next page.` — so a human or agent immediately knows that rows remain and how to retrieve them.

**Breaking in 0.28.0:** paginated `list --json` output is an object rather than a bare array. Rows are under `items`; `pagination` reports `offset`, `limit`, `returned`, `total`, `has_more`, and `next_offset`. When `has_more` is false, `next_offset` is `null`. Migrate consumers from `response[]` to `response.items[]`:

```json
{
  "items": [],
  "pagination": {
    "offset": 0,
    "limit": 50,
    "returned": 0,
    "total": 0,
    "has_more": false,
    "next_offset": null
  }
}
```

Resolving a node by an ambiguous name or fragment errors with the full candidate list, each as `[<short-id>] <name>`, so a duplicate is addressable by id (`show`/`remove`) instead of leaving a bare count to guess from.
