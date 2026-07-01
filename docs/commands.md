# loom v2 — Commands

Status: **full target surface** — this describes the complete intended CLI. Sections marked `[deferred]` are milestone 2 only. MVP rings cover: init/sync/export/import, intent, edge, codefile, validation, quality, hypothesis, saga (spec + runner), interface surfaces, inbox, task records, vocab, layer, audit/integrity, note.

---

## Orientation

```
loom status
```

Graph maturity + compass. Returns:

```text
maturity:           current rung on the ladder (seed → realized → proven → hardened → excellent)
coverage:           vertical spine gaps (ungrounded intents, unowned files, unrun proofs)
queue_counts:       per-queue counts (build, fix, validate, quality, analyze, align, prove, wiki, inbox)
human_gated:        queues requiring human presence + count
one_turn_plan:      lane, role, guide command, next queue for this session
alarms:             integrity violations that override normal routing
```

`graph_state.stale` is the stale/failing asserted-edge bucket for fix/build work. Finding triage is split separately: `untriaged` means no recorded finding verdict; `stale_findings` means a previously recorded finding verdict must be re-triaged because the flagged file changed.

```
loom session
```

Turn-zero entry when the user says "use loom" without a specific task. Returns an offer menu — every way this session could be spent, each backed by a live queue and count, one recommended. LLM asks human one question before proceeding.

```
loom next [--mode <queue>] [--take N] [--compact] [--slice <id>]
```

Highest-priority `WorkItem` + `PromptContract` for the current queue. Without `--mode`, routes by compass priority.

```text
--mode:     build | fix | validate | quality | analyze (alias: discovery) | align | prove | wiki | inbox | review | debt
--take N:   bulk read, N compact items (max 50); used for batch loops
--compact:  minimal projection — ids, edge, read set, one-line command; no full context
--slice:    scope to one intent subtree territory for parallel worker dispatch
```

Returns:

```text
work_item:      WorkItem shape (see llm-driver.md)
prompt_contract: PromptContract shape
next_step:      the exact command to run after acting
graph_state:    one-line graph pulse (maturity, open queues, alarms)
```

```
loom next --all
```

Closeout view: every queue at once — counts + top item per queue, vertical gaps, doctor health, top smells. The single answer to "what is left?"

```
loom find "<query>"
```

BM25 search across active intents, codefiles, surfaces, rules, and validations. Returns ranked hits with hierarchy chain, grounding locators, and stale-edge count. A miss distinguishes "not mapped" from "does not exist."

```
loom door "<utterance>"
```

Capture-first entry for free-form human/LLM language. Creates an `InboxItem`, assembles routing context (intent matches, compass pulse, landing menu), returns the normalized route for the LLM to act on.

```
loom guide [--mode greenfield|brownfield|refactor|port|seed] [--role <role>]
```

Self-contained driving protocol. Without flags: mental model, lifecycle, and the full loop. With `--mode`: entry-mode-specific population checklist. With `--role`: the role's mindset, allowed actions, forbidden actions, and evidence requirements (the PromptContract for that lane).

```
loom schema
```

Node types, edge kinds, property registry, tag vocabulary, status state machine, lifecycle model, and valid value enums. Generated from the schema registry; never drifts from the code.

---

## Graph init and travel

```
loom init [<path>] [--name <graph-name>] [--observed]
```

Creates `.loom/` and initializes `graph.sqlite`. `--observed` maps code the driver does not own (discovery/quality/validation work only; build/fix lanes disabled). Idempotent.

```
loom sync [<path>]
```

Recomputes the structural (derived) plane from disk. Content-hash based — mtime churn never false-flags. For every changed file:

- re-extracts symbols, imports, metrics
- marks stale: `implements` locators, `governs`, `validates` (Validation.last_result → not_run, linked validates edges → needs_reverification), asserted relationship edges, wiki page dependencies

Returns: N files changed, M edges staled, K validations reset, next_step.

```
loom export [--check]
```

Writes deterministic `loom.graph.json`. `--check` exits non-zero if committed export drifts from live graph. Run after any graph mutation; gate CI/pre-commit with `--check`.

```
loom import <file> [--as-planned]
```

Restores into a fresh `loom init`. Two-phase: validates before writing, never leaves a partial graph. `--as-planned` drops all groundings and proof results (porting mode: intents arrive planned, criteria travel as acceptance contracts).

```
loom detect
```

Heuristic repo scan: project type, language stack, recommended quality packs, greenfield vs brownfield signal.

---

## Intent commands

```
loom intent add --name "<name>" --description "<desc>" --level system|component|feature|cross_cutting
  [--lifecycle planned|implemented|needs_change]
  [--visibility user_visible|internal]
  [--aspect capability|happy|sad|fallback|edge_case|invariant]
  [--layer <layer>] [--domain <domain>]
  [--tag <term>] [--source <path>]
  [--allow-symbol-name]
```

**Atomization guard:** if the intent name matches a symbol pattern (snake_case with no spaces, no verb-phrase structure), the command is rejected unless both `--allow-symbol-name` and a non-empty `--description` containing a behavioral criterion are provided. Both flags required; either alone is not enough.

```text
rejected:   loom intent add --name "capture_payment"
rejected:   loom intent add --name "capture_payment" --allow-symbol-name
rejected:   loom intent add --name "capture_payment" --description "the capture_payment fn" --allow-symbol-name
accepted:   loom intent add --name "capture_payment"
              --description "payment is captured and inventory reserved before fulfillment"
              --allow-symbol-name
accepted:   loom intent add --name "payment can be captured" --description "..."
```

The override is recorded on the node. `loom doctor` audits all overrides and surfaces them as function-level granularity drift candidates.

```
loom intent update <intent> --description "<new>" --reason "<why>"
  [--reword]
```

Description change = redefinition. Ripples one hop: passing/independent edges → needs_reverification; Validation.last_result → not_run; old wording preserved in Note. `--reword` = same meaning, clearer words; no ripple, align clock resets.

```
loom intent mark <intent> --lifecycle planned|implemented|needs_change [--reason "<why>"]
```

```
loom intent confirm <intent> [--visibility user_visible|internal]
```

Ratifies meaning (resets align drift clock). `--visibility internal` removes the intent from align queue until meaning is redefined.

```
loom intent retire <intent> --reason "<why>" [--replaced-by <intent>]
```

Status → deprecated. Invisible to computation (queues, coverage, centrality, ripple). Visible to history. Reports triggered fallout: orphaned children, files losing only owner, dangling proofs.

```
loom intent show <intent>
loom intent list [--level <level>] [--lifecycle <lc>] [--limit N]
loom intent context <intent>
```

`context` returns the full intent neighborhood: hierarchy, requires/variants/triggers, codefiles, validations, rules, surfaces, notes, wiki pages, debt clusters.

---

## Edge commands

```
loom edge implement <intent> <codefile> --locator "<symbol>" [--confidence <n>]
loom edge unimplement <intent> <codefile>
```

```
loom edge remove <edge-id> [--reason "<why>"]
loom edge show <edge-id>
loom edge list [--kind <kind>] [--status <status>] [--limit N]
```

`edge remove` refuses derived edges. When removing an `implements` edge leaves its source intent with zero remaining groundings, the command still completes for script compatibility but prints a warning so the operator can immediately re-ground or intentionally accept the realized-rung drop.

```
loom edge hierarchy <parent> <child>
```

```
loom edge requires <intent> <intent>
loom edge scenario-of <scenario> <parent>
loom edge variant-of <variant> <base>
loom edge triggers <context-intent> <response-intent> --condition "<condition>"
loom edge sequence <before> <after>
```

```
loom edge verdict <edge-id> <ground|issue|independent>
  --criterion "<falsifiable claim>"
  --evidence "<what was found>"
  --confidence <0-1>

loom edge explore <intent-a> <intent-b> <ground|issue|independent>
  --criterion "<falsifiable claim>"
  --evidence "<what was found>"
  [--evidence-locator <file:lines>]
  --confidence <0-1>
```

Verdict commands for relationship inspection. `independent` requires non-empty evidence ("verified no relationship" — not the default).

---

## CodeFile commands

```
loom codefile add '<glob-or-path>'
loom codefile remove <path>
loom codefile show <path>
loom codefile list [--limit N]
```

`show` returns: owning intents + locators, governing rules, imports, symbols, stale-edge count, tangle flag (≥3 owners).

```
loom ignore add '<glob>' --reason "<why>"
loom ignore remove '<glob>'
loom ignore list
```

Coverage exclusions live in the graph with a recorded reason. `loom coverage` honors them.

---

## Validation commands

```
loom validation add --name "<name>" --type test|assertion|benchmark|manual_check|saga|scenario|contract
  --command "<cmd>"
  --intent <intent>
  [--proof-level L0|L1|L2|L3|L4|L5|L6]
  [--proof-kind journey]
  [--journey-id <id>]
  [--repo-native-kind <kind>]
  [--artifact <path-or-ref>]
```
```
loom validation mark <validation> --result passed|failed|blocked
  --evidence "<observed proof>"   (passed/failed)
  --reason "<blocker>"            (blocked)
```

Manual verdict for proofs without a runnable command.

```
loom validation update <validation> [--command "<cmd>"] [--description "<desc>"]
```

Changing `--command` resets `last_result → not_run` and linked `validates` edges → needs_reverification (it proved a different command).

```
loom validation delete <validation>
loom validation list [--intent <intent>] [--limit N]
loom validate <intent> | --all
```

`--all` runs every pending proof (last_result = not_run) in one call. Settled verdicts are not re-run; blocked proofs keep their recorded reason.

### Finding triage commands

```
loom finding list [--kind <kind>] [--state untriaged|stale|justified|needed|blocked]
loom finding verdict <id> justified|needed|blocked --reason "<why>"
```

`--state untriaged` lists findings with no adjudication. `--state stale` lists previously adjudicated findings whose flagged file hash changed; these are served by `loom next --mode triage` with "prior verdict is stale" in the work item. Status reports these as separate `untriaged` and `stale_findings` counts because first-triage and re-triage are different operator tasks.

### Saga commands (MVP — JSON saga + HTTP contract runner)

```
loom saga add <spec.json|http-contract.json>
loom saga list
```

`loom saga add` accepts either the native saga JSON shape (`saga`, `base`, `steps`) or a repo-agnostic HTTP contract JSON shape (`name`, optional `base`/`auth`, `routes`). It creates a `Validation(type=saga)` with JourneyProof metadata (`proof_level=L5`, `proof_kind=journey`, `journey_id`, `repo_native_kind`, `artifact`) and links `validates`/`sequence` edges for route or step intents it can resolve. HTTP contract routes without matching intents are reported as `unmatched_steps` under `--json`; normal saga specs fail on unresolved step intents.

```
loom saga run <spec.json|http-contract.json>
loom saga diagnose <spec.json|http-contract.json>
```

`loom saga run` executes the normalized HTTP journey with reqwest. HTTP contract routes support method/path, `query`, JSON `example_request`, response-field existence checks, and `extract` state threaded into later path/query/body templates. `BASE_URL` is used when an HTTP contract omits `base`; bearer auth contracts use `LOOM_SAGA_AUTH_TOKEN`. Passing/failing route evidence stamps linked `validates` edges. `loom saga diagnose` dry-runs without graph writes and prints runner hints.

---

## Quality commands

### Seeding packs

```
loom rule seed <pack>
```

Seeds a pre-authored rule pack. Available packs: `iso5055`, `service`, `data`, `concurrency`, `web-ui`, `mobile`, `docker`, `security`.

Pack rules ship with all guidance fields pre-authored: `inspection_guide`, `detection_hints`, `evidence_template`, `passing_example`, `failing_example`, and `detection_kind`. They are ready to use without further configuration.

### Adding custom rules

```
loom rule add --name "<name>" --description "<desc>" --severity error|warning
  [--category security|performance|defect|style|robustness]
  [--effort low|mid|high]
  [--detection-kind llm_judgment|pattern]
  [--pattern-kind regex|tree_sitter --pattern '<query>' [--pattern-scope file|symbol]
                                    [--pattern-hit-label '<description>']]
                                                      (repeatable; each group adds one entry to patterns[])
  [--inspection-guide "<step-by-step prose>"]
  [--detection-hint "<llm guidance prose>"]            (repeatable; LLM-facing only, not machine-run)
  [--evidence-template-passing "<phrasing>"]
  [--evidence-template-failing "<phrasing>"]
  [--passing-example-criterion "<text>" --passing-example-evidence "<text>" --passing-example-confidence <n>]
  [--failing-example-criterion "<text>" --failing-example-evidence "<text>" --failing-example-confidence <n>]
  [--applies-when '<json>']
```

`--pattern-kind`/`--pattern`/`--pattern-scope`/`--pattern-hit-label` are machine-executable. Sync runs them and attaches hits as `pre_screened_hits` in the quality WorkItem. `--detection-hint` is LLM-facing prose only — it guides inspection but sync never runs it.

`--detection-kind pattern` requires at least one `--pattern-kind/--pattern` group. Specifying `--detection-kind pattern` with no pattern flags is rejected at write time — no silent downgrade to `llm_judgment`.

Custom rules default to `detection_kind=llm_judgment`. Guidance fields are optional but strongly recommended — without `inspection_guide` and `evidence_template`, LLM verdicts drift across sessions.

### Updating rule guidance

```
loom rule update "<rule>"
  [--description "<new>"]
  [--severity error|warning]
  [--effort low|mid|high]
  [--detection-kind llm_judgment|pattern]
  [--pattern-kind regex|tree_sitter --pattern '<query>' [--pattern-scope file|symbol]
                                    [--pattern-hit-label '<description>']]
                                                      (repeatable; replaces existing patterns[])
  [--inspection-guide "<text>"]
  [--detection-hint "<llm guidance prose>"]            (repeatable; replaces existing hints)
  [--evidence-template-passing "<text>"]
  [--evidence-template-failing "<text>"]
  [--passing-example-criterion "<text>" --passing-example-evidence "<text>" --passing-example-confidence <n>]
  [--failing-example-criterion "<text>" --failing-example-evidence "<text>" --failing-example-confidence <n>]
```

Updating `inspection_guide` or `detection_kind` does not stale existing verdicts — the criterion/evidence already recorded stands. Use `loom rule verdict` to re-measure if needed. Changing `detection_kind` to `pattern` without providing at least one pattern group is rejected at write time.

### Recording verdicts

```
loom rule verdict "<rule>" "<intent>"
  --status passing|failing|independent
  --criterion "<what compliance means here>"
  --evidence "<what inspection found>"
  [--evidence-locator <file:lines>]
  --confidence <0-1>
```

A verdict at component altitude covers descendants unless a leaf needs a specific verdict. `independent` = measured, does not apply — requires evidence.

### Verdict flag matrix

| Command | Outcome selector | Evidence flags | Confidence |
|---|---|---|---|
| `loom edge verdict <edge-id> <ground|issue|independent>` | positional outcome | `--criterion`, `--evidence` | `--confidence` |
| `loom edge explore <a> <b> <ground|issue|independent>` | positional outcome | `--criterion`, `--evidence` | `--confidence` |
| `loom rule verdict <rule> <intent>` | `--status passing|failing|independent` | `--criterion`, `--evidence`, optional `--evidence-locator` | `--confidence` |
| `loom finding verdict <id> <justified|needed|blocked>` | positional verdict | `--reason` | not accepted |
| `loom validation mark <validation>` | `--result passed|failed|blocked` | `--evidence` for passed/failed, `--reason` for blocked | not accepted |

Do not transfer flags between families: finding and validation verdicts intentionally reject `--criterion`, `--evidence`, and `--confidence` forms that belong to edge/rule inspection.

### Inspecting rules

```
loom rule list [--limit N]
loom rule show "<rule>"
```

`show` includes all guidance fields — inspection guide, detection hints, evidence template, examples. This is the reference the LLM reads before a quality work item.
---

## Hypothesis commands

```
loom hypothesis add --name "<name>"
  --claim "<what is wrong now>"
  --proposal "<the change>"
  --predicted-outcome "<measurable result>"
  --target <intent>

loom hypothesis prove <hypothesis>
  --verdict supported|refuted
  --evidence "<what code showed>"

loom hypothesis adopt <hypothesis> --spawned <planned-intent>
loom hypothesis reject <hypothesis> --reason "<why>"

loom hypothesis list [--status proposed|supported|adopted|rejected] [--limit N]
```

Hypotheses are invisible to coverage and completeness until adopted. Speculation never counts as graph state.

---

## Inbox commands

```
loom inbox add "<raw text>" [--source human|llm|code_audit|wiki|validation|import|external]
  [--link <kind>:<ref>]

loom inbox normalize <id>
  --kind <inbox-kind>
  --claim "<normalized claim>"
  --route <route-kind>
  --command "<proposed graph command>"

loom inbox mark <id> --status routed|rejected|duplicate|deferred --reason "<why>"
loom inbox list [--status <status>] [--limit N]
loom inbox show <id>
```

The single free-form input boundary. Raw text enters as InboxItem first; normalization produces a typed proposed graph command; the operator runs that command separately then marks the card.

---

## TaskRecord commands

```
loom task add "<title>" --kind spike|investigation|experiment|review|chore
  [--source human|llm|queue|wiki|validation]
  [--target <ref>]

loom task start <task>
loom task close <task> --result "<summary>" --evidence "<refs>" [--promoted-to <ref>]
loom task abandon <task> --reason "<why>"
loom task list [--status active|completed|abandoned] [--limit N]
```

TaskRecords guide work but do not certify truth. Durable outcomes must be promoted to graph facts.

---

## Proposal commands

```
loom proposal add --title "<title>" (--file <path> | --text "<raw proposal>")
loom proposal list [--limit N]
loom proposal show <proposal>

loom proposal item add <proposal> --text "<item>" [--kind <kind>]
loom proposal item adopt <proposal> <number>
  [--as intent|task] [--name "<spawned name>"] [--description "<spawned description>"]
loom proposal item defer <proposal> <number> --reason "<why>"
loom proposal item reject <proposal> <number> --reason "<why>"
```

Proposals are durable plan/RFC artifacts. Use them when a human or LLM gives a structured plan that is too rich for an InboxItem but not yet graph truth. A proposal is one `proposal` node; numbered items live in `body.items`, not as graph nodes. Adoption is a one-way MVP transition from `open` to `adopted` and can optionally spawn a planned Intent or proposed TaskRecord with source proposal/item metadata.

Proposals do not certify behavior. Adopted items must still become ordinary Loom facts: intents get implemented and grounded, tasks get closed or abandoned, and validations prove behavior.

---

## InterfaceSurface commands

```
loom surface add --name "<name>" --kind http|cli|ui_route|message_topic|sdk_method|internal_module|storage
  --identity "<method+path, command, topic, symbol>"
  [--contract-ref <path>]
  [--intent <intent>]

loom surface show <surface>
loom surface list [--kind <kind>] [--limit N]
```

```
loom interface gaps
```

Surfaces without validations, boundary intents without surface bindings, and validation edges missing `calls`.

---

## Audit and integrity

```
loom coverage
```

Vertical spine: intent tree shape, leaf grounding, file ownership. Reports unaccounted files (after ignores) and missing groundings.

```
loom doctor
```

Schema conformance, provenance, evidence vacuity, role gate audit. Exits non-zero on any issue.

```
loom smells [--limit N] [--summary]
```

Structural signals: twin intents, overlapping ownership, tangle, undeclared coupling, layering violations, complex symbols, duplicated responsibility, unjourneyed surfaces, vocab drift, etc. Each finding carries an exact remedy command. Open findings gate the `excellent` maturity rung. Removing shared `implements` groundings from a tangled file can expose `duplicated_responsibility` smells for intent pairs that share vocabulary but no explicit relationship; that is expected fallout, not a failed fix. Resolve it by recording real `relates` edges or retagging/splitting intents whose shared vocabulary was misleading.

```
loom debt [--limit N]
```

Ranked statistical cluster feed: co-change, clone, shotgun surgery, recurrence, proof-locality. Advisory only. Never in `loom next`. Confirmation promotes to `Hypothesis`, `needs_change Intent`, manual edge, or decision `Note`.

```
loom impact <path-or-intent>
```

What would stale if this codefile or intent changed: edges, validations, wiki pages, interfaces.

```
loom hotspots [--limit N]
```

Most-central intents (blast radius) and most-tangled files.

```
loom dig <work-item-id>
```

Focused code-intelligence view for one WorkItem: suggested read set, prior evidence, stale causes, and target facts. Supplements the PromptContract when the LLM needs more file-level context before acting.

---

## Note commands

```
loom note add --text "<text>" [--kind decision|justification|question|idea|todo|commentary]
  [--intent <id>] [--edge <id>] [--file <path>]
  [--for <role>] [--author human|llm]

loom note list [--intent <id>] [--edge <id>] [--file <path>] [--kind <kind>] [--for <role>] [--limit N]
```

---

## Vocab and layer commands

```
loom vocab add "<term>" --why "<contrastive definition>"
loom vocab list
loom vocab merge <from> <to>
loom intent tag add <intent> <term>
loom intent tag remove <intent> <term>
```

```
loom layer order <top> <next> ... <bottom>
loom layer list
loom layer clear
```

Declares architecture layer order for layering violation detection. Intents with `--layer` labels pointing "up" in the declared order surface as smells.

---

## Wiki commands `[deferred — milestone 2]`

```
loom wiki plan [--json]
```

Build `WikiManifest`: page plan, nav tree, per-page `depends_on` refs from current graph.

```
loom wiki generate [--run-id <id>]
```

Render wiki pages with citations into an isolated preview run at `.loom/wiki-runs/<run-id>/`. Always writes to the preview location — never directly to `docs/loom/**`. `publish` is the only path to docs.

```
loom wiki verify [--run <run-id>] [--json]
```

Runs hard and soft checks against a preview run (see wiki-projection.md). Exits non-zero if any hard check fails.

```
loom wiki publish --run <run-id>
```

Copies a verified preview run to `docs/loom/**`. Requires all hard checks to pass. The only command that writes to docs.

```
loom wiki update [--run-id <id>]
```

Incremental path: identify stale pages from dependency tracking, regenerate only those pages into a new preview run, then verify and publish. Equivalent to `generate` + `verify` + `publish` scoped to stale pages.

---

## Federation `[deferred — milestone 2]`

```
loom init --name <graph-name>
loom delegate add '<glob>' --to <child-graph-path>
loom delegate list
```

Root-level `loom coverage` buckets delegated files as covered by the child. A missing child export is flagged, never silently trusted. Data flows up only — children export, parent observes.

---

## Output conventions

Every command:

- Renders human-readable text by default; `--json` for machine-readable
- Ends state-changing output with `next_step` + `graph_state` pulse
- Bounds lists to `--limit` (default 50) with explicit `+N more — <command>` markers
- Names the corrective command in every error message
- Ambiguous name fragments error with candidates — never a silent guess

`loom batch -` accepts newline-delimited JSON operations on stdin for bulk writes without per-item ceremony:

```json
{"op":"ground","a":"<intent>","b":"<intent>","criterion":"...","evidence":"...","confidence":0.9}
{"op":"rule_verdict","rule":"<rule>","intent":"<intent>","status":"passing","criterion":"...","evidence":"...","confidence":0.9}
```
