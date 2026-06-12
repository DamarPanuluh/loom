//! `loom guide` — self-contained onboarding for an LLM new to loom, with a
//! mode-specific playbook (greenfield / brownfield / refactor). The mode is
//! auto-detected from the repo (via `loom detect`) unless given with `--mode`.

use anyhow::Result;

use crate::output::Printer;

const GOLDEN_RULES: &[&str] = &[
    "Drive via `loom next` — it prioritises and tells you the exact next command.",
    "After ANY code change: `loom sync`. It is the flag engine — see THE RIPPLE below. When a sync stales MANY claims at once, drain in bulk: `loom next --mode fix --take 20` / `--mode quality --take 20` hand back compact groups (fix groups by staling file, quality by intent — read each hot neighborhood ONCE) with a prefilled template, then `loom batch -` applies one JSONL verdict per line, and `loom validate --all` re-runs every invalidated proof in one verb (same gates as the single commands — bulk changes the ceremony, never the honesty).",
    "Per edge, work the Socratic loop: read both intents → form a hypothesis (\"I expect the code to show X\") → inspect the actual code → confirmed = ground it, code wrong = record the issue, hypothesis wrong = revise and re-inspect. Never record a verdict you didn't check.",
    "Batch by neighborhood: when you inspect an edge, `loom cluster <intent-id>` lists every other unresolved edge touching it — work those while the context is loaded.",
    "ASK THE MAP: `loom find \"<what you're looking for>\"` — keyword search over intent names/descriptions when you don't know the intent's name yet. Hits carry hierarchy position, code groundings with locators, and a staleness warning (claims about since-changed code). No fuzzy matching — a miss means reformulate in the map's vocabulary, or the area isn't mapped (`loom coverage`).",
    "Use `--json` on every command for machine-readable output (incl. a `graph_state` pulse).",
    "Every command has `--help`. `loom schema` = data model; `loom status` = where you are; `loom doctor` = integrity.",
    "Prescriptive intents (planned/needs_change) still need a falsifiable criterion — that's what makes the design a test.",
    "Two axes of completeness. VERTICAL is the binding spine: HIERARCHY is a tree (one parent per intent), every implemented leaf intent is grounded in code (IMPLEMENTS), every CodeFile is reached. HORIZONTAL (RELATES_TO, the N×N grid) is optional understanding/cleanup.",
    "Done (vertical) when `loom status` shows vertical ✓ + `loom coverage` reports nothing unaccounted. Horizontal closure (phase=complete) is optional polish.",
    "360° COVERAGE: the pulse footer counts every vantage point — grounded (files explained) · realized (leaves coded) · explored (the grid) · measured (rules held against coded intents) · proven (passed validations) — and the compass routes to the weakest axis. `measured` never closes by itself: seed the packs `loom detect` recommends (`loom rule seed iso5055|mobile|web-ui|service|data|concurrency`), then `loom next --mode quality` serves every never-measured rule×intent pair. ONE command resolves each — `loom rule verdict` creates the edge with the verdict; a verdict at component altitude covers descendants; `independent` = measured, doesn't apply.",
    "Criteria and evidence must be substantive — loom rejects placeholders, and `loom doctor` audits verdicts (vacuous criterion, bad confidence, missing timestamp, out-of-lane provenance).",
    "CLOSE OUT with `loom next --all` — every role queue, vertical gaps, and doctor health as ONE prioritized list (the answer to \"what's left?\" without reconciling five commands by hand).",
    "FEDERATION (monorepo / cross-service): every graph has an identity (`loom init --name`, in the export). A root graph DELEGATES service subtrees (`loom delegate add 'services/x/**' --to services/x/loom.graph.json`) and grounds seam intents in the children's committed exports — `loom sync` then ripples cross-service for free. Data flows UP (children export, parent observes); never write into a child's graph — emit findings and let the child's own agent record them in its lane. Map code you don't own with `loom init --observed`: understanding/measuring/proving work, build/fix lanes are off (findings, not fixes).",
    "A proof that CANNOT run yet (live target down, missing credential) is `loom validation mark <id> --result blocked --reason \"…\"` — honest and out of the queue; never leave it looking forgotten as not_run.",
    "TIERED DRIVING: every work item carries `effort: low|mid|high` — a statement about the WORK (loom never names models; the harness maps tiers). Low-capability agents drive the bulk and record HONEST confidence: 0.5-and-true beats 0.9-and-guessed, because verdicts below 0.7 feed `loom next --mode review` — the strategic double-check, ranked uncertain×central — where a stronger agent independently re-inspects (own hypothesis FIRST, then the recorded evidence) and confirms or overturns. Confidence is the coordination channel between tiers; no agent ever messages another.",
    "DESIGN CHANGES MIDWAY: when an intent is superseded, `loom intent retire <id> --reason … [--replaced-by <successor>]` — never delete (delete is for mistakes), never leave it counting. Retired = invisible to computation, visible to history; the command reports the triggered work (orphaned children, files that lost their only owner, dangling proofs). Address handoffs: `loom note add --for <role>` puts a message at the top of that lane's next relevant work item.",
    "THE HYPOTHESIS PLANE (pre-decision): an improvement idea is NOT work until it is proven. `loom hypothesis add --claim <what's wrong NOW> --proposal <the change> --predicted-outcome <measurable result> [--target <intent>]…` (any lane; the redesign-shaped smells emit this as their remedy). A DIFFERENT agent proves it: `loom next --mode prove` serves proposals ranked by target blast radius — `loom hypothesis prove <id> --verdict supported|refuted --evidence …` (analyzer lane; proposer ≠ prover; the verdict stamps the TARGETS edges). Then the builder decides: `loom hypothesis adopt <id> --spawned <planned-intent>…` converts it into ordinary build work AND writes the predicted outcome as a not_run Validation on the spawned intents — when the validator later marks that proof passed, the hypothesis derives `confirmed`: every adopted improvement is checked for whether it DELIVERED. `loom sync` stales hypothesis support when target code changes (the prove queue re-serves it as a RE-PROVE item). Speculation never counts in coverage/completeness — proving is optional, like discovery/review.",
    "THE CONSUMER PLANE (runtime proof of composition): everything else grounds claims by READING code — a saga proves intents compose by EXECUTING them the way a real consumer will. Write a YAML spec (ordered endpoint chain; every step names the intent it proves; captures thread one response into the next request), `loom saga add <spec.yaml>` to declare it (Validation type=saga + VALIDATES edges + the RELATES_TO path), `loom saga run <name>` to execute: consecutive passing steps stamp their RELATES_TO edge passing with RUNTIME evidence; the boundary into a failing step goes failing with the exact broken expectation ('expected 200, got 502'); never-reached steps stay untouched. Validator lane; exits non-zero so it runs under `loom validate`/CI; sync re-queues it when step-intent code changes. ENVIRONMENT VALUES: `{{ env.X }}` in a spec means the value arrives AT INVOCATION — `BASE_URL=http://localhost:3000 loom saga run <name>` — never stored in the graph (it points at a LIVE target; start the system under test first). `loom saga add`/`list` name what's required (`run with: BASE_URL=<value> …`); a missing value refuses to run with the exact invocation to use (nothing stamped — environment-not-ready is never recorded as a failed proof: `loom validate` marks it `blocked` instead). Use it for any endpoint-reachable surface; deliberately not a general HTTP test tool.",
    "YOU keep the travel format fresh: after graph changes, run `loom export` before committing code — `loom status` and `loom next --all` warn when the committed loom.graph.json drifted, and `loom export --check` verifies (exit code; CI wiring is optional extra hardening, not the primary guard — you are).",
];

/// What `loom sync` invalidates when a registered file's CONTENT changes — the
/// graph's impact analysis, taught up-front so a driver knows why green decays.
/// (Change detection is content-hash based: checkout/rebase mtime churn does
/// not false-flag. Every flipped edge gets a transition note naming the file
/// that caused it, so staleness explains itself in `loom edge show`/`loom next`.)
const RIPPLE: &[&str] = &[
    "RELATES_TO edges of intents grounded in the changed file → needs_reverification (re-inspect via `loom next --mode fix`; the edge's transition note names the changed file)",
    "passing GOVERNS verdicts on those intents → needs_reverification (quality green is re-earned via `loom next --mode quality` + `loom rule verdict`)",
    "passing TARGETS evidence on hypotheses aimed at those intents → needs_reverification (hypothesis support must be re-earned against the changed target code)",
    "Validations linked to those intents → last_result = not_run (re-run via `loom validate <intent>`, or every pending proof at once: `loom validate --all`)",
    "IMPLEMENTS locators that no longer occur in their file (renamed symbol) → needs_reverification, and reported — re-ground with a fresh locator",
    "files registered in the graph but missing on disk are reported — drop phantoms with `loom codefile remove <path>` or restore the file",
    "static imports are re-extracted per file — they feed `loom smells` (undeclared coupling, layering violations against the declared `loom domain order`) and discovery ranking",
];

/// The role lanes: who does what, and which `loom next` mode serves the lane.
/// Declared roles (LOOM_AGENT=llm:<role>) are ENFORCED — an agent acting
/// outside its lane gets an error. Bare `llm`/`human` = solo mode (all lanes).
const ROLE_LANES: &[(&str, &str, &str)] = &[
    ("builder",   "build",     "constructs the graph: intents, hierarchy, codefiles, IMPLEMENTS links, lifecycle; adopts/rejects proven hypotheses"),
    ("analyzer",  "discovery", "the Socratic loop: grounds RELATES_TO edges with criterion/evidence/verdict; proves hypotheses (`loom next --mode prove`)"),
    ("fixer",     "fix",       "resolves failing edges (`loom edge fix`) and needs_change intents; re-grounds what it repairs"),
    ("validator", "validate",  "proves intents: runs validations, confirms intents (`loom intent confirm`)"),
    ("quality",   "quality",   "the green gate: defines rules, applies them, records GOVERNS verdicts (`loom rule verdict`)"),
];

/// Orchestration — loom defines the CONTRACT (roles, lanes, owned fields, the
/// handoff dependency). It does NOT predefine the TOPOLOGY: one agent switching
/// hats, sequential subagents, parallel fan-out, or any mix are all valid. loom
/// enforces the lane when a role is declared; it never dictates when or how many.
const ORCHESTRATION: &[&str] = &[
    "loom tells you HOW to work with it; YOU choose how your agents are organized. Valid shapes:",
    "  · one agent, all roles (bare `llm`) — switch hats as the phase changes",
    "  · one agent declaring a role per phase (set LOOM_AGENT, work that lane, switch) — sequential",
    "  · many agents in sequence — builder finishes → analyzer picks up → … (handoff via the graph)",
    "  · many agents in parallel — each role works its own lane at once",
    "THE CONTRACT (identical in every shape):",
    "  · declare your role `LOOM_AGENT=llm:<role>` (or stay bare `llm` for solo)",
    "  · stay in your lane; fill ONLY your owned fields (`loom schema`); `loom note` anything out of lane",
    "  · hand off through the GRAPH, not chat — the next agent reads `loom status`/`loom next`/notes and continues",
    "HANDOFF ORDER is a DEPENDENCY, not a schedule: builder (construct + ground) → analyzer (verify)",
    "  → validator (prove) → quality (green); fixer on any failing/needs_change. Run these one at a",
    "  time or overlap where the graph allows — loom enforces the lane, never the timing.",
    "SEPARATION OF DUTIES is as strong as your topology: distinct agents per role = real (no one",
    "  green-lights its own work); one agent switching roles = discipline. `loom doctor` audits either way.",
    "THE LOOP: `loom status` → read `phase` → whoever owns that lane acts (`loom next` names the role +",
    "  fields per item) → repeat until phase=complete: vertical ✓, horizontal ✓, and the AUDIT gate",
    "  (zero open `loom smells` findings — every suspicion resolved or refuted via its remedy).",
];

fn brownfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the repo root."),
        ("seed intents", "Read the code; add `system` → `component` → `feature` intents (lifecycle defaults to `implemented`). Link with `loom edge hierarchy <parent> <child>`. GRANULARITY CONTRACT: system = 1–3 per repo (the product's purpose), component = 5–15 (cohesive subsystems), feature = MANY and ATOMIC — independently verifiable. The test: can you write ONE falsifiable criterion for it? If the description needs an 'and' ('RBAC manages users and roles and permissions'), it's several intents — seed 'users', 'roles', 'permissions' as children instead. Too coarse is recoverable (the scattered smell routes you to split the INTENT in the graph — cheap — never to refactor the code), but seeding at the right grain avoids the churn. OPTIONAL but cheap: register a small tag vocabulary as you go (`loom vocab add <term> --why \"<covers X, NOT Y>\"`) and tag intents (`--tag`, max 3) — tags from a shared registry collide where free prose doesn't, which is how duplicated responsibility in unrelated files gets caught later. Same stance for `--domain`: give intents consistent domain labels, and once the architecture's layering is clear, declare it (`loom domain order <top> … <bottom>`) — imports pointing UP that order surface as layering_violation, which no edge-level inspection catches (a recorded relationship doesn't excuse direction)."),
        ("ground to code", "`loom codefile add '<glob>'` then `loom edge implement <intent> <codefile> --locator \"<symbol>\"` (the symbol AS IT APPEARS in the file — e.g. `def shorten`, `fn run`, `class Link` — `loom sync` flags it stale if it isn't found verbatim)."),
        ("discover", "`loom next` repeatedly: read the code it points to, then record `loom edge explore <a> <b> ground|issue|independent …`."),
        ("fix", "`loom next --mode fix` for failing/stale edges."),
        ("coverage", "`loom coverage` — map or `loom ignore` every file so nothing is missed."),
        ("prove", "`loom validation add …` + `loom edge validates …`, then `loom validate <intent>`. Manual/async proofs: `loom validation mark <id> --result passed|failed --evidence …` (or `--result blocked --reason …` while something external is in the way)."),
        ("prove from outside", "If the system exposes endpoints, prove the COMPOSITION from the consumer's vantage: write a saga spec (ordered chain, each step bound to its intent) and `loom saga add` + `loom saga run` — passing runs stamp runtime evidence along the intent path; a failure lands as a failing edge naming the broken boundary."),
        ("gate", "Encode the codebase's norms: seed the packs `loom detect` recommends (`loom rule seed iso5055` baseline; `mobile`/`web-ui`/`service`/`data`/`concurrency` per repo kind) plus `loom rule add …` for repo-specific sticks. Then `loom next --mode quality` serves every never-measured rule×intent pair — ONE command resolves each: `loom rule verdict … --status passing|failing|independent --criterion … --evidence …` (the verdict CREATES the edge; independent = measured, doesn't apply). Measure at the highest HONEST altitude: a verdict on a component covers its descendants; drop to a leaf only where the rule has specific bite. The layer order is a norm too: if intents carry domains and the architecture is layered, `loom domain order <top> … <bottom>` arms the layering audit."),
        ("audit", "`loom smells` — derived suspicions the graph noticed for you: twin intents (split-brain), duplicated responsibility (tag collisions across unrelated code), overlapping ownership, scatter, tangles, undeclared coupling, layering violations (imports pointing UP the declared `loom domain order` — a recorded relationship doesn't excuse direction; adjudicate a deliberate up-dependency with a decision note on the importing intent), vocab drift, rules never held against coded intents, happy-path-only feature groups (no sad/fallback behavior declared). OPEN findings GATE GREEN: once every queue is dry the compass routes phase=audit until `loom smells` returns zero. Refute or confirm each via its remedy; `independent`/a decision note is as valuable as a fix (scatter/tangle/happy-path adjudicate via `loom note add --intent|--file … --kind decision`; a later structural change re-opens the question). Per-file ownership questions: `loom codefile show <path>`. The report also DISCLOSES what its detectors cannot see (untagged coded intents; domains in use with no declared order) — a quiet report is only as good as its armed instruments."),
        ("close out", "`loom next --all` — every lane's remainder as one prioritized list. Then `loom export --check` before committing, so the graph travels with the repo."),
    ]
}

fn greenfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the (empty/new) repo root."),
        ("design as planned intents", "Write the spec AS intents: `loom intent add … --level system|component|feature --lifecycle planned`. Each feature's criterion IS its acceptance contract — so features must be ATOMIC (one falsifiable criterion each; a description needing 'and' is several intents). Counts: system 1–3, component 5–15, features many. Use `--aspect happy|sad|fallback` so error paths are designed in."),
        ("capture architecture", "Relate intents: `loom edge hierarchy` for structure, `loom edge explore … ground` for contracts between components. If the design is layered, declare it up front: give intents `--domain` labels and `loom domain order <top> … <bottom>` — the build is then continuously audited for imports pointing up the order (layering_violation)."),
        ("build", "`loom next --mode build` → for each planned LEAF intent: write the code, `loom codefile add`, `loom edge implement`, then `loom intent mark <id> --lifecycle implemented`. Parents are deferred until their children are done, then surface as a roll-up. The criterion you wrote is your test."),
        ("verify", "Once built, `loom next` (discovery) and `loom validate` confirm reality matches the design. For endpoint-exposing designs, add a consumer saga per journey (`loom saga add` / `loom saga run`) — the design's composition is proven by execution, not just per-leaf tests."),
        ("gate", "Set the quality bar: seed the packs `loom detect` recommends (`loom rule seed <pack>`) + `loom rule add …` for repo-specific sticks, then earn green with `loom next --mode quality` + `loom rule verdict` (the verdict creates the edge; component altitude covers descendants)."),
    ]
}

fn refactor() -> Vec<(&'static str, &'static str)> {
    vec![
        ("map first if needed", "If the area isn't in the graph yet, do the brownfield steps for it."),
        ("find the problems", "`loom smells` — the graph surfaces split-brain twins, overlapping ownership, scatter, tangles, undeclared coupling, layering violations (imports against the declared `loom domain order`), recurrent trouble, and unmeasured quality rules; each finding carries its remedy command."),
        ("propose & prove redesigns", "Anything redesign-shaped (recurring breakage, a file split, a merge of twins) goes through the HYPOTHESIS PLANE before it becomes work: `loom hypothesis add --claim … --proposal … --predicted-outcome … --target <intent>` (the redesign smells emit this for you), then a DIFFERENT agent proves it (`loom next --mode prove` → `loom hypothesis prove`), then `loom hypothesis adopt --spawned <planned-intent>…` — the predicted outcome becomes a proof on the spawned work, and the hypothesis is `confirmed` only when that proof later passes. Unproven ideas die honestly (`loom hypothesis reject --reason …`) instead of becoming speculative refactors."),
        ("flag what must change", "`loom intent mark <id> --lifecycle needs_change --reason \"…\"`. Set/refresh the criterion to the desired end state; capture rationale (the --reason is recorded as a note). This is the honest 'known issue' state — no faking a verdict."),
        ("build", "`loom next --mode build` surfaces needs_change intents first. Make the minimal change, then `loom intent mark <id> --lifecycle implemented`."),
        ("experiment (optimization)", "Trying VARIANTS of an implementation needs no special node — use the primitives: the intent's criterion is the budget ('p99 < 50ms at 10k entries'), ONE benchmark validation (`loom validation add --type benchmark --command …`) is the yardstick reused across variants, and each variant's result goes in an append-only decision note (`loom note add --intent <id> --kind decision --text \"mutex-based: 120ms; lock-free: 45ms — chose lock-free, contention dominated\"`). The winner is what's on disk (grounded + passing benchmark); the losers live in git history; the WHY lives in the graph forever."),
        ("re-verify", "`loom sync` — it flags everything the change touched (stale relationships, stale quality green, invalidated proofs), and notes WHY on each flipped edge. Then `loom next --mode fix`, `--mode quality`, and `loom validate` until the compass is green."),
        ("close out", "`loom next --all` for the cross-lane remainder, then `loom export --check` before committing."),
    ]
}

/// PORTING: the semantic plane travels, the physical plane is rebuilt. The
/// criteria written for the old code become the acceptance contract for the new.
fn port() -> Vec<(&'static str, &'static str)> {
    vec![
        ("export the source", "In the SOURCE repo: `loom export` (its committed loom.graph.json carries the semantic plane — intents, hierarchy, criteria, quality rules, proofs-as-specs, the note history)."),
        ("adopt as design", "In the TARGET repo: `loom init . --name <target>` then `loom import <source-export> --as-planned`. Intents/hierarchy/criteria/rules/notes travel; CodeFiles, groundings, verdicts, and proof results do NOT — they were claims about the OLD code. Every intent arrives lifecycle=planned (the design), every proof not_run (the spec), every RELATES_TO/GOVERNS uninspected with its criterion intact (the contract). The target keeps its OWN graph identity — a port is a new graph."),
        ("re-realize", "`loom next --mode build` walks the design leaf-by-leaf in dependency order: write the code in the new language, `loom codefile add`, `loom edge implement <intent> <file> --locator …`, then `loom intent mark <id> --lifecycle implemented`. The criterion written for the old code is the acceptance test for the new — if it can't be met in the new language, that's a real design decision: record it (`loom note add --kind decision`) and update the intent, never silently diverge."),
        ("re-prove", "Each validation's command is a SPEC from the old toolchain — re-express it (`loom validation update <name> --command \"<new-toolchain equivalent>\"`; the reset-to-not_run is the point), then `loom validate <intent>`. Saga specs are the exception that travels VERBATIM: they speak HTTP, not the implementation language — copy the YAML across, `loom saga add` it, and the old consumer journey becomes the new code's first end-to-end acceptance test. Re-earn quality green per `loom next --mode quality` (the packs apply to the new language exactly as the old)."),
        ("verify the seams", "`loom next` (discovery) on the ported pairs: the criteria still describe how intents coexist — confirm the NEW code honors each, or record the divergence as an issue. Parity is measured per criterion, not vibes."),
        ("close out", "`loom next --all` until only optional discovery remains; `loom coverage` for unaccounted files (new-repo scaffolding may need `loom ignore add … --reason`); `loom export --check` before committing the new graph."),
    ]
}
fn seed() -> Vec<(&'static str, &'static str)> {
    vec![
        ("why this mode", "`loom sync` catches intent↔code drift mechanically; THIS mode catches user↔intent drift. A graph can be green and still describe a product the user no longer wants. Two loops share it: ELICIT (zero/few intents — capture the user's head from nothing) and ALIGN (populated graph — `loom next --mode align` serves meanings to re-affirm). Pick by graph state: sparse graph → elicit; otherwise align."),
        ("calibrate altitude", "Start at SYSTEM altitude: ask \"what is this product, in one sentence?\" and land the answer with `loom intent add … --level system --lifecycle planned`. Descend only while answers stay confident. Fluent user → grill at FEATURE level for falsifiable criteria and `loom intent add … --level feature`. Vague user → stay at system/component, PROPOSE candidate features with a recommended answer, then let them react. NEVER ask a vague user to enumerate features cold."),
        ("one question, one landing", "Ask ONE question at a time, always with your recommended answer. If code can answer it, switch to brownfield and explore instead of asking (`loom guide --mode brownfield`). The moment an answer crystallises, LAND it before the next question: behavior → `loom intent add … --lifecycle planned`; term → `loom vocab add`; hard tradeoff → `loom note add --kind decision`; error path → same tree with `--aspect sad|fallback`. Atomic only: if an intent description needs 'and', split it."),
        ("challenge, don't transcribe", "When the user's term collides with registered vocab, call it out and resolve with `loom vocab add` or a decision note (`loom note add --kind decision`). When a claim contradicts existing intents or code, surface the contradiction and make the user choose. Stress-test boundaries with scenarios: \"a payment fails mid-checkout — what does the user see?\" Each answer usually lands as `loom intent add … --aspect sad|fallback --lifecycle planned`."),
        ("terminate on completeness, not exhaustion", "The interview ends when the GRAPH says so, never when conversation peters out. Every question must close an enumerable gap: component with no children (`loom edge hierarchy`), feature with no criterion (`loom intent update … --description … --reason …`), happy-path-only group with no `--aspect sad|fallback`, or vocab collision (`loom vocab add`). No open gap → STOP. Explicitly declined scope lands as `loom note add --kind decision`; silence and decision must never look alike."),
        ("the align loop", "On a populated graph, `loom next --mode align` serves drift SUSPECTS only: meanings whose claims flipped since the user last confirmed them — code churn, but also a neighbour's redefinition or retirement rippling in, exactly like a changed codefile stales the claims earned against it — plus quiet wording unaffirmed past a grace period. Intents ruled `internal` are NEVER served (machinery isn't interview material) until a redefinition clears the ruling. Align the CONCEPT, not the wording: present what the product can DO because this exists (one or two plain sentences — jargon test: would a non-coder nod?), why it matters (its place in the design), and its audience UP FRONT — internal machinery presented as a product capability is how interviews go wrong. The item carries `visibility`, `where_it_sits`, and `not_to_confuse_with` (siblings + verified-independent neighbours) for exactly this. Vocabulary enters only when the user asks, stumbles, or uses a term that conflicts with the graph. Record exactly ONE outcome: concept still right → `loom intent confirm <id>`; words confusing, concept right → `loom intent update <id> --description … --reword --reason …` (no ripple, clock resets); concept evolved → translate their words BACK into a falsifiable description, `loom intent update <id> --description … --reason …`; internal machinery → `loom intent confirm <id> --visibility internal` (stops the asking until redefined); superseded → `loom intent retire <id> --reason … --replaced-by <successor>`; revealed gap → `loom intent add … --lifecycle planned`. A laundry-list meaning that needs 'and' is itself a finding — propose the split. Every outcome resets that intent's suspicion clock; then pull the queue AGAIN — it drains to empty, and empty is the stopping point: never one question, never the whole graph."),
        ("handoff", "After seeding/aligning, builder lanes take over: `loom status` routes the compass, and planned intents flow through `loom next --mode build`. Iterate code freely while intents are `planned` (nothing downstream to stale). After grounding, every meaning change uses `loom intent update … --reason …` and costs re-verification through `loom sync` — which is the point."),
    ]
}


fn resolve_mode(mode: Option<&str>) -> Result<&'static str> {
    if let Some(m) = mode {
        return match m {
            "greenfield" => Ok("greenfield"),
            "brownfield" => Ok("brownfield"),
            "refactor" => Ok("refactor"),
            "port" => Ok("port"),
            "seed" => Ok("seed"),
            other => anyhow::bail!("Unknown mode '{}'. Valid: greenfield, brownfield, refactor, port, seed", other),
        };
    }
    // Seed is explicit-only: this is a user-in-the-loop session, and the binary cannot detect "the user wants to talk".
    // Auto-detect from the repo: no source on disk → greenfield, else brownfield.
    let cwd = crate::db::resolve_root()?;
    Ok(if crate::repo::detect(&cwd)?.has_source { "brownfield" } else { "greenfield" })
}

pub fn run(mode: Option<&str>, printer: &Printer) -> Result<()> {
    let m = resolve_mode(mode)?;
    let steps = match m {
        "greenfield" => greenfield(),
        "refactor" => refactor(),
        "port" => port(),
        "seed" => seed(),
        _ => brownfield(),
    };

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode": m,
            "what_is_loom": "Externalized, falsifiable memory for understanding, building, and cleaning up a codebase. \
                A living graph of intents (what code should do), grounded in real files, every relationship carrying a \
                verification status + evidence. The graph is durable memory; the context window is the working set.",
            "planes": {
                "semantic": "Intent — what the system should do",
                "physical": "CodeFile — what exists on disk",
                "normative": "QualityRule — what good looks like",
            },
            "lifecycle": "Each intent has a lifecycle: planned (designed, not built) → implemented → needs_change (must change). \
                `loom next --mode build` drives planned/needs_change; discovery/fix drive relationship verification.",
            "steps": steps.iter().map(|(t, d)| serde_json::json!({"step": t, "do": d})).collect::<Vec<_>>(),
            "golden_rules": GOLDEN_RULES,
            "ripple": {
                "when": "Run `loom sync` after ANY code change — it detects mtime deltas on registered files and propagates the impact one hop. The graph structure IS the impact analysis.",
                "what_goes_stale": RIPPLE,
            },
            "roles": {
                "how": "Many limited agents lift together: each agent declares its role once via \
                    LOOM_AGENT=llm:<role> (or --by/--inspected-by/--author). Declared roles are ENFORCED — \
                    acting outside your lane is an error pointing you back to your own queue. \
                    Bare 'llm'/'human' = solo mode (one agent drives every lane). \
                    Separation of duties: the builder cannot green-light its own work; verdicts \
                    (ground/issue/independent, confirm, validate, rule verdict) belong to other lanes. \
                    `loom doctor` audits provenance after the fact.",
                "lanes": ROLE_LANES.iter().map(|(role, mode, what)| serde_json::json!({
                    "role": role, "queue": format!("loom next --mode {mode}"), "does": what,
                })).collect::<Vec<_>>(),
            },
            "orchestration": {
                "principle": "loom defines the CONTRACT (roles, lanes, owned fields, the handoff dependency). It does NOT predefine the TOPOLOGY — you choose how agents are organized; loom enforces the lane when a role is declared, never when or how many.",
                "topologies": [
                    "one agent, all roles (bare `llm`) — switch hats as the phase changes",
                    "one agent declaring a role per phase (set LOOM_AGENT, work that lane, switch) — sequential",
                    "many agents in sequence — builder finishes, analyzer picks up, … (handoff via the graph)",
                    "many agents in parallel — each role works its own lane at once",
                ],
                "contract": "Identical in every shape: declare your role `LOOM_AGENT=llm:<role>` (or stay bare `llm` for solo); stay in your lane; fill ONLY your owned fields (`loom schema`); `loom note` anything out of lane; hand off through the GRAPH (status/next/notes), not chat.",
                "handoff_order": "A DEPENDENCY, not a schedule: builder (construct + ground) → analyzer (verify) → validator (prove) → quality (green); fixer on any failing/needs_change. Run sequentially or overlap where the graph allows.",
                "separation_of_duties": "As strong as your topology: distinct agents per role = real (no one green-lights its own work); one agent switching roles = discipline. `loom doctor` audits provenance either way.",
                "loop": "`loom status` → read phase → whoever owns that lane acts (`loom next` names the role + fields per item) → repeat until phase=complete: vertical ✓, horizontal ✓, and zero open `loom smells` findings (the audit gate).",
            },
            "consumer_plane": {
                "what": "Runtime proof of COMPOSITION: a saga is an ordered chain of endpoint invocations run the way a real consumer will (captures thread one response into the next request). Everything else grounds claims by reading code; a saga stamps the RELATES_TO path between its step intents with EXECUTION evidence.",
                "loop": [
                    "write a YAML spec — every step binds to the intent it proves (`intent:` is first-class); see `loom saga add --help` for the format",
                    "declare (builder|validator): `loom saga add <spec.yaml>` — Validation (type=saga) + VALIDATES edges + the uninspected RELATES_TO path + the spec as a CodeFile",
                    "start the system under test, then run (validator): `BASE_URL=<live target> loom saga run <name>` — `{{ env.X }}` values are passed AT INVOCATION, never stored in the graph; `loom saga list` shows each saga's exact `run with:` line",
                    "outcome stamping: consecutive passing steps stamp passing with runtime evidence; the failing boundary stamps failing with the broken expectation ('expected 200, got 502'); never-reached steps stay untouched; a MISSING env value refuses to run with nothing stamped (environment-not-ready ≠ failed proof — `loom validate` records it as `blocked`)",
                    "staleness: `loom sync` flips the saga's proof to not_run when step-intent code changes — the validate queue re-serves it",
                ],
                "honesty": "Exits non-zero on failure (works under `loom validate`/CI). Deliberately a saga executor, not a general HTTP test tool — anything fancier is an ordinary command-based Validation.",
            },
            "hypothesis_plane": {
                "what": "The PRE-DECISION plane: an improvement idea is not work until proven. Hypothesis = falsifiable claim (what's wrong NOW) + proposal (the change) + predicted_outcome (measurable result). State machine: proposed → supported|refuted → adopted → confirmed | rejected.",
                "loop": [
                    "propose (any lane): `loom hypothesis add --name … --claim … --proposal … --predicted-outcome … [--target <intent>]…` — the redesign-shaped smells emit this as their remedy",
                    "prove (analyzer, a DIFFERENT agent): `loom next --mode prove` ranks proposals by target blast radius → `loom hypothesis prove <id> --verdict supported|refuted --evidence …` (stamps the TARGETS edges)",
                    "decide (builder): `loom hypothesis adopt <id> --spawned <planned-intent>…` — converts into ordinary build work AND writes the predicted outcome as a not_run Validation on the spawned intents; or `loom hypothesis reject <id> --reason …`",
                    "confirm (validator): when the outcome validation is marked passed, the hypothesis derives `confirmed` — adopted improvements are checked for whether they DELIVERED",
                    "staleness: `loom sync` flips hypothesis support when target code changes; the prove queue re-serves it as a RE-PROVE item",
                ],
                "honesty": "Speculation never counts in coverage/completeness — proving is optional like discovery/review; proposer ≠ prover when roles are declared.",
            },
            "completeness": {
                "vertical": "BINDING spine, mechanically verifiable: HIERARCHY is a well-formed tree (one parent per non-root intent, no cycles); every implemented leaf intent has ≥1 IMPLEMENTS (realized); every CodeFile is reached by ≥1 IMPLEMENTS. Surfaced as `vertically_complete` in `loom status`; details in `loom report` + `loom doctor` + `loom coverage`.",
                "horizontal": "OPTIONAL closure: every intent pair has an inspected RELATES_TO edge (passing/failing/independent). Surfaced as `horizontally_explored`. Good for deep understanding/cleanup, but never required for 'done'.",
            },
            "done_condition": "VERTICAL done = `vertically_complete: true` in `loom status` + `loom coverage` shows nothing unaccounted. HORIZONTAL (phase=complete) is optional polish.",
        }));
        return Ok(());
    }

    println!("══ loom — driving guide  [mode: {}] ═════════════════════════════════", m);
    println!();
    println!("WHAT IT IS");
    println!("  Externalized, falsifiable memory for understanding, building, and cleaning");
    println!("  up a codebase: a living graph of intents grounded in real files, where every");
    println!("  relationship carries a verification status + evidence.");
    println!();
    println!("THE THREE PLANES");
    println!("  semantic   Intent       — what the system is supposed to do");
    println!("  physical   CodeFile     — what actually exists on disk");
    println!("  normative  QualityRule  — what good looks like");
    println!();
    println!("LIFECYCLE  planned (designed, not built) → implemented → needs_change (must change)");
    println!("  `loom next --mode build` drives planned/needs_change; discovery/fix verify relationships.");
    println!();
    println!("PLAYBOOK ({} — {})", m, match m {
        "greenfield" => "design first, then build",
        "refactor" => "change existing code with intent",
        "port" => "re-realize a mapped system in a new language/repo",
        "seed" => "capture & re-align the user's head — interview, land, terminate on completeness",
        _ => "map & verify existing code",
    });
    for (i, (title, doc)) in steps.iter().enumerate() {
        println!("  {}. {}", i + 1, title);
        println!("       {}", doc);
    }
    println!();
    println!("GOLDEN RULES");
    for r in GOLDEN_RULES {
        println!("  • {}", r);
    }
    println!();
    println!("THE RIPPLE (run `loom sync` after ANY code change — the flag engine)");
    println!("  A changed file invalidates, one hop out:");
    for r in RIPPLE {
        println!("  → {}", r);
    }
    println!();
    println!("ROLE LANES (multi-agent: declare once with LOOM_AGENT=llm:<role>; enforced)");
    for (role, mode, what) in ROLE_LANES {
        println!("  {role:<10} queue: loom next --mode {mode:<10} {what}");
    }
    println!("  Bare 'llm'/'human' = solo mode (one agent, all lanes). Separation of duties:");
    println!("  builders never green-light their own work — verdicts live in other lanes,");
    println!("  and `loom doctor` audits provenance.");
    println!();
    println!("ORCHESTRATION (you have loom access — usually an orchestrator that can spawn subagents)");
    for line in ORCHESTRATION {
        println!("  {}", line);
    }
    println!();
    println!("Other modes: `loom guide --mode greenfield|brownfield|refactor|port|seed`. Start: `loom status` · `loom next`.");
    Ok(())
}
