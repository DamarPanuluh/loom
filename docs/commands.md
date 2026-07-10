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

Graph identity, maturity ladder, queue counts, validation summary, code ownership, and the compass. `graph_state.low_confidence` is the count served by `loom next --mode review`; `graph_state.open_questions` is the count of open first-class `Question` nodes.

`loom status` now prints a true per-queue backlog line (`fix=N validate=N build=N coverage=N quality=N analyze=N prove=N triage=N review=N elaborate=N`) computed by the same partition that `loom next` serves, plus a note when human questions are waiting. In JSON mode the output gains a `queues` object with the same counts.

```text
loom session [--json]
```

Turn-zero entry when the user says "use loom" without a specific task. Returns an offer menu backed by live queue counts, open questions, and one recommended command.

```text
loom next [--mode <queue>] [--all] [--json]
```

Highest-priority `WorkItem` + `PromptContract` for the current queue. Without `--mode`, routes by compass priority.

```text
--mode: build | coverage | fix | analyze/discovery | validate | quality | prove | triage | review | elaborate
--all:  closeout view — the top item of every queue at once
--mode <m> --all:  the FULL depth of one queue — every item it would serve, in
                   priority order (entry 1 is what `loom next --mode <m>` serves),
                   as lightweight rows (target + reason + effort, no packet). Use
                   it to page a queue that `loom status` reports as hundreds deep;
                   work an item with the singular `loom next --mode <m>`.
```

Queue partition is deliberately disjoint:

- `fix`: every failing asserted edge — strictly root-cause repair. A fix packet never carries verdict authority: repair the source, run `loom sync`, and the owning lane re-measures.
- `analyze`: uninspected and stale non-`governs`/non-`validates` asserted claims. Stale claims are served first — a settled truth that broke misleads readers; an uninspected claim only waits.
- `quality`: uninspected or stale `governs` only. Failing `governs` routes to `fix`.
- `validate`: uninspected or stale `validates` only. Failing `validates` routes to `fix`.
- `coverage`: registered codefiles with no live realizing owner. Files grounded only by `consumes`, `configures`, or `verifies` edges remain unowned. If the file is missing from disk, the packet is a dedicated missing-file contract: re-ground any successors, then unregister the dead registration — do not attempt to read a ghost.
- `review`: asserted `passing` or `independent` verdicts with `0 < confidence < 0.7`, lowest confidence first. The work item keeps the edge kind's registry owner as `owner_role`, but the mindset is independent re-inspection.
- `elaborate`: the most-incomplete user-visible feature intent by Definition-of-Complete scorecard. The packet embeds the open axes and routes the builder to add missing scenarios/prerequisites/proofs/journey coverage, raise product questions, or waive non-question axes with reasons.

Fixer lane safety: fix the source and run `loom sync`; sync re-opens the claim (`needs_reverification` plus any `stale_cause` facet), and the owning lane re-measures it.

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
  "graph_state": { "planned": 0, "stale": 0, "uninspected": 0, "low_confidence": 0, "open_questions": 0 }
}
```

```text
loom find [--limit N] [--exact] [--tag <term>] [--where KEY=VALUE] ["<query>"] [--json]
loom explain <intent> [--json]
```

`find` searches intents/codefiles/quality rules by keyword (fuzzy) or `--exact` whole-name match. `--tag` and repeatable `--where KEY=VALUE` filter by vocabulary tag and allowlisted facets (`visibility`, `level`, `aspect` — also listed in `loom schema`). Filters AND together; query may be omitted when filters alone select the set.

`explain` is a read-only neighborhood brief for one intent: description, facets/tags, groundings, 1-hop related intents, validations, completeness scorecard, open questions. It is **not** a `loom next` work lane.

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

---

## Graph init and travel

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

Runs a discovery pass then recomputes the structural plane from disk. The discovery pass expands all remembered codefile globs (from prior `codefile add '<glob>'` calls) and registers any new files that appeared since the last run, respecting `loom ignore` exclusions — so a single `loom sync` both discovers and extracts without a separate `codefile rescan`. The structural recompute is content-hash based — mtime churn never false-flags. Sync stales Targets (`hypothesis -> intent`) edges, records `stale_cause` facets on every staled edge, deterministically resets validations, downgrades never-reached previously-passing journey steps to `needs_reverification` when a journey run fails earlier, reopens realizing `implements` groundings on changed files **symbol-scoped**, and reopens non-realizing `implements` groundings only when their seam locator drifts. Symbol-scoped means: sync keeps a per-symbol fingerprint map for every extracted file, and a realizing grounding whose `--locator` resolves to a symbol the change did not touch is spared instead of re-opened (reported as `edges_spared`); a grounding with no locator, an unresolvable locator (routes, config keys), or a file synced before fingerprints existed stales whole-file as before, and same-named symbols fold into one fingerprint so an ambiguous locator is never spared past a real change. The `stale_cause` is precise (`symbol 'x' in <file> changed`) and graded by evidence anchoring: when the recorded verdict cited `file:line` spans, the cause says whether every cited span is still intact (`cited evidence intact, cheap re-confirm`) or not (`cited evidence rewritten, full re-inspection`) — and a rewritten cited span re-opens the grounding even when its locator symbol is untouched. As a byproduct sync refreshes `loom.graph.json` when — and only when — the file already exists and has drifted, so the committed export stays fresh without a separate `loom export` call (it never creates an untracked file, and a fresh export is left byte-identical). JSON output includes `new_files` (list of discovered paths) and `new_observed` (count of observed-mode discoveries) alongside the structural counts (`edges_staled`, `edges_spared`, …).

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
loom graph unlink <alias-or-graph-id> [--json]
loom graph list [--json]
```

Link an upstream graph via its committed `loom.graph.json` export. `link` reads the export, registers the upstream in portable config (`upstream_graphs` meta key), and creates `UpstreamIntent` shadow nodes for each intent in the upstream graph. The `--name` alias defaults to the upstream graph's name; aliases must be unique. Shadow nodes are named `upstream/<alias>/<intent-name>`.

`UpstreamIntent` is a distinct node type — invisible to all local intent queries (status counts, maturity ladder, completeness scorecards, work queues, coverage gates). It follows the CodeFile truth-class pattern: the node itself is asserted (created by `graph link`, body carries provenance `{graph_id, node_id, alias}`), while live upstream state (`upstream_description`, `upstream_status`, `upstream_content_hash`) lives as derived facets rebuilt every sync from the upstream export. `wipe_derived + sync` converges (INV-2).

`loom sync` runs a federation pass after the codefile discovery pass: for each linked upstream, it reads the export file, compares a content hash against a cached value, and on change parses the export, diffs against shadow nodes, creates new shadows for new upstream intents, updates derived facets on changed ones, marks deleted upstream intents with `upstream_missing=true`, and stales all `DependsOn` edges whose upstream target changed. An unchanged upstream adds only one `stat()` + hash comparison — negligible overhead.

`unlink` removes the upstream registration but intentionally leaves shadow nodes orphaned (never auto-deleted). `loom doctor` flags orphaned upstream intents (`orphaned_upstream_intent` issue kind).

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

Read commands (`loom status`, `loom next`) open the graph under a **shared** advisory lock with SQLite `query_only`, so several agents can query one graph at the same time and never block each other. Writers still take the lock exclusive, but only for their (short) transaction. `loom scan` runs its external adapter commands with **no** lock held — reading adapter config under a shared lock, executing the subprocesses lock-free, then reopening for a brief exclusive write to reconcile findings — so one long scan no longer freezes every other agent for the duration of its subprocesses.

```text
loom detect [--json]
```

Detects repo languages and recommends seedable quality packs only. Available packs are: `iso5055`, `service`, `web-ui`, `data`, `concurrency`, `docker` (29 rules total across the shipped pack set).

```text
loom bootstrap suggest [--json]
```

Cold-start assist when the graph has **registered codefiles and zero intents**. Scans derived signals (top-level `src/` modules from registered codefiles, `tests/*.rs`, README `##` headings) and writes a **Proposal** whose items are candidate pillar intents (suggested name/description/level/visibility). The operator adopts with `loom proposal item adopt <proposal> <n> --as intent` → `lifecycle=planned` only.

Hard rules: never creates `implements`/`governs`/`validates` verdicts; never sets `implemented`; refuses if any intent already exists. `loom session` offers this command when `intents == 0 && codefiles > 0`.

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
loom policy gate-add <lane> [--json]
loom policy gate-remove <lane> [--json]
loom policy reset [--json]
```

Read or set the evidence policy. `set-floor` sets the review-confidence floor (a fraction in `[0.0, 1.0]`) below which a recorded verdict is routed to `loom next --mode review`; `gate-add`/`gate-remove` move an owner lane (`builder | analyzer | fixer | validator | quality`) in or out of the human-gated set described in `llm-driver.md`. The policy persists to portable `config.evidence_policy` and travels with the export; absent config means the shipped defaults, and `reset` drops the config to restore them.

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

```text
loom intent update <intent> --reason "<why>"
  [--description "<new>"] [--reword]
  [--name <new-name>]
  [--level system|component|feature|cross_cutting]
  [--visibility user_visible|internal]
  [--aspect happy|sad|fallback|edge_case]
  [--lifecycle planned|implemented|needs_change]
  [--json]
```

`update` is the single mutation verb. The ripple rule lives in the fields, not in command choice: a `--description` change is a redefinition and ripples one hop (passing/independent edges become `needs_reverification`, linked validations reset, completeness waivers are cleared so waived axes re-open, and old wording is preserved in decision notes); `--reword` is same meaning, clearer words, no ripple. `--name`, `--level`, `--visibility`, `--aspect`, and `--lifecycle` never ripple. Every update records `--reason`.

```text
loom intent confirm <intent> [--json]
loom intent retire <intent> --reason "<why>" [--replaced-by <intent>] [--json]
loom intent remove <intent> --reason "<why>" [--json]   (mistakes only; refuses intents that still have hierarchy children)
loom intent reactivate <intent> --reason "<why>" [--json]
loom intent waive <intent> scenarios|prerequisites|boundary|proof|journey --reason "<why>" [--json]
loom intent show <intent> [--json]
loom intent list [--limit N] [--offset N] [--json]
loom intent tag add <intent> <term> [--json]
loom intent tag remove <intent> <term> [--json]
```

`confirm` ratifies meaning. `retire` sets status to deprecated and removes the intent from active computation while preserving history. `waive` records a reasoned waiver for a non-question completeness axis (`scenarios`, `prerequisites`, `boundary`, `proof`, `journey`); if the intent is later redefined through `intent update --description`, waiver facets are cleared and those axes are scored again. Open questions must be answered with `loom question answer` or closed with `loom question close`.

---

## Edge commands

```text
loom edge implement <intent> <codefile> [--role realizes|consumes|configures|verifies] [--locator "<symbol>"] [--json]
loom edge call <validation> <surface> [--json]
loom edge remove <edge-id> [--reason "<why>"] [--json]
loom edge set-locator <edge-id> <locator> [--json]
loom edge set-role <edge-id> realizes|consumes|configures|verifies --reason "<why>" [--json]
loom edge rehome <edge-id> --to "<successor intent>" --reason "<why>" [--json]
loom edge show <edge-id> [--json]
loom edge list [--limit N] [--offset N] [--json]
loom edge depends-on <intent> <upstream-shadow> [--json]
```

`edge implement` defaults to `--role realizes`; only realizing groundings own coverage. Use `consumes` when a file calls behavior across a seam, `configures` when it supplies configuration, and `verifies` when it checks behavior elsewhere. `edge set-role` records a decision note and reopens a settled edge with `stale_cause=role_changed...` when the role changes. `edge rehome` supersedes the old grounding with a `superseded_by` facet, creates or reuses the successor grounding with the old locator and role, and reopens it with `stale_cause=rehomed...`. `edge show` prints edge facets; JSON includes a `facets` object. `edge remove` refuses derived edges. `edge call` records that a validation exercises an interface surface; sync resets that contract when the code behind the surface changes.

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

Verdict commands inspect relationship/grounding claims. `independent` means measured and not related/applicable; it requires real evidence. **Evidence anchoring:** every verdict-recording command (`edge verdict`, `edge explore`, `rule verdict`, `validation verdict`, `apply` batches) parses `file:line[-line]` citations out of `--evidence`; each citation that resolves to a real file under the graph root is stamped with a fingerprint of the cited lines (asserted `evidence_spans` edge facet) so sync can later grade a re-open as "cited span intact" vs "rewritten". Citing an existing file at lines that do not exist rejects the verdict — evidence must describe bytes someone can read. Citations that resolve to nothing (URLs, tool output, deleted paths) are ignored, never guessed at.

---

## CodeFile and coverage exclusion commands

```text
loom codefile add <path-or-glob> [--observed] [--json]
loom codefile rescan [--json]
loom codefile remove <path-or-key> [--json]
loom codefile show <path-or-key> [--json]
loom codefile list [--limit N] [--offset N] [--json]
```

`--observed` registers files the graph monitors but does not own (vendored or upstream code): `loom sync` scans them and surface/contract staleness still ripples, but they carry no ownership, coverage, or build obligations — the per-file counterpart of the graph-level observed mode. Re-adding an already-registered file with `--observed` marks it observed. A glob added with `--observed` is remembered, so `codefile rescan` and `loom sync` register files that appear under it later as observed too; a file matched by both an owned and an observed glob registers as owned.

Glob-based registration (`codefile add '<glob>'`, `codefile rescan`, and the discovery pass inside `loom sync`) respects `loom ignore` exclusions: a file matching an ignore glob is silently skipped during glob expansion. Explicit literal adds (`codefile add path/to/file.rs`) always go through — explicit intent overrides ignore. Files already registered before an ignore glob is added stay registered (ignore never deletes nodes; it only gates future discovery).

`show` returns ownership, locators, imports/symbols/metrics, governing rules, findings, and stale-edge context.

```text
loom ignore add '<glob>' --reason "<why>" [--json]
loom ignore remove '<glob>' [--json]
loom ignore list [--json]
```

Coverage exclusions live in the graph with a recorded reason. `loom coverage` honors them, and glob-based codefile discovery (rescan / sync) skips files matched by ignore globs.

---

## Validation and proof commands

```text
loom validation add --name "<name>" --intent <intent>
  [--type test|assertion|benchmark|manual_check|journey|scenario|contract]
  [--command "<cmd>"]
  [--proof-level L0|L1|L2|L3|L4|L5|L6]
  [--proof-kind journey]
  [--journey-id <id>]
  [--repo-native-kind <kind>]
  [--artifact <path-or-ref>]
  [--json]
```

Journey metadata flags require `--proof-kind journey`; the canonical journey creation path is `loom journey add <spec>`.

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

`journey` is the canonical family for flow/composition proofs.

```text
loom journey add <spec.json|spec.yaml|http-contract.json> [--json]
loom journey list [--limit N] [--offset N] [--json]
loom journey run <spec.json|spec.yaml|http-contract.json> [--base-url <url>] [--json]
loom journey diagnose <spec.json|spec.yaml|http-contract.json> [--base-url <url>] [--json]
```

`add` creates a `Validation` whose body uses `type: "journey"`, `proof_level: "L5"`, `proof_kind: "journey"`, and command `loom journey run <artifact>`. It links resolved step intents with `validates`. It does NOT link steps with `sequence` — a spec's step order is a test script, not a domain claim; assert ordering deliberately with `loom edge relate sequence` if it is real. Unresolved step intents do not fail the add: they are reported as `unmatched_steps` (both native specs and HTTP-contract routes).

Native specs accept JSON or YAML:

```json
{
  "journey": "checkout happy path",
  "base": "{{ env.BASE_URL }}",
  "steps": [
    {
      "name": "create cart",
      "intent": "cart can be created",
      "request": { "method": "POST", "url": "/carts", "query": {}, "json": { "sku": "A" } },
      "expect": { "status": 201, "body": { "ok": true }, "exists": ["$.id"] },
      "capture": { "cart_id": "$.id" }
    }
  ]
}
```

The spec name key is `journey`. A step is either **HTTP** (`request` + response expectations) or **CLI** (`run` + exit/stdout expectations) — not both:

```yaml
journey: door capture happy path
steps:
  - name: capture utterance
    intent: an operator captures a topic through door and routes it from the landing menu
    run: "loom door 'ship faster checkout' --json"
    expect:
      exit_code: 0
      stdout_contains: ["landing_menu"]
    capture:
      inbox_id: "$.captured.id"
```

CLI steps run via `sh -c` with the graph root as cwd (so repo-local binaries and fixtures resolve). `journey run` releases the exclusive graph lock while steps execute, then reopens to stamp verdicts — so a step may invoke the same repo's CLI (or any other graph writer) without deadlocking. `expect.exit_code` defaults to `0`. `body` / `exists` / `capture` on a CLI step parse stdout as JSON. HTTP contract specs use `name`, optional `base`/`auth`, and `routes`; route fields include `method`, `path`, optional `intent`/`name`, `success_status`, `query`, `example_request`, `response_fields`, and `extract`. Bearer auth injects `{{ env.LOOM_JOURNEY_AUTH_TOKEN }}`.

`run` records graph verdicts. A failing boundary records the exact failed expectation and reopens previously-passing never-reached later steps. `diagnose` executes directly without graph writes and is useful for missing env/auth/404/template failures. Both accept `--base-url`, which overrides the spec base and `{{ env.BASE_URL }}` for HTTP steps.

### Journey coverage

```text
loom journey coverage add <intent> --name <name> --flow <flow>
  [--description <description>]
  [--runner-ref <path-or-symbol>]
  [--test-ref <path-or-symbol>]
  [--contract-artifact <path>]
  [--json]
loom journey coverage update <coverage> --reason "<why>"
  [--runner-ref <path-or-symbol>] [--test-ref <path-or-symbol>] [--contract-artifact <path>]
  [--json]
loom journey coverage remove <coverage> [--json]
loom journey coverage list [--limit N] [--offset N] [--json]
loom journey coverage discover [--spawn-missing] [--json]
loom journey coverage drift [--json]
```

Coverage nodes mark flows that need a journey proof. Effective coverage is derived: covered iff the linked intent has a passing L5/L6 journey validation. Runner/test refs alone do not satisfy coverage; the proof must run.

### Journey invariant points

```text
loom journey invariant add <intent> --name <name> --field <field> --assertion <assertion>
  [--reason <reason>]
  [--json]
loom journey invariant update <invariant> --reason "<why>"
  [--field <field>] [--assertion <assertion>] [--asserts <intent>] [--reason-text <reason>]
  [--json]
loom journey invariant remove <invariant> [--json]
loom journey invariant list [--limit N] [--offset N] [--json]
```

Invariant points mark internal domain assertions that a journey should verify. `update --asserts <intent>` re-points the invariant at a different intent by replacing its `asserts` edge in place — the node, its history, and its notes stay intact, and the move is recorded as a decision note.

### Journey runner prompt

```text
loom journey prompt <intent> [--json]
```

Assembles a typed journey-runner prompt context from loom's code understanding of an intent. It is read-time assembly, not code generation.

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
```

Custom-rule creation is intentionally small in the current binary. Rich guidance fields are provided by seeded packs and visible through `rule show`.

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
loom inbox mark <key> routed|rejected|duplicate|deferred [--reason "<why>"] [--json]
loom inbox remove <key> [--json]
```

The single free-form input boundary. Raw text enters as `InboxItem`; allowed sources are `human|external|support|import`. Evidence-backed observations belong in `loom finding add`; product decisions belong in `loom question add`. Typed creation commands plus positional `mark` dispositions close the loop.

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
loom task add "<title>" [--kind spike|investigation|experiment|review|chore] [--json]
loom task start <task> [--json]
loom task close <task> --result "<summary>" [--json]
loom task abandon <task> --reason "<why>" [--json]
loom task remove <task> [--json]
loom task show <task> [--json]
loom task list [--limit N] [--offset N] [--json]
```

TaskRecords guide work but do not certify truth. Durable outcomes must be promoted to graph facts.

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
loom smells [--json]
loom debt [--json]
loom whoami [--json]
```

- `coverage`: vertical spine — intent tree shape, leaf grounding, file ownership by live realizing `implements` edges, unaccounted files after ignores.
- `completeness`: Definition-of-Complete scorecard for one intent or all feature intents; non-question axes can be waived through `loom intent waive` and re-open on intent redefinition.
- `scan`: external diagnostic adapters; `run` turns registered-codefile diagnostics into derived findings for triage, and disappeared diagnostics resolve on the next run.
- `doctor`: schema conformance, provenance, evidence vacuity, role-gate audit; exits non-zero on any issue. Includes `consumes_without_seam` when a settled `consumes` grounding has neither a locator nor a criterion naming a seam.
- `smells`: structural signals from graph shape, each with a remedy. Sync materializes every smell as a derived Finding (content-addressed by its subject ids), so smells are served by the triage queue and adjudicated with `loom finding verdict <id> <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason "…"`; the adjudication is durable across syncs and shown by `loom smells`. Includes `pack_drift` when a seeded/builtin rule body differs from the shipped pack definition (remedy: `loom rule seed <pack>` to re-baseline, or adjudicate the customization `justified` or `deferred`) and `consumer_owned_file` when a file's sole realizing owner is an intent whose other realizing files live in a different top-level directory cluster; the remedy names the edge. Includes `vague_intent` when an active intent's description leans on a hedge term (`handles`, `properly`, `correctly`, `robustly`, …) and names no observable outcome (no action verb, digits, literals, paths, or "by <doing>") — a falsifiability lint on the intent plane: every verdict against a mushy description is judgment theater, so either reword it with `loom intent update --description --reword` or adjudicate the finding `justified` for a deliberate summary-level intent.
- `debt`: advisory statistical cluster feed; never appears in required work queues until promoted.
- `whoami`: acting agent identity and lane enforcement.

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
loom graph unlink <alias-or-graph-id> [--json]
loom graph list [--json]
```

Cross-graph federation over committed exports; see "Graph init and travel" for the `UpstreamIntent` shadow-node model and `loom edge depends-on` for cross-graph claims.

---

## Removed / deferred names

These are **not** current shipped commands or flags. Do not emit them from prompts or examples unless explicitly discussing absence:

- removed/deferred from `next`: `--take`, `--compact`, `--slice`
- removed/deferred command families: impact preview, hotspots, dig
- removed/deferred subcommands: intent context, edge unimplement, vocab merge, inbox normalize
- removed/deferred flags: `guide --mode`, `import --as-planned`
- shipped since this list was written (do **not** treat as deferred): batch writes (`loom apply`), wiki projection (`loom wiki`, with the verb set above — the older `generate/verify/publish/update` design in `wiki-projection.md` was superseded), and federation (`loom graph link/unlink/list`, `loom edge depends-on`)
- removed legacy (grammar convergence): top-level `loom validate` (→ `loom validation run`), `validation mark --result` (→ `validation verdict <outcome>`), `rule verdict --status` (→ positional outcome), `hypothesis prove --verdict` (→ positional outcome), `validation delete`/`surface delete` (→ `remove`), `rule ungovern` (→ `rule unlink`), the `loom saga` alias, the `saga` validation type, and the `saga:` spec name key

---

## Output conventions

Mutating commands support `--json`. In JSON mode they emit one object containing the command payload plus at least these pulse fields (GraphState has the full fields shown in `llm-driver.md`):

```json
{
  "next_step": "loom status",
  "graph_state": { "planned": 0, "stale": 0, "uninspected": 0, "low_confidence": 0, "open_questions": 0 }
}
```

In text mode the human summary ends with:

```text
next: <step>
```

List commands bound output with `--limit` and page with `--offset` (0-based) where the binary exposes it (`intent`, `codefile`, `edge`, `rule`, `validation`, `hypothesis`, `surface`, `proposal`, `task`, `note`, `inbox`, `question`, `wiki`, `journey`, `journey coverage`, `journey invariant`). Text output prints a footer — `… showing N–M of TOTAL; --offset M for the next page` — so rows past the first page are never silently hidden; JSON output stays a bare array (page it via `--limit`/`--offset`). Resolving a node by an ambiguous name or fragment errors with the full candidate list, each as `[<short-id>] <name>`, so a duplicate is addressable by id (`show`/`remove`) instead of leaving a bare count to guess from.
