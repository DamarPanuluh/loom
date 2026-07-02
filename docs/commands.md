# loom v2 — Commands

Status: **shipped CLI surface** — this page follows the compiled `target/debug/loom --help` tree. Names listed under "Removed / deferred names" are intentionally not current commands.

---

## Orientation

```text
loom status [--json]
```

Graph identity, maturity ladder, queue counts, validation summary, code ownership, and the compass. `graph_state.low_confidence` is the count served by `loom next --mode review`; `graph_state.open_questions` is the count of unanswered question-sourced inbox items.

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
--all:  closeout view: every queue at once
```

Queue partition is deliberately disjoint:

- `fix`: every failing asserted edge — strictly root-cause repair. A fix packet never carries verdict authority: repair the source, run `loom sync`, and the owning lane re-measures.
- `analyze`: uninspected and stale non-`governs`/non-`validates` asserted claims. Stale claims are served first — a settled truth that broke misleads readers; an uninspected claim only waits.
- `quality`: uninspected or stale `governs` only. Failing `governs` routes to `fix`.
- `validate`: uninspected or stale `validates` only. Failing `validates` routes to `fix`.
- `coverage`: registered codefiles with no owning intent. If the file is missing from disk, the packet is a dedicated missing-file contract: re-ground any successors, then unregister the dead registration — do not attempt to read a ghost.
- `review`: asserted `passing` or `independent` verdicts with `0 < confidence < 0.7`, lowest confidence first. The work item keeps the edge kind's registry owner as `owner_role`, but the mindset is independent re-inspection.
- `elaborate`: the most-incomplete user-visible feature intent by Definition-of-Complete scorecard. The packet embeds the open axes and routes the builder to add missing scenarios/prerequisites/proofs/journey coverage, raise product questions, or waive non-question axes with reasons.

Fixer lane safety: fix the source and run `loom sync`; sync re-opens the claim (`needs_reverification` plus any `stale_cause` facet), and the owning lane re-measures it.

Quality fallback: if no `governs` edge needs work, `loom next --mode quality` proposes the first never-measured `(QualityRule × root implemented Intent)` pair. Recording the verdict creates the `governs` edge, so seeding a pack creates actionable work.

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
loom find [--limit N] "<query>" [--json]
```

Keyword-substring search over intents, codefiles, surfaces, rules, and validations. It is not BM25.

```text
loom door "<utterance>" [--json]
```

Capture-first entry for free-form human/LLM language. Creates an `InboxItem` and returns a landing menu: closest intents by keyword score, compass pulse, prefilled landing commands (`existing_intent`, `new_intent`, `hypothesis`, `spike`, `dismiss`), and the closing `loom inbox mark <id> routed` step. The `new_intent` landing includes `--visibility user_visible`, `--aspect happy`, and an `after` hint to run `loom next --mode elaborate` so the first idea grows its forgotten surroundings.

```text
loom guide [--role builder|analyzer|fixer|validator|quality|monitor] [--json]
```

Self-contained driving protocol. `--json` includes `truth_axes`; each axis includes `correct_when`, the falsifiable criterion for that form of truth. `--role` adds the lane's mindset, allowed/forbidden writes, evidence requirements, and the same truth-axis honesty line.

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
loom sync [--json]
```

Recomputes the structural plane from disk. Content-hash based — mtime churn never false-flags. Sync now stales Targets (`hypothesis -> intent`) edges, records `stale_cause` facets on every staled edge, deterministically resets validations, and downgrades never-reached previously-passing journey steps to `needs_reverification` when a journey run fails earlier.

```text
loom export [--check] [--json]
```

Writes deterministic `loom.graph.json`. `--check` exits non-zero if committed export drifts from the live graph. The export includes a portable `config` map for `layer_order`, `ignores`, `codefile_globs`, and `scan_adapters`, so import no longer silently loses layer/ignore/glob/adapter setup.

```text
loom import <file> [--json]
```

Restores an export into a fresh store. Import is validate-then-write and never leaves a partial graph.

```text
loom detect [--json]
```

Detects repo languages and recommends seedable quality packs only. Available packs are: `iso5055`, `service`, `web-ui`, `data`, `concurrency`, `docker` (29 rules total across the shipped pack set).

```text
loom scan add <name> "<command>" [--map <regex>] [--json]
loom scan list [--json]
loom scan update <name> [--command "<cmd>"] [--map <regex>] [--json]
loom scan remove <name> [--json]
loom scan run [<name>] [--json]
```

External diagnostic adapters can wrap any language's linter, type-checker, static analyzer, or bespoke script. `scan add` stores the adapter command in graph config; `scan list` shows registered adapters; `scan remove` deletes one; `scan run [<name>]` runs one adapter or all adapters and converts parsed diagnostics into derived `Finding` nodes for ordinary `triage`.

The default parse map is GCC-style `file:line[:col]: message`. The default parser also pairs a bare `file:line[:col]` location line with the message on the immediately following line (svelte-check-style two-line output; a blank line in between drops the pair). `--map` accepts a custom regex with named groups `file` and `line`, plus optional `msg` and `code`; a custom map is strictly per-line. Only diagnostics whose `file` resolves to a registered `CodeFile` become findings. Re-running an adapter converges: findings for diagnostics still present stay active, new diagnostics create findings, and findings whose diagnostics disappeared are resolved. Scan adapters travel with `loom export` in `config.scan_adapters`.

```text
loom completeness [<intent>] [--json]
```

Definition-of-Complete scorecard: per-intent axes met/open/waived. Omit the key for all feature intents. The axes are `scenarios`, `prerequisites`, `boundary`, `proof`, `journey`, and `questions`. `scenarios` is satisfied by a family of `scenario-of` intents with `--aspect happy|sad|fallback|edge_case`; `questions` is driven by linked question inbox items (`loom inbox add "..." --source question --link intent:<id>`) and closes when those questions are answered/routed/withdrawn, not by a waiver.

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
loom intent update <intent> --description "<new>" --reason "<why>" [--reword] [--name <new-name>] [--json]
```

Description change = redefinition. It ripples one hop: passing/independent edges become `needs_reverification`, linked validations reset, completeness waivers (`waiver:*` facets) are cleared so waived axes re-open, and old wording plus waiver reopening are preserved in decision notes. `--reword` is same meaning, clearer words; no ripple.

```text
loom intent set <intent> [--level <level>] [--visibility user_visible|internal] [--aspect happy|sad|fallback|edge_case] [--json]
loom intent mark <intent> --lifecycle <lifecycle> [--reason "<why>"] [--json]
loom intent confirm <intent> [--json]
loom intent retire <intent> --reason "<why>" [--replaced-by <intent>] [--json]
loom intent remove <intent> --reason "<why>" [--json]   (mistakes only; refuses intents that still have hierarchy children)
loom intent reactivate <intent> --reason "<why>" [--json]
loom intent waive <intent> scenarios|prerequisites|boundary|proof|journey --reason "<why>" [--json]
loom intent show <intent> [--json]
loom intent list [--limit N] [--json]
loom intent tag add <intent> <term> [--json]
loom intent tag remove <intent> <term> [--json]
```

`confirm` ratifies meaning. `retire` sets status to deprecated and removes the intent from active computation while preserving history. `tag` uses positional action `add|remove`. `waive` records a reasoned waiver for a non-question completeness axis (`scenarios`, `prerequisites`, `boundary`, `proof`, `journey`); if the intent is later redefined through `intent update --description`, waiver facets are cleared and those axes are scored again. Open questions must be answered/routed/withdrawn through the linked inbox item.

---

## Edge commands

```text
loom edge implement <intent> <codefile> [--locator "<symbol>"] [--json]
loom edge call <validation> <surface> [--json]
loom edge remove <edge-id> [--reason "<why>"] [--json]
loom edge set-locator <edge-id> <locator> [--json]
loom edge show <edge-id> [--json]
loom edge list [--limit N] [--json]
```

`edge remove` refuses derived edges. `edge call` records that a validation exercises an interface surface; sync resets that contract when the code behind the surface changes.

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

Verdict commands inspect relationship/grounding claims. `independent` means measured and not related/applicable; it requires real evidence.

---

## CodeFile and coverage exclusion commands

```text
loom codefile add <path-or-glob> [--json]
loom codefile rescan [--json]
loom codefile remove <path-or-key> [--json]
loom codefile show <path-or-key> [--json]
loom codefile list [--limit N] [--json]
```

`show` returns ownership, locators, imports/symbols/metrics, governing rules, findings, and stale-edge context.

```text
loom ignore add '<glob>' --reason "<why>" [--json]
loom ignore remove '<glob>' [--json]
loom ignore list [--json]
```

Coverage exclusions live in the graph with a recorded reason. `loom coverage` honors them.

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

Journey metadata flags require `--proof-kind journey`.

```text
loom validation mark <validation> --result passed|failed|blocked
  [--evidence "<observed proof>"]
  [--reason "<blocker>"]
  [--json]

loom validation update <validation> [--type <type>] [--command "<cmd>"] [--json]
loom validation unlink <validation> <intent> [--json]
loom validation delete <validation> [--json]
loom validation show <validation> [--json]
loom validation list [--limit N] [--json]
loom validate [<intent>] [--all] [--json]
```

`loom validate` runs stored commands without holding the graph lock while the command executes. Settled verdicts are not re-run unless made pending by sync or command changes.

### Finding triage commands

```text
loom finding list [--kind <kind>] [--state <state>] [--json]
loom finding verdict <id> <verdict> --reason "<why>" [--json]
```

Findings are derived structural signals. Verdicts are adjudications of those signals, not fixes.

---

## Journey commands

`journey` is the canonical family for flow/composition proofs. Hidden legacy alias: `loom saga` still parses to this family for old scripts; new docs, prompts, and write-backs use `loom journey`.

```text
loom journey add <spec.json|spec.yaml|http-contract.json> [--json]
loom journey list [--limit N] [--json]
loom journey run <spec.json|spec.yaml|http-contract.json> [--base-url <url>] [--json]
loom journey diagnose <spec.json|spec.yaml|http-contract.json> [--base-url <url>] [--json]
```

`add` creates a `Validation` whose body uses `type: "journey"`, `proof_level: "L5"`, `proof_kind: "journey"`, and command `loom journey run <artifact>`. It links resolved step intents with `validates` and adjacent steps with `sequence`. Normal journey specs fail on unresolved step intents; HTTP contract routes without matching intents are returned as `unmatched_steps`.

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

The preferred name key is `journey`; the older name key accepted by legacy specs is normalized only when `journey` is absent. HTTP contract specs use `name`, optional `base`/`auth`, and `routes`; route fields include `method`, `path`, optional `intent`/`name`, `success_status`, `query`, `example_request`, `response_fields`, and `extract`. Bearer auth injects `{{ env.LOOM_JOURNEY_AUTH_TOKEN }}`.

`run` records graph verdicts. A failing boundary records the exact failed expectation and reopens previously-passing never-reached later steps. `diagnose` executes directly without graph writes and is useful for missing env/auth/404/template failures. Both accept `--base-url`, which overrides the spec base and `{{ env.BASE_URL }}`.

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
loom journey coverage list [--limit N] [--json]
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
loom journey invariant list [--limit N] [--json]
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
loom rule ungovern <rule> <intent> [--json]
loom rule list [--limit N] [--json]
loom rule show <rule> [--json]
```

Custom-rule creation is intentionally small in the current binary. Rich guidance fields are provided by seeded packs and visible through `rule show`.

### Recording verdicts

```text
loom rule verdict <rule> <intent>
  --status passing|failing|independent
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
loom hypothesis prove <hypothesis> --verdict supported|refuted [--evidence "<what code showed>"] [--json]
loom hypothesis adopt <hypothesis> [--spawned <planned-intent>] [--json]
loom hypothesis reject <hypothesis> --reason "<why>" [--json]
loom hypothesis remove <hypothesis> [--json]
loom hypothesis show <hypothesis> [--json]
loom hypothesis list [--limit N] [--json]
```

Hypotheses are invisible to coverage and maturity until adopted. Speculation never counts as graph truth.

---

## Inbox commands

```text
loom inbox add "<raw text>" [--source <source>] [--link <ref>] [--json]
loom inbox list [--status new|routed|rejected|duplicate|deferred] [--limit N] [--json]
loom inbox show <key> [--json]
loom inbox mark <key> routed|rejected|duplicate|deferred [--reason "<why>"] [--json]
loom inbox remove <key> [--json]
```

The single free-form input boundary. Raw text enters as `InboxItem`; typed creation commands plus positional `mark` dispositions close the loop.

---

## TaskRecord commands

```text
loom task add "<title>" [--kind spike|investigation|experiment|review|chore] [--json]
loom task start <task> [--json]
loom task close <task> --result "<summary>" [--json]
loom task abandon <task> --reason "<why>" [--json]
loom task remove <task> [--json]
loom task show <task> [--json]
loom task list [--limit N] [--json]
```

TaskRecords guide work but do not certify truth. Durable outcomes must be promoted to graph facts.

---

## Proposal commands

```text
loom proposal add --title "<title>" (--file <path> | --text "<raw proposal>") [--json]
loom proposal list [--limit N] [--json]
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
loom surface delete <surface> [--json]
loom surface list [--limit N] [--json]
```

```text
loom interface gaps [--json]
```

Surfaces without validations, boundary intents without surface bindings, and validation edges missing `calls`.

---

## Audit and integrity

```text
loom coverage [--json]
loom completeness [<intent>] [--json]
loom scan add <name> "<command>" [--map <regex>] [--json]
loom scan list [--json]
loom scan remove <name> [--json]
loom scan run [<name>] [--json]
loom doctor [--json]
loom smells [--json]
loom debt [--json]
loom whoami [--json]
```

- `coverage`: vertical spine — intent tree shape, leaf grounding, file ownership, unaccounted files after ignores.
- `completeness`: Definition-of-Complete scorecard for one intent or all feature intents; non-question axes can be waived through `loom intent waive` and re-open on intent redefinition.
- `scan`: external diagnostic adapters; `run` turns registered-codefile diagnostics into derived findings for triage, and disappeared diagnostics resolve on the next run.
- `doctor`: schema conformance, provenance, evidence vacuity, role-gate audit; exits non-zero on any issue.
- `smells`: structural signals from graph shape, each with a remedy. Includes `pack_drift` when a seeded/builtin rule body differs from the shipped pack definition (remedy: `loom rule seed <pack>` to re-baseline, or keep the customization as its recorded trace).
- `debt`: advisory statistical cluster feed; never appears in required work queues until promoted.
- `whoami`: acting agent identity and lane enforcement.

---

## Note commands

```text
loom note add <target> --text "<text>" [--kind decision|context|warning] [--json]
loom note list [<target>] [--limit N] [--json]
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

## Removed / deferred names

These are **not** current shipped commands or flags. Do not emit them from prompts or examples unless explicitly discussing absence:

- removed/deferred from `next`: `--take`, `--compact`, `--slice`
- removed/deferred command families: batch writes, impact preview, hotspots, dig, wiki projection, delegate/federation
- removed/deferred subcommands: intent context, edge unimplement, vocab merge, inbox normalize
- removed/deferred flags: `guide --mode`, `import --as-planned`

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

List commands bound output with `--limit` where the binary exposes it. Ambiguous name fragments error with candidates instead of silently guessing.
