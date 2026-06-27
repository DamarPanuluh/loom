//! `loom guide` — self-contained onboarding for an LLM new to loom, with a
//! mode-specific playbook (greenfield / brownfield / refactor). The mode is
//! auto-detected from the repo (via `loom detect`) unless given with `--mode`.

use anyhow::Result;

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

const CORE_RULES: &[&str] = &[
    "`loom next` is the router — it tells you the exact next command. Don't guess.",
    "`loom sync` after ANY code change. Bulk-drain: `loom next --mode fix --take 20` / `--mode quality --take 20` → `loom batch -` → `loom validate --all`.",
    "The Socratic loop per edge: read both intents → hypothesize → inspect code → ONE verdict. No code read, no verdict.",
    "HONEST confidence: 0.5-and-true beats 0.9-and-guessed. <0.7 routes to review. Empty evidence = laundered claim.",
    "Batch by neighborhood: `loom cluster <intent>` lists every unresolved edge on one node — work those while context is loaded.",
    "`→ Next:` is DIRECTIVE (just do it); `→ Recommended:` is DISCRETIONARY (your call — override when you hold priorities the graph can't see).",
    "Evidence and criteria must be substantive. The write gate is SYNTACTIC — it rejects empty / too-short / obvious-placeholder values (TODO, n/a, xxxx, one word repeated), NOT a grammatical-but-content-free criterion ('they are related yes indeed'); judging whether a criterion is truly FALSIFIABLE is the review lane's job (a <0.7 confidence routes to `loom next --mode review`). doctor audits provenance + vacuity, not semantic substance.",
    "CLOSE OUT: `loom next --all` → `loom export` → `loom export --check`.",
];

const DEEPER_RULES: &[&str] = &[
    "ASK THE MAP: `loom find \"<topic>\"` searches intent names/descriptions. `loom explain <intent|file>` answers it whole — groundings, coupling, governance, blast radius (`--impact`).",
    "Use `--json` on every command for machine-readable output. For audit: `loom smells --summary --json` and `loom coverage --summary --json` first; full `--json` only for specific findings.",
    "Every command has `--help`. `loom schema` = data model; `loom status` = where you are; `loom doctor` = integrity.",
    "Prescriptive intents (planned/needs_change) still need a falsifiable criterion — that's what makes the design a test.",
    "VERTICAL spine (HIERARCHY tree + IMPLEMENTS + CodeFile reach) feeds REALIZED. HORIZONTAL risk closure (explicit RELATES_TO + signal-bearing unexplored pairs) feeds HARDENED. `loom edge unexplored --class suspected-coupling` lists owed risk pairs; `--class all` is optional survey.",
    "REALIZED needs vertical closed + every leaf proven by a discriminating test. HARDENED needs horizontal risk closed — inspect signal-bearing pairs, `ground` real couplings, mark non-couplings `independent`.",
    "360°: grounded · realized · explored · measured · proven. `measured` never closes alone: `loom rule seed iso5055|mobile|web-ui|service|data|concurrency|docker` → `loom next --mode quality`.",
    "FEDERATION: `loom init --name`, `loom delegate add`, `loom delegate seam`. Children export, parent observes. `loom init --observed` for code you don't own.",
    "UNBLOCK proofs needing live deps: scan repo for docker-compose/Makefile/scripts/README — loom ships no mock. `blocked` only when you genuinely cannot.",
    "ADOPT THE LANE: `loom guide --role <role>` serves the lane skill JIT. `effort: low|mid|high` is about WORK, not models. Spawn sub-contexts ONLY for genuinely bulk queues.",
    "DESIGN CHANGES: `loom intent retire <id> --reason … [--replaced-by …]` — never delete (delete is for mistakes). `loom note add --for <role>` for handoffs.",
    "HYPOTHESIS PLANE: `loom hypothesis add --claim … --proposal … --predicted-outcome …` → `loom next --mode prove` → `loom hypothesis adopt|reject`. Proposer ≠ prover.",
    "CONSUMER PLANE (sagas): `loom saga add <spec.yaml>` → `loom saga diagnose` (triage) → `loom saga run` (stamp proof). Missing env = blocked, not failed.",
    "PROOF RELEVANCE: `loom validate` checks whether a passing test actually exercises the grounded code — static import/symbol-usage analysis by default. For a DEFINITIVE answer (the test imports but never calls the symbol), enable coverage: `LOOM_COVERAGE_FILE=<lcov-path> loom validate <intent>` — an LCOV report showing the grounded symbol's lines were executed confirms executed-proven regardless of imports.",
    "EXPORT: `loom export` before committing. `loom export --check` verifies freshness. `loom wiki` for human-readable architecture doc.",
];

/// What `loom sync` invalidates when a registered file's CONTENT changes — the
/// graph's impact analysis, taught up-front so a driver knows why green decays.
/// (Change detection is content-hash based: checkout/rebase mtime churn does
/// not false-flag. Every flipped edge gets a transition note naming the file
/// that caused it, so staleness explains itself in `loom edge show`/`loom next`.)
const RIPPLE: &[&str] = &[
    "RELATES_TO edges of intents grounded in the changed file → needs_reverification (re-inspect via `loom next --mode fix`; the edge's transition note names the changed file), with three exceptions that keep the N×N grid from re-staling on every edit: an `independent` edge re-opens ONLY when a NEW import coupling appears (a behavior-preserving edit cannot create an interaction, so independence is durable); a PASSING edge coupled SOLELY by `imports` is mechanically re-derived — it re-opens only if the import itself is added/removed, never on an unrelated body edit; and meaning-only kinds (shares_vocab/same_domain/doc_reference) never re-open. Edges carrying a judgment coupling (calls/inheritance/shares_state) DO re-open when their owning file changes",
    "passing GOVERNS verdicts on those intents → needs_reverification (quality green is re-earned via `loom next --mode quality` + `loom rule verdict`)",
    "passing TARGETS evidence on hypotheses aimed at those intents → needs_reverification (hypothesis support must be re-earned against the changed target code)",
    "Validations linked to those intents → last_result = not_run (re-run via `loom validate <intent>`, or every pending proof at once: `loom validate --all`)",
    "IMPLEMENTS locators that no longer occur in their file (renamed symbol) → needs_reverification, and reported — re-ground with a fresh locator",
    "files registered in the graph but missing on disk are reported — drop phantoms with `loom codefile remove <path>` or restore the file",
    "static imports are re-extracted per file — they feed `loom smells` (undeclared coupling, layering violations against the declared `loom layer order`) and discovery ranking",
    "the ripple is SYMBOL-PRECISE for an IMPLEMENTS edge grounded to an EXTRACTED TOP-LEVEL symbol: it flips only when THAT symbol's body changed — a comment/whitespace/unrelated-symbol edit flips nothing. But tree-sitter extracts only top-level symbols, so a grounding to a NESTED symbol (a class method) falls back to WHOLE-FILE — ANY edit to the file, including a comment, re-opens it; `loom edge implement` warns inline when a locator resolves to such a nested/non-extracted symbol. GOVERNS verdicts and linked validations on a changed owning file likewise re-open and are re-earned (this is why a method-grounded proof re-verifies on a cosmetic edit — it is honest, not a bug)",
    "the auto `transition` notes recording these flips are bounded per target (transition_cap, default 20) so the flip-flop log never bloats — `loom sync` trims it; tune with `loom note prune --set-cap N` (0 = off). `loom smells` also surfaces ADVISORY `cochange_coupling` (files that change together in git but whose intents aren't linked), `shotgun_surgery` (one intent repeatedly co-changing with many unrelated intents), and `code_clone` (cross-file structural duplication via per-symbol shape_hash, with exact body_hash fallback)",
];

const ARCHITECTURE_METADATA_GUIDANCE: &str = "Architecture metadata is positive evidence, not a template. Use `--domain` for product/business facets (auth, billing, onboarding); it has NO layering effect. Use `--layer` only for dependency direction you mean to audit — do NOT invent generic backend/frontend/database labels unless the repo actually has those seams. Prefer repo-shaped names (for a CLI: presentation/commands/application/queries/persistence, etc.). Once enough coded intents carry honest layer labels, run `loom layer list`; if layers are in use but the order is empty, tell the user the layering detector is unarmed, then declare the real order with `loom layer order <top> … <bottom>`. That arms `layering_violation`: imports pointing UP the declared order are findings, and a recorded RELATES_TO edge does not excuse direction. Leave `--layer` unset when the direction is unknown or not enforceable. Use `--boundary inbound|outbound` only for system-boundary crossings (provider surfaces or external consumer dependencies); internal machinery stays unset.";
const AUTONOMY_SET_WITH: &str = "loom init --autonomy <autonomous|guided>";

fn role_setup(role: &str) -> String {
    format!("export LOOM_AGENT=llm:{role}")
}

/// The role lanes: who does what, and which `loom next` mode serves the lane.
/// Declared roles (LOOM_AGENT=llm:<role>) are ENFORCED — an agent acting
/// outside its lane gets an error. Bare 'llm'/'human' = solo mode (all lanes).
const ROLE_LANES: &[(&str, &str, &str)] = &[
    ("builder",   "build",     "constructs the graph: intents, hierarchy, codefiles, IMPLEMENTS links, lifecycle; backfills derived graph surfaces via `loom next --mode populate`; adopts/rejects proven hypotheses"),
    ("analyzer",  "discovery", "the Socratic loop: grounds RELATES_TO edges with criterion/evidence/verdict; proves hypotheses (`loom next --mode prove`)"),
    ("fixer",     "fix",       "resolves failing edges (`loom edge fix`) and needs_change intents; re-grounds what it repairs"),
    ("validator", "validate",  "proves intents: runs validations, confirms intents (`loom intent confirm`)"),
    ("quality",   "quality",   "the green gate: defines rules, applies them, records GOVERNS verdicts (`loom rule verdict`)"),
];

/// Per-role working DISCIPLINE — the BODY of the lane-skill the binary serves
/// JUST-IN-TIME. Adopting a lane = pulling `loom guide --role <role>`, which
/// renders the mandate (ROLE_LANES) + allowed actions (the gate) + THIS
/// discipline into one complete, self-contained, adoptable skill. The working
/// wisdom the lane agent files used to carry now lives IN THE BINARY, emitted on
/// demand — no shipped/installed markdown to scavenge, and it can't drift from
/// the gate.
///
/// Crafted on the patterns proven by widely-used discipline skills (mattpocock,
/// ponytail, gstack, karpathy): each lane LEADS with a thesis, carries a named
/// runnable loop, elevates its honesty law to its own line, bakes refusal
/// conditions into the procedure (do-X-NOT-Y), and ends on a single anchor
/// motto. (role, the JIT-trigger `description`, the anchor motto, discipline lines).
const ROLE_DISCIPLINE: &[(&str, &str, &str, &[&str])] = &[
    ("builder",
     "Adopt when loom routes you to the builder/build lane — seeding or decomposing intents, or REALIZING a planned intent by writing the code its criterion demands (phase=build, `loom next --mode build`).",
     "Build to the criterion, prove it, then mark it — a leaf marked implemented with no proof is a promise, not a fact.",
     &[
        "THE LOOP, per planned leaf: the criterion IS the spec AND the acceptance test. Write the code it demands → `loom codefile add` → `loom edge implement <intent> <file> --locator \"<symbol AS IT APPEARS>\"` (verified against the file NOW — a typo'd symbol is rejected here) → PROVE the criterion (add + run a validation) → `loom intent mark <id> --lifecycle implemented`.",
        "GRANULARITY when seeding: 1–3 `system`, 5–15 `component`, MANY ATOMIC `feature` leaves — ONE falsifiable criterion each. Description needs an 'and'? It is several intents — split it. (Too coarse trips the `scattered` smell later.)",
        ARCHITECTURE_METADATA_GUIDANCE,
        "A planned PARENT whose children are implemented is a ROLL-UP: verify each child meets its criterion, then mark it — NEVER write code at that altitude.",
        "SUPERSEDED design → `loom intent retire <id> --reason … [--replaced-by …]` (keeps history, exits computation). Delete is ONLY for things that should never have existed.",
        "REFUSE to grade your own work: you record NO criterion/evidence/verdicts on it — analyzer grounds it, validator proves it, quality grades it. That separation is what makes the graph trustworthy; `loom doctor` audits it.",
        "DONE WHEN: the leaf is grounded (IMPLEMS locator matches a real symbol), proven (validation passed), and marked implemented. A leaf marked implemented with no proof is a promise, not a fact.",
        "NEVER: mark implemented before grounding · ground to a mockup or spec doc (only production code) · seed one intent for two responsibilities (split it) · delete instead of retire (delete erases history; retire preserves it).",
     ]),
    ("analyzer",
     "Adopt when loom routes you to the analyzer/discovery lane — grounding RELATES_TO edges or proving hypotheses (phase=discovery; `loom next --mode discovery|prove|review`).",
     "0.5-and-true beats 0.9-and-guessed — honest confidence is the safety net; a faked 0.9 poisons the graph AND skips it.",
     &[
        "THE SOCRATIC LOOP is the skill; everything else is mechanical. Per edge: read both intents → form a hypothesis (\"I expect the code to show X\") → read the ACTUAL code → record exactly ONE verdict. NEVER record a verdict you didn't check — no code read, no verdict.",
        "VERDICTS: `loom edge explore <a> <b> ground --criterion … --confidence <honest>` (it holds — the criterion IS the assertion, so a ground carries no evidence field) · `… issue --criterion … --evidence …` (the code CONTRADICTS the claim — evidence is what backs the contradiction) · `… independent --notes …` (they don't interact). Independence is a REAL verdict — it gives closure at zero centrality cost. NEVER fake a relationship to look productive: when the evidence reads 'foundation / universal / not specific', the verdict is INDEPENDENT, not passing@0.6.",
        "CONFIDENCE is the cross-tier channel: anything <0.7 auto-surfaces in `loom next --mode review` for a stronger pass. Record the confidence you ACTUALLY have — the review queue exists so your uncertainty is SAFE to record, not something to hide behind a fake 0.9.",
        "REVIEW sub-lane (`loom next --mode review`): re-inspect low-confidence × central verdicts. Form your OWN hypothesis FIRST, THEN read the recorded evidence, then CONFIRM or OVERTURN — never rubber-stamp.",
        "BULK: `loom next --mode discovery --take 50` groups unexplored pairs with both intents + groundings inline — read each neighborhood ONCE, apply the whole group in one `loom batch -`. `loom cluster <intent>` lists every unresolved edge on one node.",
        "HYPOTHESES: `loom next --mode prove` → `loom hypothesis prove <id> --verdict supported|refuted --evidence …` (proposer ≠ prover). Out-of-lane finding → `loom note add --for <role>`.",
        "DONE WHEN: every edge you picked up has a verdict (ground/issue/independent) with a substantive criterion and honest confidence. Empty evidence = laundered claim — the review queue catches it regardless of confidence.",
        "NEVER: record a verdict without reading the code · record passing@0.9 when the evidence reads 'foundation/universal' (that's independent) · use one evidence string for many unrelated edges (that's laundering — the doctor flags it) · rubber-stamp a review (form your OWN hypothesis first).",
     ]),
    ("fixer",
     "Adopt when loom routes you to the fixer/fix lane — repairing a failing edge or a needs_change intent at root cause (`loom next --mode fix`).",
     "Fix the root, not the symptom — then sync and let the ripple show you everything the fix touched.",
     &[
        "REPAIR ONLY: failing RELATES_TO edges (`loom edge fix`) and `needs_change` intents, at the ROOT CAUSE. New-code construction is the builder's — NOT yours.",
        "THE RIPPLE is the discipline: end every repair with `loom sync` (it stales every claim the change touched), then re-ground/re-verify what it flagged. Expect fix → sync → re-verify → re-prove → re-green; skipping the sync leaves the graph lying about your own change.",
        "Re-ground what you repaired (`loom edge implement` with a fresh locator if a symbol moved), then `loom intent mark <id> --lifecycle implemented` to close the loop.",
        "DONE WHEN: the failing edge is passing (re-grounded with evidence) AND `loom sync` ran AND the ripple it created is addressed (re-verify what it staled).",
        "NEVER: patch the symptom while the root cause persists · write new features (that's builder work) · skip `loom sync` after a repair (the graph lies about your change) · mark passing without re-grounding (a moved symbol means the old locator anchors nothing).",
     ]),
    ("validator",
     "Adopt when loom routes you to the validator/validate lane — proving intents by running their validations (`loom next --mode validate`).",
     "Unblock before you block; never fake a pass — a proof you didn't run is not a proof.",
     &[
        "PROVE intents: `loom validate <intent>` runs the linked proofs; `loom validate --all` re-runs every not_run proof after a sync flood. Record passed/failed from what you ACTUALLY ran.",
        "UNBLOCK FIRST: a proof needing a live dep (DB/service/queue) is NOT automatically blocked — scan the repo for how it provisions things (docker-compose, Makefile/justfile, scripts/, package.json, the README), stand it up, pass the address in at invocation. ONLY when you genuinely cannot is it `loom validation mark <id> --result blocked --reason …`.",
        "A FAILING proof means the intent is NOT fulfilled — flag it (`loom intent mark <id> --lifecycle needs_change --reason …`) or hand the fixer a note. Manual/async proof → `loom validation mark <id> --result passed --evidence …`; confirm meaning with `loom intent confirm`.",
        "DONE WHEN: every `not_run` validation in the queue has a result (passed/failed/blocked), recorded from what you ACTUALLY ran — not from reading the command and guessing.",
        "NEVER: mark passed without running the proof · mark blocked before scanning the repo for how to provision the dependency · leave a failing proof silent (flag the intent needs_change or hand the fixer a note) · accept a proof that tests the wrong thing (a test that passes but doesn't exercise the intent's criterion is not a proof).",
     ]),
    ("quality",
     "Adopt when loom routes you to the quality lane — holding quality rules against coded intents and recording GOVERNS verdicts (`loom next --mode quality`).",
     "Measure at the highest honest altitude; `independent` is as valuable as `passing` — never fake either to clear the gate.",
     &[
        "THE GREEN GATE: seed the packs `loom detect` recommends (`loom rule seed iso5055|…`), then `loom next --mode quality` serves every never-measured rule×intent pair.",
        "ONE verdict per pair, after reading the intent's grounded code ONCE: `loom rule verdict <rule> <intent> --status passing|failing|independent --criterion … --evidence … --confidence <honest>` (the verdict CREATES the edge). `independent` = measured, no surface here — record it, NEVER fake a passing.",
        "ALTITUDE: a verdict on a component covers its descendants ONLY with --covers-descendants; otherwise it covers the component alone — drop to a leaf wherever the rule has specific bite.",
        "A `failing` verdict routes to the fixer; quality re-earns green after the fixer's sync. HONEST confidence: <0.7 routes to review. Bulk via `loom next --mode quality --take 50` + `loom batch -`.",
        "DONE WHEN: the quality queue is empty (every rule×intent pair measured) AND each verdict has substantive evidence and honest confidence. `independent` closes a pair — it's measured, not skipped.",
        "NEVER: fake passing to clear the gate · record independent without reading the code (independent means you checked and found no surface) · use one evidence string across many pairs (that's laundering) · leave a failing verdict without routing to the fixer.",
     ]),
];

/// The lane-skill name loom serves/installs for a role: `loom-<role>`.
fn role_skill_name(role: &str) -> String {
    format!("loom-{role}")
}

/// A role's JIT-trigger `description`, anchor motto, and discipline lines (the
/// skill body). `None` for a role with no authored discipline.
fn role_discipline(role: &str) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    ROLE_DISCIPLINE
        .iter()
        .find(|(r, _, _, _)| *r == role)
        .map(|(_, desc, anchor, lines)| (*desc, *anchor, *lines))
}

/// Render a role's lane-skill as a standalone `SKILL.md` — the OPT-IN install
/// artifact (`loom skill install`). A generated PROJECTION of the lane table,
/// exactly like `loom wiki` / `loom export`: regenerable, and its body points the
/// live charge back to `loom guide --role <role>` so a pinned copy can't silently
/// drift from what the gate enforces. Empty for an unknown/disciplineless role.
pub(crate) fn lane_skill_markdown(role: &str) -> String {
    let Some((description, anchor, discipline)) = role_discipline(role) else {
        return String::new();
    };
    let skill = role_skill_name(role);
    let mandate = ROLE_LANES
        .iter()
        .find(|(r, _, _)| *r == role)
        .map(|(_, _, what)| *what)
        .unwrap_or("");
    let queue = crate::gate::mode_for_role(role)
        .map(|m| format!("loom next --mode {m}"))
        .unwrap_or_else(|| "loom next".to_string());
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("name: {skill}\n"));
    s.push_str(&format!("description: {description}\n"));
    s.push_str("---\n\n");
    s.push_str(&format!("# loom {role} lane — adopt this discipline\n\n"));
    s.push_str(&format!(
        "<!-- GENERATED by loom — a projection of the gate's lane table, like `loom wiki`. \
         Regenerate after a loom upgrade with `loom skill {role}`. Your LIVE charge is always \
         `loom guide --role {role}`. -->\n\n"
    ));
    s.push_str(&format!("**THE LAW** — {anchor}\n\n"));
    s.push_str(&format!("**Mandate.** {mandate}\n\n"));
    let setup = role_setup(role);
    s.push_str(&format!("## Adopt\n\n```\n{setup}\n{queue}\n```\n\n"));
    s.push_str("## Your lane (everything else errors — hand it off)\n\n");
    for l in crate::gate::actions_for_role(role) {
        s.push_str(&format!("- {}\n", l.action));
    }
    s.push_str("\n## Discipline\n\n");
    for line in discipline {
        s.push_str(&format!("- {line}\n"));
    }
    s.push_str(&format!(
        "\nOut of lane → `loom note add --for <role>`. Your live charge is always \
         `loom guide --role {role}`.\n\n⟐ {anchor}\n"
    ));
    s
}

/// The lane-skill manifest: (skill name, role, JIT-trigger description) for every
/// enforced lane that carries a discipline — the menu `loom skill list` prints.
pub(crate) fn lane_skill_manifest() -> Vec<(String, &'static str, &'static str)> {
    crate::db::schema::ROLES
        .iter()
        .filter_map(|&role| {
            role_discipline(role).map(|(desc, _, _)| (role_skill_name(role), role, desc))
        })
        .collect()
}

/// Orchestration — loom defines the CONTRACT (roles, lanes, owned fields, the
/// handoff dependency). It does NOT predefine the TOPOLOGY: one agent switching
/// hats, sequential subagents, parallel fan-out, or any mix are all valid. loom
/// enforces the lane when a role is declared; it never dictates when or how many.
const ORCHESTRATION: &[&str] = &[
    "ADOPT THE LANE, don't go find an agent for it. loom serves each lane as a SKILL just-in-time: when",
    "  the compass routes you to a lane, run `loom guide --role <role>` and the binary hands you that",
    "  lane's complete discipline (THE LAW + the loop + the honesty guards) to ADOPT in your current",
    "  context. No install, nothing to scavenge. (`loom skill install` only PINS them as harness skills —",
    "  optional, for control; the binary alone is enough.)",
    "loom tells you HOW to work with it; YOU choose the TOPOLOGY. Valid shapes, DEFAULT first:",
    "  · ONE context, adopt the lane-skill the compass names, switch skills as the phase changes — the",
    "    default: warm context, no cold-start, no handoff cost (`loom guide --role <role>` per switch)",
    "  · one context declaring a role per phase (set LOOM_AGENT, work that lane, switch) — sequential",
    "  · SCALE OUT only when a queue is genuinely BULK: spawn a fresh sub-context for that ONE lane (a",
    "    cheap model can flood thousands of grid edges in isolation), sequential or parallel — handoff via",
    "    the GRAPH. Reserve this for volume; in-context adoption is the norm, not subprocess fan-out.",
    "CONCURRENCY (loom handles it): loom DOES cross-process-lock writes — an advisory flock on",
    "  `.loom/graph.lock`, taken per write transaction, so at most one write session touches a graph at a",
    "  time (readers still run concurrently under WAL). A competing writer waits up to LOOM_LOCK_DEADLINE_MS",
    "  (default 5000ms), then fails with a NAMED error ('graph write lock is held by another loom session …",
    "  loom serializes writers'), never a raw 'database is locked'. So parallel lane work is SAFE against",
    "  corruption on one graph — loom serializes it for you. For heavy parallel WRITE fan-out, handle that",
    "  named lock-wait error (retry/backoff) or give each writer its own graph clone + merge later. Sequential",
    "  shapes never contend; reads never block.",
    "THE CONTRACT (identical whether you adopt in-context or spawn a sub-context):",
    "  · declare your role `LOOM_AGENT=llm:<role>` (the lane-skill's SETUP line does this; or stay bare `llm` for solo)",
    "  · stay in your lane; fill ONLY your owned fields (`loom schema`); `loom note` anything out of lane",
    "  · hand off through the GRAPH, not chat — the next reader (you later, or another context) reads `loom status`/`loom next`/notes and continues",
    "HANDOFF ORDER is a DEPENDENCY, not a schedule: builder (construct + ground) → analyzer (verify)",
    "  → validator (prove) → quality (green); fixer on any failing/needs_change. Run these one at a",
    "  time or overlap where the graph allows — loom enforces the lane, never the timing.",
    // PERFORMANCE/storage guidance is single-sourced in PERFORMANCE_GUIDANCE and
    // printed after this list (so the human render and the json
    // `orchestration.performance` field can never drift — see that const).
    "SEPARATION OF DUTIES is enforced at the WRITE BOUNDARY on the LOOM_AGENT role string — NOT on process",
    "  identity: a spawned context per role and one context switching lane-skills are held to the SAME gate.",
    "  Distinct contexts make separation STRUCTURAL; one context switching skills makes it DISCIPLINE —",
    "  `loom doctor` audits provenance either way.",
    "SOLO MODE IS SILENT BY DEFAULT: a bare `llm` (no LOOM_AGENT role) passes every lane — correct for",
    "  one driver, but if you mean the lanes to bind, set LOOM_AGENT=llm:<role> before recording verdicts",
    "  (adopting the lane-skill sets it). `loom batch` FLAGS all-solo at record time (advisory, never",
    "  rejected), and `loom doctor` hints when ALL verdicts are solo.",
    "THE LOOP: `loom status` → read the maturity ladder + its FOCUS (the lowest unmet rung) → whoever owns",
    "  that rung's lane acts (`loom next` names the role + fields per item) → repeat until the ladder reaches",
    "  your target rung (Production-ready for a full close; the focus rung names the exact remaining gap).",
];

/// The storage/performance guidance, defined ONCE. Both the human render (printed
/// after ORCHESTRATION) and the json `orchestration.performance` field consume
/// THIS — so the hand-mirrored-copy drift that once shipped it human-only
/// (invisible to --json drivers, its exact audience) is now impossible by
/// construction. Single-source is the OCP fix: extend in one place.
const PERFORMANCE_GUIDANCE: &str = "SQLite is the active embedded graph store: every command opens `.loom/graph.sqlite` directly, uses transactions for multi-step mutations, and exports `loom.graph.json` as the portable review/commit artifact. `loom serve` is retired; there is no daemon to start, drain, or tune.";

const LIFECYCLE_SUMMARY: &str = "Intent.lifecycle is the implementation-work axis: `planned` means designed but not built, `implemented` means grounded in code, `needs_change` means known work remains, and `deferred` means consciously PARKED — the design is valid and still wanted, just not being built now (e.g. premature for current scale). `deferred` is distinct from retirement: `loom intent retire` (status=deprecated) is for meanings that are SUPERSEDED or out of scope — dead, kept for history — whereas a deferred intent is alive and may resume. Faking deferral as `planned` (it nags the build queue) or as `deprecated` (it isn't superseded) would both be dishonest, which is why deferral earns its own work-axis state. A deferred intent leaves the build queue and never blocks a parent roll-up; resume it with `loom intent mark <id> --lifecycle planned`.";

const LIFECYCLE_STATES: &[(&str, &str, &str)] = &[
    (
        "planned",
        "Designed promise; not expected to be grounded in current code yet.",
        "`loom next --mode build` serves it; build the leaf, ground it with IMPLEMENTS, then mark implemented.",
    ),
    (
        "implemented",
        "Current code is meant to realize this intent.",
        "It must stay grounded, proven, and measured; `loom sync` can stale evidence around it after code or meaning changes.",
    ),
    (
        "needs_change",
        "Known issue or refactor target; the graph admits work is still needed.",
        "`loom next --mode build`/fixer work repairs it, then `loom intent mark <id> --lifecycle implemented` closes the implementation loop.",
    ),
    (
        "deferred",
        "Consciously parked: the design is valid and still wanted, but not being built now (e.g. premature for current scale). Distinct from retire (superseded/deprecated) — deferred is alive, just not active work.",
        "Out of the build queue and never blocks a parent roll-up; the deferral rationale belongs in a `loom note add --kind decision`. Resume with `loom intent mark <id> --lifecycle planned`.",
    ),
];

const LIFECYCLE_TRANSITIONS: &[(&str, &str, &str)] = &[
    (
        "seed_or_design",
        "new behavior -> planned",
        "`loom guide --mode seed`, `loom saga add --spawn-missing`, and hypothesis adoption land future work as planned intents.",
    ),
    (
        "build",
        "planned -> implemented",
        "Write code, `loom codefile add`, `loom edge implement <intent> <file> --locator ...`, then mark implemented.",
    ),
    (
        "repair",
        "implemented -> needs_change -> implemented",
        "Use `loom intent mark ... --lifecycle needs_change --reason ...` for known issues; fix, re-ground, and mark implemented.",
    ),
    (
        "defer_or_resume",
        "planned <-> deferred",
        "`loom intent mark ... --lifecycle deferred --reason ...` parks valid-but-not-now work (record the why in a decision note); `--lifecycle planned` resumes it. Distinct from retire, which is for superseded design.",
    ),
    (
        "meaning_change",
        "active meaning -> stale evidence",
        "`loom intent update ... --reason ...` preserves history and ripples claims/proofs for re-verification; `--reword` is wording-only and does not ripple.",
    ),
    (
        "retire",
        "active intent -> status=deprecated",
        "`loom intent retire ... --reason ... [--replaced-by ...]` removes superseded design from computation while keeping history.",
    ),
    (
        "port",
        "source graph -> target planned design",
        "`loom import <source-export> --as-planned` resets imported intents to planned, proofs to not_run, and verdicts to uninspected.",
    ),
];

const RELATED_STATUS_FAMILIES: &[(&str, &str)] = &[
    (
        "intent.status",
        "proposed/confirmed/deprecated says whether the meaning itself is accepted or retired; it is distinct from lifecycle.",
    ),
    (
        "edge.inspection_status",
        "uninspected/passing/failing/independent/needs_reverification says whether a relationship claim has current evidence.",
    ),
    (
        "validation.last_result",
        "not_run/passed/failed/blocked says whether a proof has current runtime/manual evidence.",
    ),
    (
        "hypothesis.status",
        "proposed/supported/refuted/adopted/confirmed/rejected gates redesign ideas before they become lifecycle work.",
    ),
];

/// The teaching layer's COMPLETENESS CONTRACT: the sections every
/// `loom guide --json` payload must expose — the analog of the graph's vertical
/// spine, for the self-teaching plane. `guide_json_exposes_every_canonical_section`
/// ratchets it (a registered section the json render forgets fails the build),
/// exactly as `every_flag_requiring_command_ships_an_example` guards per-command
/// help. Adding teaching = add the key here + provide it in the payload; the
/// build refuses a half-landed section. (Coverage leg of teaching completeness;
/// the Just-In-Time leg is the per-mutation anchor, the Findability leg is the
/// guide/schema/find pull surface.)
#[cfg(test)]
const GUIDE_SECTIONS: &[&str] = &[
    "what_is_loom",
    "planes",
    "lifecycle",
    "architecture_metadata",
    "steps",
    "golden_rules",
    "deeper_rules",
    "roles",
    "orchestration",
    "consumer_plane",
    "hypothesis_plane",
    "completeness",
    "done_condition",
    "output_hygiene",
];

/// FINDABILITY CONTRACT (leg 3 of teaching completeness): the pull surfaces the
/// guide MUST name, so when Just-In-Time anchoring isn't enough the LLM can find
/// what it needs from the entry point instead of guessing. `guide_names_every_pull_surface`
/// ratchets it — drop a reference here and the build fails.
#[cfg(test)]
const FINDABILITY_SURFACES: &[&str] = &[
    "loom status",             // where am I
    "loom next",               // what's the next item
    "loom inbox",              // raw language intake
    "loom find",               // ask the map (keyword search)
    "loom schema",             // the data model
    "--help",                  // per-command EXAMPLE + flags
    "loom smells --summary",   // bounded audit counts before detail
    "loom coverage --summary", // bounded coverage counts before archives
];

/// The sub-keys the json `orchestration` object must carry. Pinning the
/// structure keeps every orchestration concept the human render teaches
/// reachable in json.
#[cfg(test)]
const ORCHESTRATION_KEYS: &[&str] = &[
    "principle",
    "topologies",
    "concurrency",
    "contract",
    "handoff_order",
    "separation_of_duties",
    "loop",
    "performance",
];

#[cfg(test)]
const LIFECYCLE_JSON_KEYS: &[&str] = &[
    "summary",
    "active_states",
    "transitions",
    "related_status_families",
];

fn brownfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the repo root."),
        ("intake boundary", "Free-form human/LLM language enters through Inbox first: `loom door \"<utterance>\"` captures an InboxItem and serves routing context; `loom inbox triage --take 20` normalizes cards into exact proposed commands. Use inbox kind for semantic shape (decision_capture, constraint, acceptance_criterion, interface_gap, evidence, risk, follow_up, duplicate_candidate, docs_gap, migration_need), and route_kind for destination. Existing structured graph commands stay valid once the fields are already known."),
        ("populate derived graph", "`loom populate plan` reports schema-upgrade/brownfield backfill work plus interface-plane gaps. Run `loom populate interfaces --from-sagas` to rebuild InterfaceSurface/CALLS from existing saga specs; inspect `loom interface gaps` for surfaces without calls, boundary intents without CALLS, and CALLS missing matching VALIDATES edges. This fills graph inventory, not product-code lifecycle."),
        ("seed intents", "Read the code; add `system` → `component` → `feature` intents (lifecycle defaults to `implemented`). Link with `loom edge hierarchy <parent> <child>`. GRANULARITY CONTRACT: system = 1–3 per repo (the product's purpose), component = 5–15 (cohesive subsystems), feature = MANY and ATOMIC — independently verifiable. The test: can you write ONE falsifiable criterion for it? If the description needs an 'and' ('RBAC manages users and roles and permissions'), it's several intents — seed 'users', 'roles', 'permissions' as children instead. Too coarse is recoverable (the scattered smell routes you to split the INTENT in the graph — cheap — never to refactor the code), but seeding at the right grain avoids the churn. OPTIONAL but cheap: register a small tag vocabulary as you go (`loom vocab add <term> --why \"<covers X, NOT Y>\"`) and tag intents (`--tag`, max 3) — tags from a shared registry collide where free prose doesn't, which is how duplicated responsibility in unrelated files gets caught later. Use `--domain` for product/business facets (auth, billing); use `--layer` for architecture direction (presentation, application, storage). Once layering is clear, declare it (`loom layer order <top> … <bottom>`) — imports pointing UP that order surface as layering_violation, which no edge-level inspection catches (a recorded relationship doesn't excuse direction). Mark intents that touch the OUTSIDE world with `--boundary inbound` (exposes a surface others call — an HTTP handler, public API: a provider contract) or `--boundary outbound` (calls an external system — a client/SDK: a consumer dependency). It rides into every work item, so a later traversal knows a change here is contract-affecting, not local — and it's the seam a frontend/federated graph will depend on."),
        ("ground to code", "`loom codefile add '<glob>'` then `loom edge implement <intent> <codefile> --locator \"<symbol>\"` (the symbol AS IT APPEARS in the file — e.g. `def shorten`, `fn run`, `class Link` — `loom sync` flags it stale if it isn't found verbatim). An HTML mockup or design comp is a CONTRACT surface (source_ref it), NOT a grounding target: never `loom edge implement` a production screen intent to its mockup — only an explicit prototype/Storybook intent, whose purpose IS the artifact, may IMPLEMENTS one."),
        ("reconcile existing docs", "If the repo already had docs (README architecture, docs/, ADRs, design wikis, big explanatory comments), they are now a SECOND source of truth that drifts from the graph — collapse it through the SAME intake boundary, never by hand-copying prose into intents. Capture each durable claim as a card: `loom inbox add \"<claim>\" --source import --link file:<doc>` (the link records where it came from). Then `loom inbox triage` and `loom inbox normalize <id> …` each one — normalization IS the reconciliation: VERIFY the claim against the code, then set `--route-kind`: matches reality → `intent` (map it, ground it) · a decision/why → `note` (`loom note add --kind decision`) · a standard/norm → `quality_rule` · CONTRADICTED by the code → `ignore` (the code wins — docs rot; the dismissed card is the audit trail of doc-vs-reality) · already in the graph → `ignore` as duplicate. The inbox is the GATE that stops stale prose laundering into graph truth. Once the cards are drained the knowledge lives in the graph: regenerate the human doc FROM it with `loom wiki` and replace (or point) the old architecture doc at the generated one, so no one hand-maintains a second copy. Leave docs loom does NOT own — install, license, contributing — alone."),
        ("discover", "`loom next` repeatedly: read the code it points to, then record `loom edge explore <a> <b> ground|issue|independent …`."),
        ("fix", "`loom next --mode fix` for failing/stale edges."),
        ("coverage", "`loom coverage --summary` first — it reports file counts plus actionable symbol-gap counts without dumping full symbol/adjudication archives. Use full `loom coverage --json` only when you need per-file or per-symbol evidence. Map or `loom ignore` every unaccounted file. Use `actionable_symbol_gaps` for open symbol work; `raw_actionable_symbol_gaps` is the audit trail before current decision notes. Do NOT chase 100% raw symbol coverage."),
        ("prove", "`loom validation add …` + `loom edge validates …`, then `loom validate <intent>`. Manual/async proofs: `loom validation mark <id> --result passed|failed --evidence …` (or `--result blocked --reason …` while something external is in the way)."),
        ("prove from outside", "If the system exposes endpoints, prove the COMPOSITION from the consumer's vantage: write a saga spec (ordered chain, each step bound to its intent), `loom saga add`, use `loom saga diagnose` for failure triage without stamping, then `loom saga run` to stamp runtime evidence along the intent path; a run failure lands as a failing edge naming the broken boundary."),
        ("gate", "Encode the codebase's norms: seed the packs `loom detect` recommends (`loom rule seed iso5055` baseline; `mobile`/`web-ui`/`service`/`data`/`concurrency`/`docker` per repo kind) plus `loom rule add …` for repo-specific sticks. Then `loom next --mode quality` serves every never-measured rule×intent pair — ONE command resolves each: `loom rule verdict … --status passing|failing|independent --criterion … --evidence …` (the verdict CREATES the edge; independent = measured, doesn't apply). Measure at the highest HONEST altitude: a verdict on a component covers its descendants ONLY with --covers-descendants; otherwise it covers the component alone — drop to a leaf only where the rule has specific bite. The layer order is a norm too: if intents carry architecture layers, `loom layer order <top> … <bottom>` arms the layering audit."),
        ("audit", "`loom smells --summary` first — it reports counts by smell kind, top remedies, excellence-debt totals, and detector blind spots without dumping evidence/teaching bodies. `loom status` also carries an audit pulse with top open smell kinds and a certification roll-up. Use full `loom smells --json` only when you need to inspect a specific finding. Smells are derived suspicions the graph noticed for you: twin intents (split-brain), duplicated responsibility (tag collisions across unrelated code, with a weaker lexical fallback for under-tagged coded pairs), overlapping ownership, scatter, tangles, oversized behavioral symbols, duplicated string contracts, panic/unwrap/todo markers in behavior, undeclared coupling, layering violations (imports pointing UP the declared `loom layer order` — a recorded relationship doesn't excuse direction; adjudicate a deliberate up-dependency with a decision note on the importing intent), symbol-accountability gaps (public/risky symbols without precise ownership), vocab drift, rules never held against coded intents, happy-path-only feature groups (no sad/fallback behavior declared). OPEN findings gate Hardened/Production-ready until fixed or explicitly refuted. Excellence-debt findings (size/clone/proof-locality/metadata debt) gate the Excellent certificate: fixing or proving false-positive/deliberate-design clears them; accepting or deferring real debt keeps overall/excellence yellow. ADJUDICATE ONE FINDING AT A TIME, after reading ITS code — a decision note is audit trail, not a fix: it must name the decomposition you considered and the concrete reason it is wrong for THIS finding, in terms true only of it. A ruling that restates the size/shape ('size reflects N cases', 'one cohesive module') is not an inspection; identical rationales across findings are rubber-stamping. loom now REJECTS a smell ruling that is vacuous or reuses the wording of one you recorded on another finding (`loom note add --smell` bounces it), and `loom doctor` flags templated clusters already on the graph — so audit each finding on its own merits, or split the code. Batch-stamping every finding to clear the gate is the failure mode this guards against."),
        ("close out", "`loom next --all` — every lane's remainder as one prioritized list. Then `loom export --check` before committing, so the graph travels with the repo."),
    ]
}

fn greenfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the (empty/new) repo root."),
        ("design as planned intents", "INTERVIEW the user first (`loom guide --mode seed` for the full grill technique): one question at a time, always with a recommended answer, calibrating altitude — start at SYSTEM (\"what is this product?\"), descend to FEATURE only when confident. Capture each answer through the inbox (`loom door \"<their words>\"` → `loom inbox normalize`), then land it as an intent: `loom intent add … --level system|component|feature --lifecycle planned`. Each feature's criterion IS its acceptance contract — so features must be ATOMIC (one falsifiable criterion each; a description needing 'and' is several intents). Counts: system 1–3, component 5–15, features many. Use `--aspect happy|sad|fallback` so error paths are designed in. Terminate when the graph has no open gaps — not when conversation peters out."),
        ("capture architecture", "Relate intents: `loom edge hierarchy` for structure, `loom edge explore … ground` for contracts between components. If the design is layered, declare it up front: give intents `--layer` labels and `loom layer order <top> … <bottom>` — the build is then continuously audited for imports pointing up the order (layering_violation). Use `--domain` separately for product/business facets."),
        ("build", "`loom next --mode build` → for each planned LEAF intent: write the code, `loom codefile add`, `loom edge implement`, then `loom intent mark <id> --lifecycle implemented`. Parents are deferred until their children are done, then surface as a roll-up. The criterion you wrote is your test."),
        ("verify", "Once built, `loom next` (discovery) and `loom validate` confirm reality matches the design. For endpoint-exposing designs, add a consumer saga per journey (`loom saga add`, `loom saga diagnose` while triaging, `loom saga run` to stamp) — the design's composition is proven by execution, not just per-leaf tests."),
        ("gate", "Set the quality bar: seed the packs `loom detect` recommends (`loom rule seed <pack>`) + `loom rule add …` for repo-specific sticks, then earn green with `loom next --mode quality` + `loom rule verdict` (the verdict creates the edge; a component verdict covers descendants ONLY with --covers-descendants)."),
        ("hand back to the user", "When `loom next` shows every AUTONOMOUS lane empty but USER-GATED work remains — aesthetic/manual-check confirms, align drift, hypothesis rulings, blocked proofs — CALL `loom session`. It surfaces exactly that batch, ranked by the scarcest resource (the user's presence). Draining the autonomous queues is not 'done' while a user-gated pass is still owed; `loom session` is how you collect it the next time the user is here instead of leaving it invisible."),
    ]
}

fn refactor() -> Vec<(&'static str, &'static str)> {
    vec![
        ("map first if needed", "If the area isn't in the graph yet, do the brownfield steps for it."),
        ("find the problems", "`loom smells` — the graph surfaces split-brain twins, overlapping ownership, scatter, tangles, oversized behavior, duplicated string contracts, panic/unwrap/todo markers, undeclared coupling, layering violations (imports against the declared `loom layer order`), recurrent trouble, advisory clones/co-change/shotgun surgery/proof-locality drift, and unmeasured quality rules; each finding carries its remedy command."),
        ("propose & prove redesigns", "Anything redesign-shaped (recurring breakage, a file split, a merge of twins) goes through the HYPOTHESIS PLANE before it becomes work: `loom hypothesis add --claim … --proposal … --predicted-outcome … --target <intent>` (the redesign smells emit this for you), then a DIFFERENT agent proves it (`loom next --mode prove` → `loom hypothesis prove`), then `loom hypothesis adopt --spawned <planned-intent>…` — the predicted outcome becomes a proof on the spawned work, and the hypothesis is `confirmed` only when that proof later passes. Unproven ideas die honestly (`loom hypothesis reject --reason …`) instead of becoming speculative refactors. SOLO (one agent)? The 'different agent' is you switching hats: propose, then re-inspect to PROVE with a fresh hypothesis FIRST — proposer≠prover is enforced as DISCIPLINE here, not structure, and `loom doctor` audits the provenance either way (it flags an all-solo proof chain), so the separation stays honest without a second process."),
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
        ("establish target idioms ONCE", "Before realizing leaf-by-leaf, fix the TARGET language's naming/idiom conventions up front so independently-built intents don't drift into N dialects: 'authentication' may be a module in Python and a trait+impl in Rust, but it must read the SAME way across every ported intent. Register the idioms as vocab in the target graph (`loom vocab add <term> --why \"<the target idiom this maps to>\"`) and tag ported intents as you realize them (`loom intent tag add <id> <term>`). The vocab_drift and duplicated_responsibility smells then catch the same concept realized two different ways across files — drift that per-intent isolation, realizing each leaf alone, structurally cannot see."),
        ("re-realize", "`loom next --mode build` walks the design leaf-by-leaf in dependency order: write the code in the new language (in the idioms you just fixed), `loom codefile add`, `loom edge implement <intent> <file> --locator …`, then `loom intent mark <id> --lifecycle implemented`. The criterion written for the old code is the acceptance test for the new — if it can't be met in the new language, that's a real design decision: record it (`loom note add --kind decision`) and update the intent, never silently diverge."),
        ("re-prove", "Each validation's command is a SPEC from the old toolchain — re-express it (`loom validation update <name> --command \"<new-toolchain equivalent>\"`; the reset-to-not_run is the point), then `loom validate <intent>`. Saga specs are the exception that travels VERBATIM: they speak HTTP, not the implementation language — copy the YAML across, `loom saga add` it, and the old consumer journey becomes the new code's first end-to-end acceptance test. Re-earn quality green per `loom next --mode quality` (the packs apply to the new language exactly as the old)."),
        ("verify the seams", "`loom next` (discovery) on the ported pairs: the criteria still describe how intents coexist — confirm the NEW code honors each, or record the divergence as an issue. Parity is measured per criterion, not vibes."),
        ("close out", "`loom next --all` until only optional discovery remains; `loom coverage` for unaccounted files (new-repo scaffolding may need `loom ignore add … --reason`); `loom export --check` before committing the new graph."),
    ]
}
fn seed() -> Vec<(&'static str, &'static str)> {
    vec![
        ("why this mode", "`loom sync` catches intent↔code drift mechanically; THIS mode catches user↔intent drift. A graph can be green and still describe a product the user no longer wants. Two loops share it: ELICIT (zero/few intents — capture the user's head from nothing) and ALIGN (populated graph — `loom next --mode align` serves meanings to re-affirm). Pick by graph state: sparse graph → elicit; otherwise align. The ELICIT loop walks ONE ladder — want → contract → logic → physical — so question ORDER populates the intent spectrum implicitly, with no new node subtype: the rungs map onto the altitude + aspect + visibility + boundary you already have."),
        ("calibrate altitude", "Start at SYSTEM altitude: ask \"what is this product, in one sentence?\" and land the answer with `loom intent add … --level system --lifecycle planned`. Descend only while answers stay confident. Fluent user → grill at FEATURE level for falsifiable criteria and `loom intent add … --level feature`. Vague user → stay at system/component, PROPOSE candidate features with a recommended answer, then let them react. NEVER ask a vague user to enumerate features cold."),
        ("seed ladder: want to contract to logic to physical", "Elicit in four ordered stages, ONE stage and one question at a time, each landing a concrete graph object so the spectrum fills as you go. WANT — the user-visible capability and WHO it's for → `loom intent add --level system|component --lifecycle planned --visibility user_visible` plus `loom persona add` for the audience (and `--boundary inbound|outbound` where it crosses the edge). CONTRACT — how you'll know it's delivered → an acceptance proof `loom validation add --type manual_check --intent <want>` (see 'stub the acceptance proof'). LOGIC — the internal machinery that delivers it → child intents `loom intent add --level feature --lifecycle planned --visibility internal` under the want (`loom edge hierarchy`), error/degraded paths as `--aspect sad|fallback` siblings. PHYSICAL — deferred until code exists: the bottom rung is `loom edge implement` via `loom next --mode build`. VISIBILITY is captured AS each rung lands — user_visible at the want stage, internal at the logic stage — so every intent is born with its audience set, never left unset for the align interview to triage later."),
        ("one question, one inbox card, one landing", "Ask ONE question at a time, always with your recommended answer. If code can answer it, switch to brownfield and explore instead of asking (`loom guide --mode brownfield`). The moment an answer crystallises, CAPTURE it first (`loom door \"<their words>\"` or `loom inbox add \"…\"`), normalize the card (`loom inbox normalize …`), then run the proposed graph command. Behavior → `loom intent add … --lifecycle planned`; term → `loom vocab add`; hard tradeoff → `loom note add --kind decision`; error path → same tree with `--aspect sad|fallback`. Atomic only: if an intent description needs 'and', split it."),
        ("stub the acceptance proof", "Make every WANT falsifiable by default. As a behavior lands at the contract/logic rung, stub its acceptance proof: `loom validation add --type manual_check --name \"<intent> — acceptance\" --intent <intent-id>` writes a not_run manual_check Validation VALIDATES-linked to the intent — the same shape a hypothesis adoption writes for its predicted outcome. The intent now carries an unmet proof: it surfaces in `loom next --mode validate` until someone marks it passed with real evidence (`loom validation mark … --result passed --evidence …`), so a seeded want is provable-or-pending by construction, never silently unverifiable. Refine the stub into a runnable test (`loom validation update … --command …`) once the code exists."),
        ("challenge, don't transcribe", "When the user's term collides with registered vocab, call it out and resolve with `loom vocab add` or a decision note (`loom note add --kind decision`). When a claim contradicts existing intents or code, surface the contradiction and make the user choose. Stress-test boundaries with scenarios: \"a payment fails mid-checkout — what does the user see?\" Each answer usually lands as `loom intent add … --aspect sad|fallback --lifecycle planned`."),
        ("the visual register", "For a user_visible SCREEN the register is REACTION-driven, not interview-driven — a user reacts to a surface faster than they can specify one. Generate an HTML mockup as the reaction surface (`loom codefile add 'mockups/<screen>.html'`, source_ref it from the screen intent), show it, and convert EACH human reaction into a graph delta: a new intent, an `--aspect populated|empty|loading|error` state child (the UI-state family the happy_path_only audit reads — a populated state with no empty/error sibling is flagged), a `loom vocab add` term, or a `loom note add --kind decision`. Then regenerate the mockup and capture the next reaction. LOOP until reactions stop changing the graph's structure — convergence, not exhaustion, is the stop. Verify MACHINE-FIRST where a machine can (a rendered-DOM/assertion validation for structure and copy)."),
        ("mockup is contract, not realization", "A production screen intent source_refs its HTML mockup and STAYS `lifecycle=planned`: the mockup is what the screen must MATCH, not the screen itself, so it NEVER takes a `loom edge implement` from the production intent — grounding the design to its own spec would falsely mark it realized. Ground the production intent only to the real component code, once that exists. The one legitimate IMPLEMENTS→mockup is an EXPLICIT prototype or Storybook intent whose whole purpose IS the artifact — there the mockup is the realization. (Convention, taught here — loom does not hard-block an html grounding, because a real prototype legitimately has one.)"),
        ("visual-confirm queue", "What a machine cannot judge — the subjective \"does it actually look right?\" residue — does not vanish: capture it as `manual_check` validations with `inspected_by` = human (`loom validation add --type manual_check --intent <screen>`), so the human visual pass is a recorded proof, not lost in chat. These batch into a USER-GATED lane: `loom session` surfaces them by the scarcest resource — the user's presence — alongside align drift, hypothesis rulings, and blocked proofs, so an autonomous agent drains everything it can and leaves ONE batched aesthetic-confirm pass for when the user is actually here. The trigger is mechanical: the moment `loom next` shows the autonomous lanes empty, run `loom session` to surface this batch — don't wait to be asked."),
        ("terminate on completeness, not exhaustion", "The interview ends when the GRAPH says so, never when conversation peters out. Every question must close an enumerable gap: component with no children (`loom edge hierarchy`), feature with no criterion (`loom intent update … --description … --reason …`), happy-path-only group with no `--aspect sad|fallback`, or vocab collision (`loom vocab add`). No open gap → STOP. Explicitly declined scope lands as `loom note add --kind decision`; silence and decision must never look alike."),
        ("the align loop", "On a populated graph, `loom next --mode align` serves drift SUSPECTS only: meanings whose claims flipped since the user last confirmed them — code churn, but also a neighbour's redefinition or retirement rippling in, exactly like a changed codefile stales the claims earned against it — plus quiet wording unaffirmed past a grace period. Intents ruled `internal` are NEVER served (machinery isn't interview material) until a redefinition clears the ruling. Align the CONCEPT, not the wording: present what the product can DO because this exists (one or two plain sentences — jargon test: would a non-coder nod?), why it matters (its place in the design), and its audience UP FRONT — internal machinery presented as a product capability is how interviews go wrong. The item carries `visibility`, `where_it_sits`, and `not_to_confuse_with` (siblings + verified-independent neighbours) for exactly this. Vocabulary enters only when the user asks, stumbles, or uses a term that conflicts with the graph. Record exactly ONE outcome: concept still right → `loom intent confirm <id>`; words confusing, concept right → `loom intent update <id> --description … --reword --reason …` (no ripple, clock resets); concept evolved → translate their words BACK into a falsifiable description, `loom intent update <id> --description … --reason …`; internal machinery → `loom intent confirm <id> --visibility internal` (stops the asking until redefined); superseded → `loom intent retire <id> --reason … --replaced-by <successor>`; revealed gap → `loom intent add … --lifecycle planned`. A laundry-list meaning that needs 'and' is itself a finding — propose the split. Every outcome resets that intent's suspicion clock; then pull the queue AGAIN — it drains to empty, and empty is the stopping point: never one question, never the whole graph."),
        ("handoff", "After seeding/aligning, builder lanes take over: `loom status` routes the compass, and planned intents flow through `loom next --mode build`. Iterate code freely while intents are `planned` (nothing downstream to stale). After grounding, every meaning change uses `loom intent update … --reason …` and costs re-verification through `loom sync` — which is the point."),
    ]
}

/// IMPORT / ADOPT: bring a pattern, subsystem, or contract from ANOTHER repo
/// into this one. Not fresh greenfield, not a local refactor — the source of
/// truth lives elsewhere, so the spine is OBSERVE → CAPTURE → ROUTE → realize
/// HERE, never a blind copy. Composes existing primitives (import --as-planned,
/// init --observed, the inbox boundary, the hypothesis plane, federation).
fn import_idea() -> Vec<(&'static str, &'static str)> {
    vec![
        ("name what you're adopting", "Decide the unit, because it picks the path: a single PATTERN (one idea — 'their retry-with-jitter'), a SUBSYSTEM (a cohesive chunk you'll re-implement here), or a CONTRACT you must consume (their API/SDK). Adopting a copy and depending on a live upstream are different jobs — see the last step."),
        ("get the source into view (don't own it)", "If the source repo HAS a loom graph, adopt its INTENTS, not just its code: `loom import <their-export> --as-planned` into a SCRATCH graph (or read its committed loom.graph.json) so you import the why. If it does NOT, map only the slice you care about as OBSERVED — `loom init --observed` makes it understanding/measuring/proving-only (build & fix lanes OFF) so you never pretend to own upstream code. Either way the source stays REFERENCE, never your graph's truth."),
        ("capture through the inbox, never hand-copy", "Every claim you want to bring over enters THROUGH the intake boundary: `loom inbox add \"<the pattern/contract/decision>\" --source import --link file:<their/path>` (the link records provenance). Capture the rationale too — `loom door \"<the idea>\" --why \"<why it fits HERE>\"` — because adopting a pattern without its reasoning is exactly how cargo-culting starts. The inbox is the gate that stops someone else's prose laundering straight into your graph truth."),
        ("route each card against THIS repo", "`loom inbox triage`, then normalize each card against YOUR code and route it: a capability you'll build here → `intent` (lands `--lifecycle planned`; it's now greenfield-of-this-slice); a redesign of something you already have → `hypothesis` (PROVE it pays off here before adopting — their context isn't yours); a norm worth enforcing → `quality_rule`; a decision/why → `note`; already covered or a bad fit for this repo → `ignore` (the dismissed card is the audit trail of why you did NOT adopt it). The code you have always wins over the pattern you admire."),
        ("realize + prove HERE", "Adopted intents are ordinary planned work: `loom next --mode build`, write the code in THIS repo's idioms, ground it, and prove its criterion — the source's passing tests DO NOT transfer (their code, their toolchain). A consumed CONTRACT proves best as a consumer saga against the real upstream (`loom saga add` the YAML journey)."),
        ("live dependency? federate, don't import", "If you are not adopting a COPY but will keep DEPENDING on the other repo as it evolves, don't import — DELEGATE: `loom delegate add '<their-path-glob>' --to <their-loom.graph.json>` and link your SEAM intents (`loom delegate seam '<pattern>' <intent>`). `loom sync` then watches their committed export and re-opens your seam claims when their contract shifts. Data flows UP (they export, you observe); you never write into their graph."),
        ("close out", "`loom next --all` until the adopted slice is built and proven and only optional discovery remains; `loom export --check` before committing."),
    ]
}

/// SAGA: authoring a consumer-plane proof. A saga is not "tests for the API";
/// it is an executable journey that proves a user-visible intent path by
/// running it against the live boundary. Each step binds to an intent; a passing
/// run stamps RUNTIME evidence on the path edges.
fn saga() -> Vec<(&'static str, &'static str)> {
    vec![
        ("saga authoring is proof authoring", "A saga proves a USER-VISIBLE journey by EXECUTING it against the live boundary. The spec binds each step to an intent; a passing `loom saga run` stamps runtime evidence on the intent path."),
        ("design the journey first", "Break the user story into steps that each exercise a real intent. If a step's intent doesn't exist, `loom saga add <spec.yaml> --spawn-missing [--under <parent>]` creates it as planned; `loom next --mode build` later realizes it and the saga becomes its acceptance test."),
        ("write the spec", "YAML: `saga: <name>` and a list of `steps` with `name`, `intent`, `request { method, url }`, and `expect { status, body? }`. Capture values from responses with JSONPath and thread them into later steps. Run `loom schema` for the full spec shape."),
        ("add the saga", "`loom saga add <spec.yaml>` registers the Validation, links VALIDATES and path RELATES_TO edges, records interface CALLS, and warns if a step hits a trivial/health-check endpoint unrelated to its intent."),
        ("diagnose before stamping", "`loom saga diagnose <name>` runs the chain WITHOUT stamping proof. Use it to triangulate env/base-url/handler problems. Missing required env is `blocked`, not `failed`."),
        ("stamp proof", "`BASE_URL=<url> loom saga run <name>` executes the journey and stamps PASSING or FAILING evidence. Only a passing run can prove a user-visible boundary intent — a forged Proven rung is a honesty bug."),
        ("rerun when the boundary changes", "`loom sync` re-opens saga proofs when registered files change. Re-run `loom saga run` to re-earn the Proven rung after edits."),
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
            "import" | "adopt" => Ok("import"),
            "saga" => Ok("saga"),
            other => anyhow::bail!(
                "Unknown mode '{}'. Valid: greenfield, brownfield, refactor, port, seed, import, saga",
                other
            ),
        };
    }
    // Seed is explicit-only: this is a user-in-the-loop session, and the binary cannot detect "the user wants to talk".
    // Auto-detect from the repo: no source on disk → greenfield, else brownfield.
    let cwd = crate::db::resolve_root()?;
    Ok(if crate::repo::detect(&cwd)?.has_source {
        "brownfield"
    } else {
        "greenfield"
    })
}

/// `loom guide --role <role>` — the role CHARGE. Derived ENTIRELY from the lane
/// table (`gate::actions_for_role` for the lane, `gate::mode_for_role` for the
/// queue) plus the role mandate, so it can never contradict what `gate.rs`
/// actually enforces. This is how an LLM adopts a lane from loom itself, in any
/// harness — no role-specific markdown to ship or drift. Opens no graph: an
/// agent adopts its role before it touches anything.
fn run_role_charge(role: &str, printer: &Printer) -> Result<()> {
    use crate::db::schema::ROLES;
    if !ROLES.contains(&role) {
        anyhow::bail!(
            "Unknown role '{role}'. Valid roles: {}. \
             Each owns a lane — `loom guide --role <role>` prints its charge.",
            ROLES.join(", "),
        );
    }
    let mandate = ROLE_LANES
        .iter()
        .find(|(r, _, _)| *r == role)
        .map(|(_, _, what)| *what)
        .unwrap_or("");
    let queue = crate::gate::mode_for_role(role)
        .map(|m| format!("loom next --mode {m}"))
        .unwrap_or_else(|| "loom next".to_string());
    let lane: Vec<&str> = crate::gate::actions_for_role(role)
        .iter()
        .map(|l| l.action)
        .collect();
    let setup = role_setup(role);
    let out_of_lane = "Acting outside the lane is a hard error naming the owner. \
        Hand off via `loom note add --for <role>`; bare `llm`/`human` = solo mode (all lanes).";
    let skill = role_skill_name(role);
    let (description, anchor, discipline) = role_discipline(role).unwrap_or(("", "", &[]));
    let (autonomy_mode, autonomy_doc) = autonomy_guidance();

    if printer.json {
        printer.print_json(&serde_json::json!({
            "skill": skill,
            "description": description,
            "role": role,
            "mandate": mandate,
            "setup": setup,
            "queue": queue,
            "lane": lane,
            "anchor": anchor,
            "discipline": discipline,
            "operating_mode": {
                "autonomy": autonomy_mode,
                "guidance": autonomy_doc,
                "set_with": AUTONOMY_SET_WITH,
            },
            "out_of_lane": out_of_lane,
            // The binary IS the skill server: this charge is the complete,
            // self-contained `loom-<role>` skill, served JIT — no install needed.
            // `loom skill install` can persist it as a harness skill (opt-in).
            "adopt": format!("Adopt the {skill} discipline below, then: {setup} && {queue}"),
            "next_step": format!("{setup} && {queue}"),
        }));
        return Ok(());
    }

    println!(
        "══ loom — adopt the {} skill ══════════════════════════════════",
        skill.to_uppercase()
    );
    println!();
    println!(
        "You are now operating loom's {role} lane. ADOPT this discipline for the lane's work."
    );
    println!("(This is the complete {skill} skill, served by the binary — no install. `loom skill install` to pin it.)");
    println!();
    if !anchor.is_empty() {
        println!("  THE LAW   {anchor}");
    }
    println!("  MANDATE   {mandate}");
    println!("  SETUP     {setup}");
    println!("  QUEUE     {queue}");
    println!();
    println!("CORE REFLEX (every turn): `loom status` → `loom next` → do the work → `loom sync` after ANY code change.");
    println!("  Full golden rules + ripple + playbook: `loom guide --all`.");
    println!();
    println!("  AUTONOMY ({autonomy_mode}): {autonomy_doc}");
    println!();
    for action in &lane {
        println!("  • {action}");
    }
    if !discipline.is_empty() {
        println!();
        println!("DISCIPLINE (how this lane works — the part that makes the verdicts honest):");
        for line in discipline {
            println!("  • {line}");
        }
    }
    println!();
    println!("{out_of_lane}");
    println!("Full driving protocol: `loom guide --all`.");
    println!();
    if !anchor.is_empty() {
        println!("  ⟐ Remember: {anchor}");
    }
    println!("  → Next: {setup} && {queue}");
    Ok(())
}

/// Bare `loom guide`'s JIT target: the role whose lane-skill serves the focus
/// rung. `None` when there is no graph yet (fresh repo → manual), every rung is
/// cleared (no focus), or the focus lane has no authored skill (triage/audit) —
/// the caller then falls back to the full driving protocol. Builds the ladder
/// the same way `loom next` routes, so `guide` and `next` agree on the lane.
fn focus_lane_role() -> Option<&'static str> {
    let root = crate::db::resolve_root().ok()?;
    let store = GraphReadHandle::open(&root).ok()?;
    let snap = store.query_snapshot().ok()?;
    let gs = store.graph_state(&snap).ok()?;
    let decision_notes = store.notes_by_kind("decision").ok()?;
    let (open_smells, excellence_debt_count) = if matches!(gs.phase.as_str(), "audit" | "complete")
    {
        let report = store.smell_report(&snap).ok()?;
        let excellence_debt_count = report.advisory.len() + report.debt.len();
        (report.open, excellence_debt_count)
    } else {
        (Vec::new(), 0)
    };
    let inbox_items = store.list_inbox_items(None, None).ok()?;
    let inbox_untriaged = inbox_items.iter().filter(|i| i.status == "new").count();
    let export_stale = store.committed_export_stale(&root).ok().flatten() == Some(true);
    let lane = crate::db::queries::build_ladder(
        &root,
        &snap,
        &gs,
        &decision_notes,
        &inbox_items,
        &open_smells,
        excellence_debt_count,
        inbox_untriaged,
        export_stale,
    )
    .ladder
    .focus_lane()?;
    lane_to_role(lane)
}

/// The graph's autonomy mode + how it changes the driving cadence. Read from the
/// live meta sentinel (best-effort: a fresh/unopenable graph reads as the
/// cautious `guided` default). This is the ONE place the protocol tells the
/// driver how much it may drive without pausing — `init --autonomy` sets it.
fn autonomy_guidance() -> (&'static str, &'static str) {
    let autonomous = (|| {
        let root = crate::db::resolve_root().ok()?;
        let store = GraphReadHandle::open(&root).ok()?;
        let snap = store.query_snapshot().ok()?;
        Some(store.graph_state(&snap).ok()?.autonomy == "autonomous")
    })()
    .unwrap_or(false);
    if autonomous {
        (
            "autonomous",
            "Drain every AUTONOMOUS lane without pausing for per-step user confirmation; \
             escalate to the user ONLY for genuine ambiguity and USER-GATED work (align drift, \
             hypothesis rulings, manual-check confirms, blocked proofs) — batch those via \
             `loom session`. This is an interrupt budget, NOT a license to skip inspection: \
             smell/decision rulings are still earned per-finding (the write gate rejects vacuous \
             or templated rulings), and excellence-debt findings are still never auto-fixed.",
        )
    } else {
        (
            "guided",
            "Interrupt-by-default: surface a confirmation beat at each lane edge and route more \
             decisions back to the user. Lead with `loom session` when user-gated work exists. \
             Flip to hands-off with `loom init --autonomy autonomous` once the user trusts the loop.",
        )
    }
}

/// Invert the enforced lane table (its `mode` field IS the lane) → the role that
/// owns it. Single-sourced from ROLE_LANES so it can't drift from the gate.
fn lane_to_role(lane: &str) -> Option<&'static str> {
    ROLE_LANES
        .iter()
        .find(|(_, mode, _)| *mode == lane)
        .map(|(role, _, _)| *role)
}

pub fn run(mode: Option<&str>, role: Option<&str>, all: bool, printer: &Printer) -> Result<()> {
    if let Some(r) = role {
        return run_role_charge(r, printer);
    }
    // Bare `loom guide` is FOCUS-SCOPED (JIT): serve the skill for the focus
    // rung's lane so the entry point answers "how do I do THIS rung" instead of
    // the full firehose. `--all` (or an explicit `--mode`) gives the full manual.
    // No graph (fresh repo) / all-green / a lane with no authored skill
    // (triage/audit) → fall through to the driving protocol below.
    if mode.is_none() && !all {
        if let Some(role) = focus_lane_role() {
            // Say WHY this lane (a cold reader didn't ask for it) — and how to
            // get the map and the full manual. JSON carries `role`, so it's self-
            // evident there; the preamble is human-only.
            if !printer.json {
                println!("(loom routed you here — the focus rung's lane. `loom status` for the map · `loom guide --all` for the full protocol.)");
                println!();
            }
            return run_role_charge(role, printer);
        }
    }
    let m = resolve_mode(mode)?;
    let steps = match m {
        "greenfield" => greenfield(),
        "refactor" => refactor(),
        "port" => port(),
        "seed" => seed(),
        "import" => import_idea(),
        "saga" => saga(),
        _ => brownfield(),
    };

    let (autonomy_mode, autonomy_doc) = autonomy_guidance();

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode": m,
            "operating_mode": {
                "autonomy": autonomy_mode,
                "guidance": autonomy_doc,
                "set_with": AUTONOMY_SET_WITH,
            },
            "what_is_loom": "Externalized, falsifiable memory for understanding, building, and cleaning up a codebase. \
                A living graph of intents (what code should do), grounded in real files, every relationship carrying a \
                verification status + evidence. The graph is durable memory; the context window is the working set.",
            "planes": {
                "semantic": "Intent — what the system should do",
                "physical": "CodeFile — what exists on disk",
                "normative": "QualityRule — what good looks like",
            },
            "lifecycle": {
                "summary": LIFECYCLE_SUMMARY,
                "active_states": LIFECYCLE_STATES.iter().map(|(state, meaning, driver)| serde_json::json!({
                    "state": state, "meaning": meaning, "driver": driver,
                })).collect::<Vec<_>>(),
                "transitions": LIFECYCLE_TRANSITIONS.iter().map(|(name, transition, command)| serde_json::json!({
                    "name": name, "transition": transition, "command": command,
                })).collect::<Vec<_>>(),
                "related_status_families": RELATED_STATUS_FAMILIES.iter().map(|(family, meaning)| serde_json::json!({
                    "family": family, "meaning": meaning,
                })).collect::<Vec<_>>(),
            },
            "architecture_metadata": ARCHITECTURE_METADATA_GUIDANCE,
            "steps": steps.iter().map(|(t, d)| serde_json::json!({"step": t, "do": d})).collect::<Vec<_>>(),
            "golden_rules": CORE_RULES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "deeper_rules": DEEPER_RULES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "ripple": {
                "when": "Run `loom sync` after ANY code change — it detects mtime deltas on registered files and propagates the impact one hop. The graph structure IS the impact analysis.",
                "what_goes_stale": RIPPLE,
            },
            "roles": {
                "how": "ADOPT the lane as a skill — `loom guide --role <role>` hands you its discipline; \
                    its SETUP declares your role `LOOM_AGENT=llm:<role>` (or per-command --inspected-by/--author). \
                    Declared roles are ENFORCED at the WRITE BOUNDARY — acting outside your lane is an error \
                    pointing you back to your own queue. Bare 'llm'/'human' = solo mode (one context drives \
                    every lane). Separation of duties: the builder cannot green-light its own work; verdicts \
                    (ground/issue/independent, confirm, validate, rule verdict) belong to other lanes. \
                    `loom doctor` audits provenance after the fact.",
                "lanes": ROLE_LANES.iter().map(|(role, mode, what)| serde_json::json!({
                    "role": role, "queue": format!("loom next --mode {mode}"), "does": what,
                })).collect::<Vec<_>>(),
                "adopt": "`loom guide --role <role>` prints one lane's full charge — mandate, lane (what it MAY do), queue, setup — derived from the enforced lane table so it can't drift. An agent adopts its role from loom itself, no harness-specific instructions.",
            },
            "orchestration": {
                "principle": "ADOPT THE LANE, don't spawn one: loom serves each lane as a SKILL just-in-time (`loom guide --role <role>` hands you the discipline to adopt IN CONTEXT — no install). loom defines the CONTRACT (roles, lanes, owned fields, the handoff dependency); it does NOT predefine the TOPOLOGY. The default is one context adopting lane-skills as the compass routes; spawning a sub-context is an OPTIONAL scale-out for genuinely bulk queues.",
                "topologies": [
                    "DEFAULT — one context, adopt the lane-skill the compass names, switch skills as the phase changes (warm context, no handoff cost)",
                    "one context declaring a role per phase (set LOOM_AGENT, work that lane, switch) — sequential hat-switching",
                    "SCALE OUT (bulk only) — spawn a fresh sub-context for one lane (a cheap model floods thousands of grid edges in isolation); sequential or parallel; handoff via the graph",
                ],
                "concurrency": "loom DOES cross-process-lock writes: an advisory flock on `.loom/graph.lock` taken per write transaction, so at most one write session touches a graph at a time (readers run concurrently under WAL). A competing writer waits up to LOOM_LOCK_DEADLINE_MS (default 5000ms), then fails with a NAMED error ('graph write lock is held by another loom session … loom serializes writers'), never a raw 'database is locked'. Parallel lane work is therefore safe against corruption on one graph — loom serializes it for you. For heavy parallel WRITE fan-out, handle that named lock-wait error (retry/backoff) or give each writer its own graph clone + merge later. Sequential shapes never contend; reads never block.",
                "contract": "Identical whether you adopt in-context or spawn a sub-context: declare your role `LOOM_AGENT=llm:<role>` (the lane-skill's SETUP line does this; or stay bare `llm` for solo); stay in your lane; fill ONLY your owned fields (`loom schema`); `loom note` anything out of lane; hand off through the GRAPH (status/next/notes), not chat.",
                "handoff_order": "A DEPENDENCY, not a schedule: builder (construct + ground + populate derived structure) → analyzer (verify) → validator (prove) → quality (green); fixer on any failing/needs_change. Run sequentially or overlap where the graph allows.",
                "separation_of_duties": "Enforced at the WRITE BOUNDARY on the LOOM_AGENT role string, NOT on process identity: a spawned context per role and one context switching lane-skills are held to the SAME gate. Distinct contexts make separation structural; one context switching skills makes it discipline — `loom doctor` audits provenance either way.",
                "loop": "`loom status` → read the maturity ladder + focus (the lowest unmet rung) → whoever owns that rung's lane acts (`loom next` names the role + fields per item) → repeat until the ladder reaches the target rung (Production-ready for a full close).",
                "performance": PERFORMANCE_GUIDANCE,
            },
            "consumer_plane": {
                "what": "Runtime proof of COMPOSITION: a saga is an ordered chain of endpoint invocations run the way a real consumer will (captures thread one response into the next request). Everything else grounds claims by reading code; a saga stamps the RELATES_TO path between its step intents with EXECUTION evidence.",
                "loop": [
                    "write a YAML spec — every step binds to the intent it proves (`intent:` is first-class); optionally add `auth.requires_scopes` so diagnosis can compare bearer JWT scopes against endpoint requirements; see `loom saga add --help` for the format",
                    "declare (builder|validator): `loom saga add <spec.yaml>` — Validation (type=saga) + VALIDATES edges + the uninspected RELATES_TO path + the spec as a CodeFile",
                    "start the system under test the way THIS repo provisions it (scan for docker-compose / a Makefile/justfile target / scripts/ / package.json scripts / the README — loom ships no mock, it drives the real composition), then diagnose (validator): `BASE_URL=<live target> loom saga diagnose <name>` for root-cause triage without stamping; when ready, run `BASE_URL=<live target> loom saga run <name>` to stamp proof — `{{ env.X }}` values are passed AT INVOCATION, never stored in the graph; `loom saga list` shows each saga's exact `run with:` line",
                    "outcome stamping: consecutive passing steps stamp passing with runtime evidence; the failing boundary stamps failing with the broken expectation ('expected 200, got 502'); never-reached steps stay untouched; a MISSING env value refuses to run with nothing stamped (environment-not-ready ≠ failed proof — `loom validate` records it as `blocked`)",
                    "staleness: `loom sync` flips the saga's proof to not_run when step-intent code changes — the validate queue re-serves it",
                ],
                "honesty": "Exits non-zero on failure (works under `loom validate`/CI). Deliberately a saga executor, not a general HTTP test tool — anything fancier is an ordinary command-based Validation.",
            },
            "hypothesis_plane": {
                "what": "The PRE-DECISION plane: an improvement idea is not work until proven. Hypothesis = falsifiable claim (what's wrong NOW) + proposal (the change) + predicted_outcome (measurable result). State machine: proposed → supported|refuted → adopted → confirmed | rejected.",
                "loop": [
                    "propose (any lane): `loom hypothesis add --name … --claim … --proposal … --predicted-outcome … [--target <intent>]…` — the redesign-shaped smells emit this as their remedy",
                    "prove (analyzer, a DIFFERENT agent): `loom next --mode prove` ranks proposals by target blast radius → `loom hypothesis prove <id> --verdict supported|refuted --evidence … --confidence 0.9` (stamps the TARGETS edges)",
                    "decide (builder): `loom hypothesis adopt <id> --spawned <planned-intent>…` — converts into ordinary build work AND writes the predicted outcome as a not_run Validation on the spawned intents; or `loom hypothesis reject <id> --reason …`",
                    "confirm (validator): when the outcome validation is marked passed, the hypothesis derives `confirmed` — adopted improvements are checked for whether they DELIVERED",
                    "staleness: `loom sync` flips hypothesis support when target code changes; the prove queue re-serves it as a RE-PROVE item",
                ],
                "honesty": "Speculation never counts in coverage/completeness — proving is optional like discovery/review; proposer ≠ prover when roles are declared.",
            },
            "completeness": {
                "vertical": "BINDING spine, mechanically verifiable: HIERARCHY is a well-formed tree (one parent per non-root intent, no cycles); every implemented leaf intent has ≥1 IMPLEMENTS (realized); every CodeFile is reached by ≥1 IMPLEMENTS. Surfaced as `vertically_complete` in `loom status`; details in `loom report` + `loom doctor` + `loom coverage`.",
                "horizontal": "Feeds the HARDENED rung (not the vertical spine): every explicit RELATES_TO edge is inspected/current, and every signal-bearing unexplored pair is adjudicated. Surfaced as `horizontally_explored`. `loom edge unexplored --class suspected-coupling` lists required risk pairs; `--class all` remains the optional exhaustive survey.",
            },
            "done_condition": "The MATURITY LADDER is now a certification vector, not just a completion scalar (`loom status` / `loom complete` → `maturity.rungs`) with a FOCUS = the lowest unmet rung, where `loom next` routes. Ordered by RECORD ≠ DISCHARGE: SEEDED (every responsibility captured; entrypoint owned, inbox triaged) → REALIZED (every leaf grounded + proven by an EXECUTED discriminating test, `proven_executed == realized`, no doc-only spec-as-built) → PROVEN (every user_visible journey has a passing boundary proof — saga or human manual_check; N/A and collapses when there are no journeys) → HARDENED (measured under rules, RELATES_TO risk closed, failure-path siblings realized, ZERO open `loom smells` findings) → PRODUCTION-READY (all lower rungs cleared + wiki fresh, inbox drained, boundary owned — deploy-fitness) → EXCELLENT (Production-ready plus zero unresolved excellence debt: refactor/design/proof-locality debt is fixed, false-positive, or deliberate design; accepted/deferred real debt keeps overall yellow). COMPREHENSION AXIS (parallel, routing-gated behind Production-ready): `loom next --mode wiki` drains the code-primary wiki prose queue — author narrative prose citing source files (not intent UUIDs); `loom wiki --prose-check` certifies coverage + freshness + consistency gates green. It is a VECTOR, never a scalar: a graph can be Production-ready while Excellent is still ◐, so read every rung, then drive the focus.",
            "output_hygiene": {
                "rule": "High-volume audit commands have summary mode. Start with `loom smells --summary --json` and `loom coverage --summary --json`; only request full JSON when a specific finding/gap needs evidence.",
                "why": "`loom smells --json` includes per-finding evidence, teaching, adjudicated rulings, and advisory bodies; `loom coverage --json` includes full file/symbol/raw-gap/adjudication archives. Summary mode preserves routing facts without blowing the driver context."
            },
        }));
        return Ok(());
    }

    println!(
        "══ loom — driving guide  [mode: {}] ═════════════════════════════════",
        m
    );
    println!();
    // ── CORE: what you need to start driving right now ─────────────────────
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
    println!("THE LOOP (the reflex — every turn)");
    println!("  1. `loom status`     → read the maturity ladder + focus rung + alarms");
    println!("  2. `loom next`        → the compass names the lane + the exact next item");
    println!("  3. do the work        → read code, record evidence, write/run the proof");
    println!("  4. `loom sync`        → after ANY code change (the flag engine — see RIPPLE)");
    println!("  Repeat until the maturity ladder reaches your target rung.");
    println!();
    println!("GOLDEN RULES");
    for r in CORE_RULES {
        println!("  • {}", r);
    }
    println!();
    println!("OPERATING MODE (autonomy: {autonomy_mode} — set with `loom init --autonomy <mode>`)");
    println!("  {autonomy_doc}");
    println!();
    // ── DEEPER: the detail you reach for when the core isn't enough ─────────
    println!("── DEEPER ────────────────────────────────────────────────────────────");
    println!();
    println!("LIFECYCLE");
    println!("  {}", LIFECYCLE_SUMMARY);
    println!("  Active states:");
    for (state, meaning, driver) in LIFECYCLE_STATES {
        println!("  - {state}: {meaning}");
        println!("    {driver}");
    }
    println!("  Transitions:");
    for (name, transition, command) in LIFECYCLE_TRANSITIONS {
        println!("  - {name}: {transition}");
        println!("    {command}");
    }
    println!("  Related status families:");
    for (family, meaning) in RELATED_STATUS_FAMILIES {
        println!("  - {family}: {meaning}");
    }
    println!();
    println!("ARCHITECTURE METADATA");
    println!("  {}", ARCHITECTURE_METADATA_GUIDANCE);
    println!();
    println!(
        "PLAYBOOK ({} — {})",
        m,
        match m {
            "greenfield" => "design first, then build",
            "refactor" => "change existing code with intent",
            "port" => "re-realize a mapped system in a new language/repo",
            "seed" =>
                "capture & re-align the user's head — interview, land, terminate on completeness",
            _ => "map & verify existing code",
        }
    );
    for (i, (title, doc)) in steps.iter().enumerate() {
        println!("  {}. {}", i + 1, title);
        println!("       {}", doc);
    }
    println!();
    println!("DEEPER RULES (reference — when the core isn't enough)");
    for r in DEEPER_RULES {
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
    println!("  Adopt one: `loom guide --role <role>` prints that lane's full charge");
    println!("  (mandate, lane, queue, setup) — derived from the enforced lane table.");
    println!();
    println!(
        "ORCHESTRATION (you have loom access — usually an orchestrator that can spawn subagents)"
    );
    for line in ORCHESTRATION {
        println!("  {}", line);
    }
    // Single-sourced with json `orchestration.performance` (see PERFORMANCE_GUIDANCE).
    println!("  PERFORMANCE — {}", PERFORMANCE_GUIDANCE);
    println!();
    println!("Other modes: `loom guide --mode greenfield|brownfield|refactor|port|seed|import`. Start: `loom status` · `loom next`.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Printer;

    fn guide_json(mode: &str) -> serde_json::Value {
        let p = Printer::capturing(true);
        run(Some(mode), None, false, &p).expect("guide opens no graph — must never fail");
        serde_json::from_str(&p.captured().expect("captured json"))
            .expect("guide --json is valid json")
    }

    fn charge_json(role: &str) -> serde_json::Value {
        let p = Printer::capturing(true);
        run(None, Some(role), false, &p).expect("charge opens no graph — must never fail");
        serde_json::from_str(&p.captured().expect("captured json"))
            .expect("charge --json is valid json")
    }

    /// The charge is a pure VIEW on the enforced lane table: its lane list must
    /// equal `gate::actions_for_role` and its queue `gate::mode_for_role`, for
    /// every role — so it can never tell an agent it may do something the gate
    /// will then reject (or hide something the gate allows).
    #[test]
    fn role_charge_is_derived_from_the_lane_table() {
        for role in crate::db::schema::ROLES {
            let v = charge_json(role);
            assert_eq!(v["role"], serde_json::json!(role), "role echoed");
            let mode = crate::gate::mode_for_role(role).expect("agent role has a queue");
            assert_eq!(
                v["queue"],
                serde_json::json!(format!("loom next --mode {mode}")),
                "queue must match gate::mode_for_role for '{role}'"
            );
            let want: Vec<&str> = crate::gate::actions_for_role(role)
                .iter()
                .map(|l| l.action)
                .collect();
            let got: Vec<String> = v["lane"]
                .as_array()
                .expect("lane is an array")
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect();
            assert_eq!(
                got, want,
                "charge lane for '{role}' must equal the gate table"
            );
            assert!(!want.is_empty(), "every role owns ≥1 lane: '{role}'");
            assert_eq!(
                v["setup"],
                serde_json::json!(role_setup(role)),
                "setup names the role"
            );
        }
    }

    /// JIT SKILL SERVING: `loom guide --role X` is the complete, self-contained
    /// `loom-<role>` skill the binary serves on demand — skill name + JIT-trigger
    /// description + the lane's working discipline — framed as adoption, no
    /// install. This is the binary AS the skill server.
    #[test]
    fn role_charge_is_a_full_adoptable_skill() {
        let v = charge_json("analyzer");
        assert_eq!(v["skill"], serde_json::json!("loom-analyzer"), "skill name");
        assert!(
            v["description"]
                .as_str()
                .unwrap_or("")
                .contains("Adopt when"),
            "the description is a JIT adoption trigger: {v}"
        );
        // The honesty law is ELEVATED to its own anchor (the craft move), not
        // buried in a bullet — it leads the skill and closes it.
        assert!(
            v["anchor"]
                .as_str()
                .unwrap_or("")
                .contains("0.5-and-true beats 0.9-and-guessed"),
            "the analyzer anchor IS its honesty law: {v}"
        );
        let disc = serde_json::to_string(&v["discipline"]).unwrap();
        assert!(
            disc.contains("THE SOCRATIC LOOP is the skill")
                && disc.contains("no code read, no verdict")
                && disc.contains("independent"),
            "the analyzer discipline leads with the thesis + bakes in the refusal: {disc}"
        );
        assert!(
            v["adopt"].as_str().unwrap_or("").contains("loom-analyzer"),
            "the charge frames itself as skill adoption: {v}"
        );
        // EVERY role serves a complete skill (name + anchor + non-empty discipline),
        // so the binary can serve any lane JIT with no shipped/installed file.
        for role in crate::db::schema::ROLES {
            let c = charge_json(role);
            assert_eq!(c["skill"], serde_json::json!(format!("loom-{role}")));
            assert!(
                !c["anchor"].as_str().unwrap_or("").is_empty(),
                "role '{role}' has an anchor motto"
            );
            assert!(
                c["discipline"]
                    .as_array()
                    .map(|a| !a.is_empty())
                    .unwrap_or(false),
                "role '{role}' serves a non-empty discipline body"
            );
        }
    }

    #[test]
    fn architecture_metadata_guidance_reaches_goldfish_surfaces() {
        let guide = guide_json("brownfield");
        let arch = guide["architecture_metadata"]
            .as_str()
            .expect("architecture metadata guidance is a first-class section");
        for required in [
            "positive evidence, not a template",
            "backend/frontend/database",
            "--domain",
            "--layer",
            "loom layer list",
            "detector is unarmed",
            "--boundary inbound|outbound",
        ] {
            assert!(
                arch.contains(required),
                "architecture guidance must teach '{required}': {arch}"
            );
        }

        let builder = serde_json::to_string(&charge_json("builder")).unwrap();
        assert!(
            builder.contains("backend/frontend/database")
                && builder.contains("detector is unarmed"),
            "builder lane must carry architecture-boundary guidance for cold LLMs: {builder}"
        );
    }

    #[test]
    fn role_charge_rejects_unknown_role() {
        let p = Printer::capturing(true);
        let err = run(None, Some("analyser"), false, &p)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown role"), "got: {err}");
        assert!(
            err.contains("analyzer"),
            "the error inlines the valid roles: {err}"
        );
    }

    /// COMPLETENESS RATCHET (coverage leg): every section in the teaching
    /// completeness contract must be present in the json payload. A section
    /// added to GUIDE_SECTIONS that the render forgets — or dropped from the
    /// render but left registered — fails the build, so the self-teaching plane
    /// is held to "verify against code" like every other plane in loom.
    #[test]
    fn guide_json_exposes_every_canonical_section() {
        let v = guide_json("brownfield");
        for key in GUIDE_SECTIONS {
            assert!(
                v.get(key).is_some(),
                "guide --json is missing canonical teaching section '{key}'. Provide it in the json payload, or remove it from GUIDE_SECTIONS — the build refuses a half-landed section."
            );
        }
    }

    /// R7/R8/R9/R10 (audit close-out): the mode guidance must teach cross-intent
    /// idiom consistency on PORT, the solo proposer≠prover discipline on
    /// REFACTOR, the when-to-call-`loom session` cue on GREENFIELD + SEED, and
    /// IMPORT/ADOPT must exist as a first-class observe→capture→route walkthrough.
    #[test]
    fn mode_guidance_closes_the_audit_gaps() {
        // R7: port teaches cross-intent naming/idiom consistency via vocab.
        let port = serde_json::to_string(&guide_json("port")).unwrap();
        assert!(
            port.contains("idioms")
                && port.contains("vocab_drift")
                && port.contains("loom vocab add"),
            "port mode must teach cross-intent idiom consistency"
        );
        // R8: refactor teaches the solo proposer≠prover discipline + doctor audit.
        let refactor = serde_json::to_string(&guide_json("refactor")).unwrap();
        assert!(
            refactor.contains("SOLO")
                && refactor.contains("proposer≠prover")
                && refactor.contains("loom doctor"),
            "refactor mode must teach the solo proposer≠prover discipline"
        );
        // R9: greenfield + seed cue WHEN to call loom session for user-gated work.
        for mode in ["greenfield", "seed"] {
            let blob = serde_json::to_string(&guide_json(mode)).unwrap();
            assert!(
                blob.contains("loom session") && blob.to_lowercase().contains("autonomous lane"),
                "{mode} mode must cue when to call loom session"
            );
        }
        // R10: import/adopt is a first-class mode with the observe→capture→route spine.
        let import = serde_json::to_string(&guide_json("import")).unwrap();
        for beat in ["--observed", "inbox add", "delegate", "--as-planned"] {
            assert!(
                import.contains(beat),
                "import mode must teach the '{beat}' beat"
            );
        }
        // The --adopt alias resolves to the same mode.
        assert_eq!(guide_json("adopt")["mode"], serde_json::json!("import"));
    }

    /// SEED-LADDER RATCHET: the seed mode must stage the want→contract→logic→
    /// physical ladder AND capture audience (--visibility) at seed time, plus
    /// carry the visual-register + mockup-as-contract + acceptance-stub rungs.
    /// These are the UI/seed-flow cluster's load-bearing guidance; a silent
    /// regression of any of them fails the build.
    #[test]
    fn seed_mode_stages_the_ladder_and_seed_time_audience() {
        let blob = serde_json::to_string(&guide_json("seed")).unwrap();
        for rung in ["want", "contract", "logic", "physical"] {
            assert!(
                blob.contains(rung),
                "seed ladder must name the '{rung}' rung"
            );
        }
        assert!(
            blob.contains("--visibility"),
            "seed mode must capture audience (--visibility) at seed time"
        );
        for surface in [
            "persona",      // want stage names the audience
            "manual_check", // acceptance-proof stub
            "the visual register",
            "mockup is contract",
            "visual-confirm queue", // human aesthetic pass, user-gated in loom session
        ] {
            assert!(
                blob.contains(surface),
                "seed mode must teach '{surface}' (UI/seed-flow cluster)"
            );
        }
    }

    /// PARITY RATCHET: the storage/performance guidance must reach the --json
    /// driver. Single-sourced via PERFORMANCE_GUIDANCE, so the human render
    /// prints the identical text and copy drift cannot recur.
    #[test]
    fn performance_guidance_reaches_the_json_driver() {
        let v = guide_json("brownfield");
        assert_eq!(
            v["orchestration"]["performance"],
            serde_json::json!(PERFORMANCE_GUIDANCE),
            "the performance guidance must travel in --json, single-sourced from PERFORMANCE_GUIDANCE"
        );
        assert!(
            PERFORMANCE_GUIDANCE.contains("graph.sqlite"),
            "performance guidance must name the active SQLite store"
        );
    }

    /// Every declared mode renders valid json with the canonical sections (the
    /// contract holds across the whole disclosure surface, not just one mode).
    #[test]
    fn every_mode_satisfies_the_contract() {
        for mode in ["greenfield", "brownfield", "refactor", "port", "seed"] {
            let v = guide_json(mode);
            assert_eq!(v["mode"], serde_json::json!(mode), "mode echoed");
            assert!(
                v.get("orchestration").is_some(),
                "{mode}: orchestration present"
            );
        }
    }

    /// FINDABILITY RATCHET (leg 3): the guide names every pull surface, so when
    /// Just-In-Time anchoring isn't enough the LLM can reach the rest from the
    /// entry point rather than guess. Drop a reference and the build fails.
    #[test]
    fn guide_names_every_pull_surface() {
        let blob = serde_json::to_string(&guide_json("brownfield")).unwrap();
        for surface in FINDABILITY_SURFACES {
            assert!(
                blob.contains(surface),
                "guide --json never names the pull surface '{surface}' — a cold LLM can't find it when JIT isn't enough. Reference it in the playbook or remove it from FINDABILITY_SURFACES."
            );
        }
    }

    /// PARITY GUARD for the section that drifted: the json `orchestration`
    /// object keeps a home for every concept the human render teaches. A new
    /// orchestration concept added human-only fails here — the closest
    /// mechanical guard short of routing the prose render through the capturable
    /// Printer.
    #[test]
    fn orchestration_section_stays_complete() {
        let v = guide_json("brownfield");
        for key in ORCHESTRATION_KEYS {
            assert!(
                v["orchestration"].get(key).is_some(),
                "orchestration.{key} missing from guide --json"
            );
        }
    }

    /// honesty-next #4 / loom-dx (concurrent-flock): the guide must teach the
    /// REAL concurrency behavior. loom DOES cross-process-lock writes (advisory
    /// flock on `.loom/graph.lock` per write tx; see acquire_write_lock /
    /// write_tx in src/db/sqlite.rs) and a collision yields a loom-NAMED error,
    /// never a raw "database is locked". Both audiences must say so — an
    /// orchestrator must know loom serializes writers for it (so it doesn't build
    /// redundant serialization or key error-handling on a raw string loom never
    /// emits). Guards against the obsolete no-lock claim sneaking back; mirrors
    /// the lock's own unit test write_lock_serializes_writers_with_a_named_error.
    #[test]
    fn orchestration_concurrency_teaches_the_real_write_lock() {
        let v = guide_json("brownfield");
        let concurrency = v["orchestration"]["concurrency"]
            .as_str()
            .expect("orchestration.concurrency present in guide --json");
        assert!(
            concurrency.contains("cross-process-lock") && concurrency.contains("serializes"),
            "must state loom DOES serialize writers via a cross-process lock: {concurrency}"
        );
        assert!(
            concurrency.contains("NAMED error") || concurrency.contains("named error"),
            "must say a collision yields a loom-named error (not a raw one): {concurrency}"
        );
        assert!(
            !concurrency.contains("does NOT cross-process-lock"),
            "must NOT carry the obsolete no-lock falsehood: {concurrency}"
        );
        // Human parity: the prose render prints the ORCHESTRATION array verbatim
        // (raw println!, which bypasses Printer capture — so assert on the const
        // that IS the human source of truth). The truth must live in both audiences.
        let human = super::ORCHESTRATION.join(" ");
        assert!(
            human.contains("CONCURRENCY")
                && human.contains("cross-process-lock")
                && !human.contains("does NOT cross-process-lock"),
            "human guide (ORCHESTRATION const) carries the real lock story too: {human}"
        );
    }

    #[test]
    fn lifecycle_contract_is_complete() {
        let v = guide_json("brownfield");
        let lifecycle = &v["lifecycle"];
        for key in LIFECYCLE_JSON_KEYS {
            assert!(
                lifecycle.get(key).is_some(),
                "lifecycle.{key} missing from guide --json"
            );
        }

        let states = lifecycle["active_states"]
            .as_array()
            .expect("lifecycle.active_states is an array");
        let state_names = states
            .iter()
            .filter_map(|state| state["state"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            state_names,
            vec!["planned", "implemented", "needs_change", "deferred"],
            "guide must teach every active implementation lifecycle state in schema order"
        );

        let blob = serde_json::to_string(lifecycle).unwrap();
        for required in [
            "loom intent retire",
            "status=deprecated",
            "loom import <source-export> --as-planned",
            "validation.last_result",
            "edge.inspection_status",
            "hypothesis.status",
            "loom intent update",
            "--reword",
        ] {
            assert!(
                blob.contains(required),
                "guide lifecycle contract does not teach '{required}'"
            );
        }
    }

    /// Greenfield step 2 must tell the LLM to INTERVIEW the user before writing
    /// intents — not jump straight to `loom intent add`. The grill technique
    /// (calibrate altitude, one question at a time, recommended answers,
    /// terminate on graph completeness) lives in `loom guide --mode seed`;
    /// the greenfield playbook must reference it so a cold LLM knows it exists.
    #[test]
    fn greenfield_playbook_teaches_the_interview_before_intents() {
        let blob = serde_json::to_string(&guide_json("greenfield")).unwrap();
        assert!(
            blob.contains("INTERVIEW"),
            "greenfield step 2 must say INTERVIEW before landing intents"
        );
        assert!(
            blob.contains("loom guide --mode seed"),
            "greenfield must reference the seed mode for the full grill technique"
        );
        assert!(
            blob.contains("one question at a time"),
            "greenfield must teach one-question-at-a-time"
        );
        assert!(
            blob.contains("calibrating altitude"),
            "greenfield must teach altitude calibration"
        );
    }
}
