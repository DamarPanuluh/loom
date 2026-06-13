# loom — Full Command Reference

This is the exhaustive per-command reference, extracted from `CLAUDE.md` to keep the
handoff doc under the context-budget limit. It is the same material the CLI emits at
runtime — `loom guide`, `loom schema`, and `loom <command> --help` are the in-CLI
equivalents. See `CLAUDE.md` for the mental model, codebase structure, and design rationale.

All commands support `--json` for machine-readable output. LLM driving mode uses `--json` everywhere.

```
loom init [path] [--name <graph-name>] [--observed]
  Creates .loom/ directory, initializes Grafeo DB with full schema, and stamps
  the graph's IDENTITY (graph_id uuid + human name, default = dir name) — what
  other looms reference in a federation; it travels in the export.
  --observed = this graph maps code its drivers DON'T own (vendor SDK, another
  team's service): discovery/quality/validation all work, but build/fix lanes
  are disabled (custody gate) — findings, not fixes. Idempotent — re-running is
  safe and backfills identity on older graphs (also the way to set --name/
  --observed later).

loom status
  Graph stats: intent count, edge counts by inspection_status, open issues.
  `uninspected_outside_queues` names the uninspected edges NO queue serves
  (structural IMPLEMENTS, blocked proofs), so the raw histogram always
  reconciles with `unresolved_edges`. Coverage math is an identity:
  explored_pairs.total = covered + pending(uninspected/stale pairs) +
  unexplored_pairs (all over ACTIVE intents).
  `human_gated` (json; ⚑ line in human mode) is the oscillation summary —
  align drift suspects + supported hypotheses awaiting the adopt/reject
  ruling + blocked proofs: what needs the USER, so the agent drains the
  autonomous queues alone and batches these into one conversation agenda.

loom sync [path]
  THE PROGRAMMATIC FLAG ENGINE.
  Walks CodeFiles and detects CONTENT changes (content_hash; mtime is only the
  first-run fallback — checkout/rebase timestamp churn never false-flags),
  then propagates needs_reverification. Every flipped edge gets an append-only
  transition note naming the changed file ("passing → needs_reverification
  (sync: src/foo.rs changed)") — staleness explains itself in `loom edge show`
  and `loom next`. Output: N files changed, M edges flagged, K validations
  invalidated. LLM calls this after any code change, then calls loom next.

loom next --all
  THE CLOSEOUT VIEW: every role queue at once — counts + top item per queue
  (build/fix/ground/validate/quality/review/prove/adopt/align/discovery, in
  handoff order), vertical-completeness gaps, doctor health, and top smells,
  as ONE prioritized list. The single operational answer to "what's left?" —
  no reconciling five commands by hand. Discovery is flagged optional
  (horizontal axis). EVERY QUEUE CARRIES A GATE: `autonomous` (an agent
  drains it alone) or `human` (needs the user — align drift suspects, the
  adopt/reject ruling on a supported hypothesis). The gate makes the
  interactive↔autonomous oscillation plannable: drain autonomous queues now,
  BATCH human-gated items into ONE agenda for the next conversation window
  (`human_gated` total in json; ⚑ lines in human mode; `loom status` carries
  the same summary plus blocked proofs).

loom next [--mode discovery|fix|build|validate|quality|review|prove|align] [--take N] [--compact]
  One queue per agent role:
  discovery = inspect relationships (analyzer) · fix = resolve failing/stale
  RELATES_TO (fixer) · build = realize planned/needs_change intents (builder) ·
  validate = failing/unrun/missing proofs (validator) · quality = uninspected/
  failing GOVERNS edges PLUS never-measured rule×intent pairs (synthetic
  `unmeasured` items, surfaced at the highest unmeasured altitude only — one
  `loom rule verdict` resolves each, creating the edge with the verdict) ·
  review = verdicts recorded with confidence < 0.7, ranked by
  (1−confidence)×centrality — THE TIERED DOUBLE-CHECK: a low-capability scout
  records honest uncertainty and the graph itself routes exactly those claims
  to a stronger reviewer (independent re-inspection: form your own hypothesis
  BEFORE reading the recorded evidence; re-record to confirm ≥0.7 or overturn).
  Optional like discovery — review hardens closure, it never blocks complete.
  align = the validator's user↔intent drift queue: intents ranked by
  churn-since-confirm × centrality × staleness — code moved under a meaning the
  user never re-affirmed. Intents ruled visibility=internal are NEVER served
  (machinery isn't interview material) until a redefinition clears the ruling.
  The item is a CONCEPT-alignment move, not a wording check: it carries
  `visibility` (user_visible | internal | untriaged), `where_it_sits` (the
  parent chain — why it matters), and `not_to_confuse_with` (siblings +
  verified-independent neighbours), and the scaffold says to present what the
  product can DO, why it matters, and the audience UP FRONT — machinery
  presented as a product capability is how interviews go wrong. Vocabulary
  enters only when the user asks, stumbles, or collides with the graph. The
  description stays graph-speak source material — never read it aloud; on
  "evolved" translate the user's answer back into a falsifiable description.
  The user rules on BEHAVIOR, never on wording. Exactly one outcome lands:
  `loom intent confirm` (concept still right — resets the suspicion clock) /
  `confirm --visibility internal` (machinery — stop asking until redefined) /
  `update --reword` (words confusing, concept right — no ripple, clock resets) /
  `update` (concept evolved) / `retire --replaced-by` (superseded) /
  `add --lifecycle planned` (missing concept revealed).
  Optional like discovery — the graph can't read heads; this is the human gate.
  prove = the pre-decision plane's queue (analyzer, effort high), ranked by
  combined target-intent centrality (blast radius). Two item kinds, told apart
  by status: proposed = never proven (prove it) · supported with stale TARGETS
  = its support was earned against since-changed target code (re-prove or
  refute; re-proving re-stamps the edges). The work item carries the claim,
  targets, their groundings, and the prove command. Optional like
  discovery/review — speculation never blocks complete.
  --take N (discovery/fix/quality, capped 50) = the bulk READ half of the batch
  loop: N COMPACT items in ONE call instead of one rich item + anchor per call.
  discovery/fix group by the file that staled them (parsed from sync transition
  notes, indexed in one scan — never per-item) with prefilled `ground` template
  lines; quality groups by INTENT (one neighborhood read pays for every rule
  held against it) with prefilled `rule_verdict` lines and per-item effort from
  the rule's annotation. The token-bounded post-sync drain: read each hot
  neighborhood once, verdict its whole group via `loom batch`. Sync suggests
  `--take 20` when it flags >10 edges.
  --compact (discovery/fix) = the single-item PROJECTION: intent ids/names,
  edge id, top grounded paths, a ONE-LINE suggested command, owner_role/effort,
  and a `dig` pointer — no validations/notes/descriptions/pulse. For agents
  that already know the loop and only need the verdict coordinates ("intent
  alignment" runs live here: `loom next --compact --json`, verdict, repeat).
  EVERY work item carries `owner_role` AND `effort: low|mid|high` — effort
  names how much capability the WORK needs (computed from structure; quality
  items inherit the rule's inspection_effort). Loom never names models — the
  harness maps effort tiers to whatever models exist. The fix queue dispatches
  by item state: needs_reverification → analyzer/mid (re-inspection of an
  existing criterion), failing → fixer/high (repair).
  Returns single highest-priority work item with FULL context:
  - Edge (type, inspection_status, criterion, evidence, priority_score)
  - Both intent nodes (name, description, abstraction_level, source_refs)
  - Related CodeFiles (paths, last_modified)
  - VALIDATES edges on those intents (validation name, last_result)
  - Suggested action
  No second lookup needed. LLM can act immediately.

loom intent add --name --description --level [--domain] [--layer] [--source ...]
loom intent add ... [--aspect happy|sad|fallback|…] [--lifecycle planned|implemented|needs_change] [--tag <term> ...] [--visibility user_visible|internal]
  --domain = product/business facet (auth, billing) — discovery/scoring, NO
  layering effect. --layer = ARCHITECTURE layer (presentation, application,
  storage) — the input `loom layer order` ranks and `layering_violation` reads
  (schema v6 split these two; pre-v6 `--domain` armed layering).
loom intent confirm <id> [--visibility user_visible|internal]
  Ratify the meaning (status → confirmed) AND stamp a freshness note (kind=
  confirm, append-only — alignment history travels in the export). Re-confirming
  is the align loop's cheap outcome: it resets the drift-suspicion clock
  `loom next --mode align` ranks by. `--visibility internal` records the
  audience ruling atomically with the confirm — the "this is machinery, stop
  asking the user about it" interview outcome (out of the align queue until
  the meaning is redefined). Validator lane.
loom intent update <id> [--name "<new>"] [--description "<new>"] [--reword] --reason "<why>"
  EVOLUTION in place — same node, same id, full history — distinct from retire
  (supersession by a different intent). A --description change is a REDEFINITION
  and ripples ONE HOP, the semantic twin of `loom sync`: passing/independent
  RELATES_TO + GOVERNS → needs_reverification, passing IMPLEMENTS →
  needs_reverification ("does the code still do what this NOW says?"), passing
  TARGETS → needs_reverification, linked proofs → not_run (blocked keeps its
  reason). Every flip is noted with cause "intent '<name>' redefined"; the old
  wording is preserved in a decision note. --name alone is cosmetic (no ripple).
  A redefinition also CLEARS the visibility ruling (the new meaning's audience
  is unknown — the align interview re-triages it). --reword (requires
  --description) = same concept, clearer words: no ripple, visibility kept,
  but the align clock still resets ("terminology confusing, keep concept").
  Lifecycle is NOT auto-flipped — the staled IMPLEMENTS routes the honest
  question through the fix queue instead of faking a needs_change verdict.
  Builder lane.
loom intent mark <id> --lifecycle planned|implemented|needs_change [--reason "<why>"]
  Set the prescriptive lifecycle. needs_change = a known issue/refactor (honest,
  no faked verdict); --reason is recorded as a note. Feeds `loom next --mode build`.
loom intent delete <id>          (remove a mistake: node + its edges + notes)
loom intent retire <id> --reason "<why>" [--replaced-by <intent>]
  Design that was REAL and got superseded (delete is for mistakes). Status →
  deprecated; node/edges/notes stay as history, but the intent becomes
  INVISIBLE TO COMPUTATION: queues, coverage axes, centrality, the N×N grid,
  completeness, and sync ripple stop counting it. Reports the TRIGGERED WORK:
  orphaned children (re-parent or retire), files that lost their only owner
  (they surface as vertical gaps), proofs left dangling. The successor is
  recorded in a decision note — lineage stays traceable.
loom intent source add <id> <path>     (append to source_refs — docs AND code:
                                        contracts, ADRs, design notes; idempotent)
loom intent source remove <id> <path>
loom intent tag add <id> <term>        (tag from the registered vocabulary, max 3;
                                        an unknown term errors with the registry
                                        inlined — the menu at the decision point)
loom intent tag remove <id> <term>
loom intent list [--status] [--level] [--limit N]
loom intent show <id>            (intent + edges + hierarchy + implements + notes)

loom edge explore <a-id> <b-id>
  Prints both intents + source_refs. Creates edge if not exists.
  Subcommands:
    ground --criterion --confidence [--evidence "<found>"]
           [--evidence-locator path:lines]... [--inspected-by]
      evidence is optional on ground (the criterion may say it all) and ALWAYS
      replaces the previous verdict's evidence (a re-ground never leaves stale
      failure evidence behind the new green).
    issue --criterion --evidence [--evidence-locator path:lines]... [--inspected-by]
    --evidence-locator (repeatable) = file/line anchor(s), e.g.
      `src/db/queries/stats.rs:299-340`, folded into the stored evidence as
      `@<locator>` — a later review lands on the exact lines, not prose.
    independent --notes
    fix --description

loom edge list [--status] [--limit N]
loom edge show <edge-id>

loom cluster <intent-id>
  All unresolved edges touching this intent. For batching neighborhood work.

loom codefile add <path>          (or a glob: 'src/**/*.rs')
loom codefile list [--limit N]
loom codefile show <path-or-id>
  The per-file OWNERSHIP view: which intents claim it (level + locator +
  status), which quality rules reach it through them, its imports, and a
  tangled flag (≥3 intents). The answer hotspots only hint at.
loom codefile remove <path-or-id> (drop a phantom after delete/rename on disk;
                                   removes its IMPLEMENTS edges too)

loom validation add --name --type [--command] [--description] [--intent <id>]...
  --intent (repeatable) links the new Validation to intent(s) (one VALIDATES
  edge each) in one step; omit to link later with `loom edge validates`.
loom validation mark <id|name> --result passed|failed --evidence "<what you checked>"
loom validation mark <id|name> --result blocked --reason "<what it is waiting on>"
  Record a verdict BY HAND for a manual_check / async proof that has no runnable
  --command (which `loom validate` would otherwise skip). Validator-lane; evidence/
  reason must be substantive. Updates last_result + the per-intent VALIDATES verdict.
  `blocked` = honest "can't run yet" (live target down, missing credential): leaves
  the validator queue + compass, stays visible in `loom report`, survives sync
  (a code change doesn't unblock it). Re-mark passed/failed to unblock.
loom validation update <id|name> [--command "<cmd>"] [--description "<text>"]
  Fix a wrong definition (e.g. a bad cargo package in --command). A changed
  command RESETS the proof — last_result → not_run, VALIDATES edges →
  uninspected — because the old result proved a different command.
loom validation delete <id|name>
  Remove a mistake (the validation analogue of `intent delete`): node +
  VALIDATES edges + their notes. Intents that lose their only proof resurface
  in `loom next --mode validate`.
loom validation list [--intent <id>] [--limit N]

loom validate <intent-id> | --all
  Runs command on all VALIDATES edges for this intent. (manual_check without a
  command is skipped — use `loom validation mark` for those.)
  Updates Validation.last_result and VALIDATES edge inspection_status.
  --all = every PENDING proof in the graph (last_result == not_run: never run
  or sync-invalidated) in one verb — the drain after a sync flood resets N
  proofs at once. Settled verdicts (passed/failed) are not re-run; blocked
  proofs keep their recorded reason and stay out.

loom saga add <spec.yaml> [--spawn-missing [--under <parent>]]
loom saga run <name|spec.yaml>
loom saga list
  THE CONSUMER PLANE: an external-consumer proof — an ordered chain of endpoint
  invocations that consumes the system the way a real consumer will (values
  captured from one response thread into the next request). Runtime complement
  to read-evidence: RELATES_TO edges are normally grounded by READING code; a
  saga stamps the edges along its intent path with EXECUTION evidence.
  Engine is built in and pure Rust (reqwest/rustls + RFC 9535 JSONPath — no
  libcurl); deliberately a saga executor, NOT a general HTTP test tool
  (anything fancier = an ordinary command-based Validation).
  JOURNEY-FIRST (the bidirectional intent↔story entrance): with
  --spawn-missing, a step may name an intent that doesn't exist yet — it is
  spawned as a planned, user_visible FEATURE (the narrated journey IS the
  design; the build queue realizes it and the saga is its acceptance test).
  --under <parent> hierarchy-links the spawns (keeps the tree, no minted
  roots). Ambiguous bindings still fail — spawning on ambiguity would mint a
  twin; only ZERO-candidate bindings spawn. Builder lane; owned graphs only.
  Saga specs are trusted repo artifacts: `run` executes the declared HTTP
  calls, allows any `http(s)` target (including localhost), and follows
  reqwest's default redirect policy (up to 10 redirects). Guardrails are size
  ceilings, not sandboxing: response bodies are capped at 8 MiB and spec files
  are capped at 512 KiB before YAML parsing.
  Spec (YAML, the graph binding is first-class — every step names the intent
  it proves):
    saga: checkout-flow
    base: "{{ env.BASE_URL }}"        # {{ var }} / {{ env.X }} interpolation
    steps:
      - name: create cart
        intent: cart-creation          # id, exact name, or unique fragment
        request: { method: POST, url: /carts, json: { items: [] } }
        expect:  { status: 201, body: { "$.id": { exists: true } } }
        capture: { cart_id: "$.id" }   # JSONPath → var for later steps
      - name: capture payment
        intent: payment-capture
        request: { method: POST, url: "/carts/{{ cart_id }}/payment" }
        expect:  { status: 200, body: { "$.state": paid } }
  expect.body values: bare value = equals · {exists: bool} · {contains: "…"};
  expect.status omitted = any 2xx; expect.headers = substring match.
  `add` declares the proof: Validation node (type=saga, command =
  `loom saga run <spec>`) + VALIDATES edges to every step intent + the
  RELATES_TO path edges between consecutive step intents (uninspected — green
  is earned by running) + the spec registered as a CodeFile (it travels in the
  export, counts in coverage). Idempotent; re-add after editing reconciles.
  `run` executes (DB closed while HTTP runs, same lock discipline as
  `loom validate`) and translates outcomes into graph verdicts — the failure
  semantics: consecutive steps that BOTH passed → their RELATES_TO edge goes
  passing with runtime evidence; the boundary into the failing step → failing
  with the exact broken expectation ("expected 200, got 502"); steps after the
  failure are UNTOUCHED (never reached ≠ failing); the Validation + all its
  VALIDATES edges carry the run verdict. Existing non-empty edge criteria are
  preserved (execution refines the analyzer's contract, never overwrites it).
  Exits non-zero on failure, so the stored command also works under
  `loom validate` and in CI. Validator lane (`add` is builder|validator).
  Sync ripple already covers re-validation: code behind a step intent changes
  → its VALIDATES edges → not_run → the saga resurfaces in the validate queue.
  ENV VALUES: `{{ env.X }}` = passed AT INVOCATION (`BASE_URL=… loom saga run
  <name>`), never stored in the graph — they point at a LIVE target (start the
  system under test first). `saga add`/`list` report what's required (`run
  with: BASE_URL=<value> …`); a missing value REFUSES to run with the exact
  invocation in the error and nothing stamped — and `loom validate` records it
  as `blocked` (environment-not-ready), never as a failed proof.

loom persona add --name <name> --description "<who they are>" [--author <agent>]
  Register an audience segment — the "as a [X]" of user stories (the consumer
  plane). Builder lane.
loom persona list [--limit N]
loom persona show <persona-id>
  The persona with its SERVES edges (each with inspection status) and JOURNEYS
  (saga proofs bound to its path). Sub-sections cap at SECTION_CAP.
loom persona serve <persona-id> <intent-id> [ground|issue|independent …]
  The SERVES edge — "does this intent actually serve this persona?" Bare:
  creates the edge (uninspected) and prints context. With ground/issue/
  independent: records the verdict (analyzer lane; the same criterion/evidence/
  confidence gates as any inspectable edge). independent = it does NOT serve them.
loom persona journey <persona-id> <saga-id>
  Bind a saga (Validation of type=saga) to the persona — a structural JOURNEYS
  edge: "this end-to-end proof exercises this persona's path." No verdict; the
  saga's own run is the evidence.

loom rule add --name --description --severity [--effort low|mid|high]
  --effort = how much capability INSPECTING this rule needs (pack rules ship
  annotated: secrets-scan low, atomicity high, default mid). Travels into
  quality work items as `effort`.
loom rule list [--limit N]
loom rule apply <rule-id> <intent-id>   (positional; creates GOVERNS edge, uninspected)
loom rule check <intent-id>             (read-only: show GOVERNS edges by status)
loom rule verdict <rule-id> <intent-id> --status passing|failing|independent \
    --criterion "<what compliance looks like>" --evidence "<what was found>" \
    [--evidence-locator path:lines]... [--confidence 0.9] [--inspected-by llm:quality]
  THE quality write path — how GOVERNS green is earned. The verdict IS the
  measurement: if no GOVERNS edge exists yet, it is CREATED with the verdict
  (no separate `apply` needed — `apply` remains for pre-declaring "this rule
  applies" without a verdict). independent = measured, rule doesn't apply.
  Quality lane; criterion/evidence must be substantive.

loom report [--format json|text]
  Full coverage: edge counts by status across all types, intents without validations,
  failing GOVERNS, validation pass rate, recent passing edges.

loom batch [file|-]
  Bulk verdicts from JSON Lines (default stdin) — THE post-sync re-verification
  surface: a sync that stales 30 claims is one `loom batch` call, not 30
  invocations (pair with the bulk read: `loom next --mode fix --take 20`).
  The frictionless apply is a HEREDOC — no scratch file to place, no repo
  pollution, nothing to clean up (an agent once stalled deliberating where a
  /tmp jsonl could safely live):
    loom batch - <<'EOF'
    {"op":"ground","a":"…","b":"…","confidence":0.9}
    EOF
  A file path argument is for very large batches.
  Ops per line: ground / issue / independent (RELATES_TO) and
  rule_verdict (GOVERNS, creates the edge if absent). ground also takes an
  optional "evidence"; ground/issue/rule_verdict take an optional
  "evidence_locator" (string or array of `path:lines` anchors). EVERY gate applies per
  line — lanes, substantive criterion/evidence/notes, confidence — and each
  edge still gets its transition note. Continues past failed lines, reports
  per-line results, exits non-zero if any failed. Bulk changes the ceremony,
  never the honesty.

loom hypothesis add --name <n> --claim <c> --proposal <p> --predicted-outcome <o> \
    [--target <intent>]... [--author <agent>]
  Propose an improvement (status=proposed) — THE PRE-DECISION PLANE, the
  structured upgrade of `note --kind idea`. Any lane proposes; evidence gates
  reject vacuous claim/proposal/outcome. --target creates TARGETS edges.
loom hypothesis target <hypothesis> <intent>   (link another affected intent)
loom hypothesis prove <id> --verdict supported|refuted --evidence "<found>" \
    [--inspected-by llm:analyzer]
  The proof step: did the claimed problem turn out to be real in the code as it
  is NOW? Analyzer lane; the prover must differ from the proposer (when both
  declare roles — solo mode passes, as everywhere). The verdict also stamps
  every TARGETS edge (supported→passing, refuted→independent) — which is also
  how stale support clears after a re-prove. Decided (adopted/confirmed/
  rejected) hypotheses cannot be re-proven.
loom hypothesis adopt <id> [--spawned <intent>]... [--reason "<how it converts>"]
  THE CONVERSION POINT (builder lane, owned custody, requires status=supported):
  link the planned intents spawned from it — lineage decision notes both ways,
  and predicted_outcome becomes a not_run manual_check Validation (its
  description carries a `hypothesis:<id>` line) VALIDATES-linked to each
  spawned intent: the acceptance contract enters the proof plane. Requires
  --spawned or --reason; from here `loom next --mode build` owns the work.
  When `loom validation mark <outcome-validation> --result passed` lands, the
  hypothesis derives `confirmed` — the improvement provably delivered.
loom hypothesis reject <id> --reason "<why>"   (any state except adopted/confirmed)
loom hypothesis list [--status proposed|supported|refuted|adopted|confirmed|rejected] [--limit N]
loom hypothesis show <id>                      (fields + TARGETS + notes)

loom note add --text <text> [--kind <kind>] [--intent <id> | --edge <id> | --file <path|id>] [--author human|llm] [--for <role>]
  Append free-text memory. kind: justification | commentary | idea | question | decision | todo
  (transition + confirm are auto-recorded by loom: verdict history and
  `loom intent confirm` freshness stamps — listable, never written by hand).
  Attach to an intent, an edge, or a code file (id or registered path), or leave
  free-floating. Append-only (never overwritten). A kind=decision note is the
  adjudication record smells honor (scatter/tangle/happy-path/recurrence).
  --for builder|analyzer|fixer|validator|quality ADDRESSES the note to a lane —
  the directed-handoff channel: an out-of-lane finding becomes a message the
  owning lane sees FIRST (`loom next` sorts addressed notes to the top of the
  item's notes). Notes surface in `loom next`, `loom intent show`, `loom edge show`.
loom note prune
  Remove notes whose target no longer exists (deleted intent/hypothesis/edge)
  — the remedy `loom doctor` names for dangling note targets. Only
  unreachable notes are removed; history on live or retired nodes is never
  touched. (The hard-delete commands now prune their edges' notes themselves;
  this cleans up damage from older versions.)
loom note list [--intent <id>] [--edge <id>] [--file <path|id>] [--kind <kind>] [--for <role>] [--limit N]
  --for <role> = the lane's inbox (only notes addressed to it). --limit keeps
  the NEWEST rows (append-only memory; the tail is the live context).

loom vocab add <term> --why "<contrastive definition>"
  Register a tag term (builder lane). The --why must be CONTRASTIVE: what it
  covers AND what it does not, naming the neighbouring term ("authz —
  permission checks, NOT login/session (that's authn)"). A term that reads
  like an existing one (same stem / containment / tiny edit distance) is
  REJECTED at the door — synonym terms split the keyspace and intents stop
  colliding. Keep the registry small (warn past ~75): its value is that an
  agent can hold the whole menu in context at the moment of choice.
loom vocab list
  The registry: every term with usage count + definition — the menu agents
  pick from when tagging.
loom vocab suggest [--limit N]
  Candidate terms mined from THIS graph's OWN intents — tokens shared across ≥2
  intents and not yet registered, ranked by collision potential (generic words
  and over-ubiquitous tokens filtered out). The low-friction way to ARM
  duplicate-responsibility detection on an untagged graph: loom can't know your
  codebase's vocabulary, so it surfaces what already recurs in it. Read-only;
  surfaces the armed/unarmed coverage (`X of Y coded intents tagged`) and names
  the register→tag→re-smell next step. loom proposes the KEY; the contrastive
  `--why` stays your judgment. The `duplicate_detection_unarmed` smell points
  here.
loom vocab merge <from> <to>
  Converge drift: every intent carrying <from> is retagged to <to> (deduped),
  <from> is deleted. One sweep, nothing to re-inspect — terms are keys, not
  inspectable claims. The `vocab_drift` smell emits this command.

loom layer order <top> … <bottom>
  Declare the architecture's LAYER order, top layer first (builder lane;
  REPLACES any previous order — one atomic list on the LoomMeta sentinel,
  travels in exports and ports). This is the normative input the
  `layering_violation` smell judges imports against: an intent in a layer
  earlier in the order may depend on later ones, never the reverse (a recorded
  RELATES_TO does not excuse direction). The smell reads each intent's `layer`
  field — set with `loom intent add --layer …`. Layers not in the order, and
  intents with no `--layer`, are exempt — declare only what you mean to enforce
  (the same positive-evidence-only stance as tags). NOTE (schema v6): this is
  about ARCHITECTURE layers, split out from product `--domain` — `--domain`
  (auth, billing) is a business-facet label with NO layering effect.
loom layer list
  The declared order with per-layer intent counts, plus layers in use that the
  order does not cover (exempt from the smell).
loom layer clear
  Remove the order — layering_violation goes silent.
loom domain order|list|clear
  DEPRECATED alias of `loom layer` (one compatibility window). Old invocations
  still declare the layer order, but product `--domain` labels no longer arm
  layering. Prefer `loom layer`.

loom doctor
  Verify graph integrity against the declared schema (src/db/schema.rs):
  schema version, required-property presence, valid field values, dangling
  references, and the evidence audit behind every verdict — vacuous criterion,
  confidence outside [0,1], confidence still 0.0 behind passing/failing, empty
  last_inspected behind a verdict, out-of-lane provenance. Also emits advisory
  HINTS (never fail the exit code): all-solo provenance (declare roles for real
  separation of duties), and a stale committed loom.graph.json.
  Exits non-zero if any issue is found. Run after upgrades or if results look wrong.
  A version mismatch points at `loom migrate`.

loom migrate
  Upgrade a LIVE graph to the current schema version IN PLACE — a version
  CHAIN, each step idempotent, the meta version stamped LAST (crash-safe by
  re-run, not by transaction: bulk read-modify loops inside one transaction
  go quadratic on grafeo 0.5.x — see commands/migrate.rs).
  v3 → v4: edge identity became DERIVED (`<prefix>:<from>:<to>`, e.g.
  `rt:<intent-a>:<intent-b>`) instead of a stored uuid — every note that
  referenced a stored edge uuid is remapped (legacy id props on old edges are
  inert and left alone). v3/v4 → v5: source_refs/tags/imports convert from
  JSON-encoded strings to NATIVE LISTS. Also backfills the property indexes.
  Idempotent: a current graph reports "nothing to do". Re-export after
  migrating. Repos with only a committed loom.graph.json don't need this —
  `loom import` upgrades v3/v4 exports in flight.

loom guide [--mode greenfield|brownfield|refactor|port|seed]
  Self-contained driving protocol for an LLM new to loom: mental model, the loop,
  the done-condition, and a MODE-SPECIFIC population checklist (auto-detected via
  `loom detect` if --mode omitted): greenfield = design-as-planned-intents then
  build; brownfield = map & verify existing; refactor = flag needs_change & change;
  port = adopt a source graph's design (`import --as-planned`) and re-realize
  it in a new language/repo; seed = the USER interview (explicit-only, never
  auto-detected — the binary can't detect "the user wants to talk"): elicit a
  head into planned intents (altitude-calibrated, one question per landing,
  terminate on enumerable gaps, not exhaustion) or re-align a populated graph
  via `loom next --mode align`. An empty graph's compass routes phase=seed here.

loom schema
  The data model — node/edge types + properties, the inspection state machine,
  and the valid value vocabularies. Generated from the schema vocabulary (drift-proof).

loom find <query> [--limit N]
  ASK THE MAP — codebase intelligence entry point: BM25 keyword search over
  active intent names + descriptions (+domain), ranked. Each hit carries its
  hierarchy chain, IMPLEMENTS groundings with locators, and a stale-edge count
  (the freshness warning: claims about since-changed code). Scoring runs in
  Rust, NOT grafeo's text index — `CALL grafeo.search.text` returns internal
  node ids that can't be joined back to properties through GQL (probed; the
  trailing MATCH parses and is silently dropped). Deterministic; no fuzzy/
  stemming by design — the calling LLM reformulates. A miss distinguishes
  "not mapped" (points at `loom coverage`) from "doesn't exist".

loom door "<utterance>" [--limit N]
  THE ENTRANCE — progressive disclosure at turn zero: route a USER UTTERANCE
  to its landing. Loom never interprets (pure computation in the tool,
  judgment in the LLM): it assembles the routing context mechanically — what
  every plane already knows about the topic (intents by BM25 = `loom find`,
  vocab/sagas/rules by token overlap), the compass pulse, and the LANDING
  MENU: the total enumeration of ways an utterance becomes a graph noun, each
  an existing command (new behavior → intent add planned · story → saga
  [--spawn-missing] · complaint → mark needs_change · redesign → hypothesis
  add · norm → rule add · term → vocab add · meaning shift → intent update ·
  ruling/declined scope → decision note · question → answer from matches,
  nothing lands · "go work" → status/next). Two contracts keep the corridor
  clear: TOTALITY (no good landing = a menu bug, not a user problem) and THE
  DOOR ADVISES, NEVER BLOCKS (state lives in the graph, not the conversation —
  any noun lands at any time; queues re-derive, the compass re-sorts; there is
  no wrong moment to say anything). Discipline: ONE landing per utterance,
  landed before the next question; before going autonomous, sweep — every
  conversational fragment must have landed (conversation residue is the
  failure mode).

loom session
  TURN ZERO, BEFORE ANY UTTERANCE — the door's complement: the user said
  "use loom" / "loom session" / "loom mode" and stopped. Loom cannot read
  minds (pure computation in the tool, judgment in the LLM): it computes the
  OFFER MENU — every way this session could be spent, each offer backed by a
  live queue and its count — and marks exactly ONE recommended. The LLM asks
  ONE question ("what do you want from this session?"), in the user's
  language, recommendation first — an offer, never a quiz. The recommendation
  order encodes the scarcity of the user's PRESENCE: user-gated queues first
  (align drift > hypothesis rulings > blocked proofs — the agent cannot drain
  these alone), then the build backlog, then (phase=complete) saga
  enrichment, else autonomous handoff ("or should I just get to work?").
  Free-form answers route through `loom door "<their words>"`; "you decide"
  = take the recommended offer and go. Works before `loom init` (offers:
  restore the committed export > map the code > interview) and on an empty
  graph (interview vs map, picked by source on disk). Synonym verbs
  (`loom start|begin|hello|mode|talk|chat|interview`) teach this command.

loom hotspots [--limit N]
  Structural importance (graph centrality, NOT runtime profiling): most-central
  intents (blast radius) and most-tangled files (most intents in one file).

loom smells [--limit N]
  Derived problem signals — the graph as instrument, not ledger. Computed from
  structure alone (no LLM judgment in the flagging): twin intents (split-brain:
  same level, similar wording, no edge), overlapping ownership (two intents
  claim the same file, no edge), scattered intents (level-aware thresholds;
  the evidence GROUPS the grounded files BY DIRECTORY — the mechanical
  clustering for a decompose: loom shows where the files cluster, the LLM
  names the children; a kind=decision note on the intent NEWER than its newest
  grounding records "the spread is deliberate" and resolves the finding, a
  later grounding re-flags it), tangled files (≥3 intents — per-file detail via
  `loom codefile show`; a kind=decision note on the FILE — `loom note add
  --file <path> --kind decision` — newer than its newest claim resolves it,
  a later claim re-flags), undeclared coupling (file A imports file B but
  their intents have no edge — physical evidence vs semantic graph), recurrent
  trouble (a target whose transition history keeps returning to failing/
  needs_change — redesign, don't re-patch; a kind=decision note NEWER than the
  last regression resolves the finding without erasing history, and a later
  regression re-flags it), unmeasured intents (a QualityRule
  was never held against a coded intent — HIERARCHY-AWARE: a verdict on a
  component covers its descendants, so measure at the highest honest altitude
  instead of grinding per-leaf busywork; a leaf can still get its own, more
  specific verdict), unused rules, happy-path-only groups (children declare an
  `--aspect happy` but no sad/fallback sibling — failure behavior undeclared;
  a kind=decision note on the parent newer than its newest aspect-tagged child
  records "N/A here" and resolves it, a new aspect child re-flags),
  duplicated responsibility (two same-level intents whose REGISTERED tags
  collide rarity-weighted, grounded in DISJOINT files with no import between
  them — the case every physical detector misses: same responsibility
  implemented twice in unrelated code; untagged intents never fire it, so
  `loom smells` also discloses the blind spot — how many coded intents carry
  no registered tag and are therefore invisible to this detector),
  layering violation (code owned by a LOWER layer imports code owned by a
  HIGHER layer per the declared `loom layer order` — direction always
  existed in the physical plane; the declared order is what makes it
  judgeable. A recorded RELATES_TO edge does NOT excuse direction —
  undeclared coupling asks "is the contact declared?", this asks "does it
  point the right way?"; undeclared layers are exempt; a kind=decision note
  on the importing intent newer than its newest grounding records "this
  layer may reach up" and resolves it, a new grounding re-flags),
  vocab drift (two registered terms that read like the same word — remedy is
  the exact `loom vocab merge`), and unjourneyed surface (the consumer
  plane's completeness check: a user_visible intent with real code that NO
  saga exercises end-to-end — what makes the visibility ruling load-bearing
  outside the align interview. Two regimes so a journey-less repo isn't
  flooded: zero sagas → ONE aggregate finding on the root, adjudicated by a
  decision note there, re-opened by a new user_visible intent; ≥1 saga →
  per-intent findings with tree-aware coverage — a step bound at component
  altitude covers the features the journey runs through, a journeyed leaf
  covers its ancestors, unjourneyed SIBLINGS still fire; a decision note on
  the intent resolves, a redefinition re-opens).
  OPEN FINDINGS GATE GREEN: once every queue is dry, `graph_state` routes
  phase=audit until `loom smells` returns zero OPEN — green means every
  suspicion was ANSWERED (structurally fixed, or refuted via its adjudication
  path above), never that the heuristics went quiet on their own.
  ADJUDICATIONS STAY VISIBLE: a finding suppressed by a decision note is not
  gone — `loom smells` prints it under `adjudicated` with the ruling (who,
  when, why) and the exact structural change that re-opens it. "No findings"
  and "N findings ruled deliberate" never look alike (dogfood lesson: five
  godfile rulings batch-stamped in one second were invisible in every output).
  Disagreeing with a ruling is overruled through the work, not the ledger:
  `loom hypothesis add … --target <intent>` routes the redesign through prove.
  Each finding carries the exact remedy command — and the redesign-shaped ones
  (recurrent trouble, tangled files, twin merges, code-level scatter) emit
  `loom hypothesis add` so a redesign gets PROVEN before it becomes work,
  instead of dying in a note. The same suspicion signals
  (import links, shared files, description overlap, shared tags
  rarity-weighted, same domain) rank
  unexplored pairs in `loom next` discovery, with the why in the work item's
  notes. `loom rule verdict --status independent` records "measured — rule
  does not apply" so unmeasured findings resolve honestly.

loom rule seed iso5055|mobile|web-ui|service|data|concurrency
  Seed a built-in measuring-stick pack — the repo-kind VANTAGE POINTS for 360°
  normative coverage, each rule written for LLM inspection (detection_logic
  says exactly what to look for). Idempotent (existing names skipped).
  iso5055 = baseline, applies to any code (10 CWE-grounded rules across
  Reliability/Security/Performance/Maintainability) · mobile = lifecycle,
  offline, permissions, main thread, battery, platform divergence, deep links ·
  web-ui = view states, a11y, XSS, client-side trust, feedback, responsive,
  URL state · service = contract artifacts, idempotency, timeouts/retries,
  saga compensation, boundary auth, observability, degradation, compatible
  evolution · data = migrations, ingest validation, loss accounting, PII,
  rerun idempotency, lineage · concurrency = sync discipline, no lock across
  I/O/await, atomic multi-step, deadlock ordering, cancellation safety,
  bounded concurrency, plus perf-budget-proven (hot-path intents must state a
  budget in their criterion AND carry a passing benchmark validation — the
  normative plane demanding proof in the validation plane).
  `loom detect` recommends which packs fit this repo. After seeding,
  `loom next --mode quality` serves every never-measured rule×intent pair.

loom export [path]                    (default loom.graph.json; "-" = stdout;
                                       positional, mirroring `loom import <file>`;
                                       --out <path> still accepted)
loom export --check
  THE COMMIT GUARD: verify the existing export matches the live graph
  byte-for-byte (determinism makes freshness a byte comparison). Exits
  non-zero on drift or a missing file — hook it into pre-commit/CI so a graph
  change can never silently ship without its travel format.
loom import <file>
  The graph's travel format: deterministic JSON (same graph → identical bytes)
  meant to be committed so the graph travels with the repo and graph changes
  are diffable in PRs. Import restores into a fresh `loom init` (never merges);
  run `loom sync` after to reconcile with the machine's files. TWO-PHASE: every
  node and edge is validated before anything is written — a corrupted/hostile
  export is rejected loudly (field-naming error) and leaves NO partial graph.
loom import <file> --as-planned
  PORTING: the semantic plane travels, the physical plane is rebuilt. Intents/
  hierarchy/criteria/rules/notes are adopted; CodeFiles, IMPLEMENTS groundings,
  verdict meta, and proof results are dropped (they were earned against the OLD
  code). Every intent arrives lifecycle=planned, every proof not_run with its
  command kept as the spec to re-express; the target keeps its own graph
  identity. `loom guide --mode port` teaches the re-realization loop; the
  criteria written for the old code are the acceptance contract for the new.

Every verdict transition (ground/issue/independent/fix/rule verdict/lifecycle
mark) is auto-recorded as an append-only note (kind=transition) — the graph's
recurrence memory, read by the recurrent_trouble smell.

loom coverage
  Reconcile files on disk (respecting .gitignore) against the graph. Buckets each
  file: grounded (≥1 IMPLEMENTS) / delegated (owned by a child graph — federation) /
  excluded (matches an ignore pattern) / registered-but-ungrounded (unexplained
  code) / unaccounted (gap). Ensures nothing is silently missed. Done = no
  unaccounted (missing delegation targets are flagged).

loom detect
  Programmable repo introspection: stack (from manifests), source presence, top
  languages, suggested mode (greenfield vs brownfield), and RECOMMENDED QUALITY
  PACKS for this repo kind (each with its disk evidence) — the binary suggests
  the 360° vantage points so the agent doesn't have to remember them. Runs even
  before `loom init`.

loom ignore add <glob> --reason <why> [--author human|llm]
  The coverage escape hatch, stored IN the graph (not a .loomignore file) as a
  recorded, doctor-checkable decision. `.gitignore` is honored separately.
loom ignore list

loom delegate add <glob> --to <child-export-path>
loom delegate list
  FEDERATION (monorepo): a subtree owned by ANOTHER loom graph. `loom coverage`
  buckets matching files as `delegated` — covered by the child, verified against
  its committed export (a missing target is reported, never silently trusted).
  The root grounds seam intents in the children's exports; content-hash sync
  then ripples cross-service automatically. Data flows UP only (children
  export, parent observes) — a parent never writes into a child's graph.

(Discoverability extras: bare `loom` prints an orientation; `loom intent add` takes
 `--aspect happy|sad|fallback|…`; `loom edge implement … --locator "fn run"` grounds
 to a symbol; `loom codefile add 'src/**/*.rs'` bulk-registers via glob. `loom status`
 ends with a phase-aware "→ Next" compass, and status/next carry a `graph_state` pulse.
 Intents and rules are addressable by id, exact name, or unique name fragment.
 Edge ids are DERIVED from the endpoints — `rt:<from>:<to>` (hy/imp/gov/val/tgt
 for the other types), never stored — stable across export/import; `loom edge
 show` takes them and notes reference them.
 `loom edge implement <intent> 'src/db/**'` bulk-grounds over REGISTERED paths;
 `loom edge unimplement <intent> <path|glob>` is the ungrounding half — used to
 move groundings down to children when decomposing a scattered intent.)
```

GRAPH TARGETING: every command resolves its graph via `--graph <path>` >
`$LOOM_GRAPH` > cwd (one shared `resolve_root()` in db/mod.rs). Pin a session
with `export LOOM_GRAPH=<repo>` and every loom call hits that graph no matter
what `cd` does — kills the cd-fallback incident class (a failed cd + a
mutating script silently hitting whatever graph cwd landed in). Interactive
driving keeps the zero-ceremony cwd default; an unpinned command in a bare
directory still errors rather than guessing.
