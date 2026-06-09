//! `loom guide` — self-contained onboarding for an LLM new to loom, with a
//! mode-specific playbook (greenfield / brownfield / refactor). The mode is
//! auto-detected from the repo (via `loom detect`) unless given with `--mode`.

use anyhow::Result;
use std::env;

use crate::output::Printer;

const GOLDEN_RULES: &[&str] = &[
    "Drive via `loom next` — it prioritises and tells you the exact next command.",
    "After ANY code change: `loom sync`. It is the flag engine — see THE RIPPLE below.",
    "Per edge, work the Socratic loop: read both intents → form a hypothesis (\"I expect the code to show X\") → inspect the actual code → confirmed = ground it, code wrong = record the issue, hypothesis wrong = revise and re-inspect. Never record a verdict you didn't check.",
    "Batch by neighborhood: when you inspect an edge, `loom cluster <intent-id>` lists every other unresolved edge touching it — work those while the context is loaded.",
    "Use `--json` on every command for machine-readable output (incl. a `graph_state` pulse).",
    "Every command has `--help`. `loom schema` = data model; `loom status` = where you are; `loom doctor` = integrity.",
    "Prescriptive intents (planned/needs_change) still need a falsifiable criterion — that's what makes the design a test.",
    "Two axes of completeness. VERTICAL is the binding spine: HIERARCHY is a tree (one parent per intent), every implemented leaf intent is grounded in code (IMPLEMENTS), every CodeFile is reached. HORIZONTAL (RELATES_TO, the N×N grid) is optional understanding/cleanup.",
    "Done (vertical) when `loom status` shows vertical ✓ + `loom coverage` reports nothing unaccounted. Horizontal closure (phase=complete) is optional polish.",
    "Criteria and evidence must be substantive — loom rejects placeholders, and `loom doctor` audits verdicts (vacuous criterion, bad confidence, out-of-lane provenance).",
];

/// What `loom sync` invalidates when a registered file's mtime advances — the
/// graph's impact analysis, taught up-front so a driver knows why green decays.
const RIPPLE: &[&str] = &[
    "RELATES_TO edges of intents grounded in the changed file → needs_reverification (re-inspect via `loom next --mode fix`)",
    "passing GOVERNS verdicts on those intents → needs_reverification (quality green is re-earned via `loom next --mode quality` + `loom rule verdict`)",
    "Validations linked to those intents → last_result = not_run (re-run via `loom validate <intent>`)",
    "files registered in the graph but missing on disk are reported — drop phantoms with `loom codefile remove <path>` or restore the file",
];

/// The role lanes: who does what, and which `loom next` mode serves the lane.
/// Declared roles (LOOM_AGENT=llm:<role>) are ENFORCED — an agent acting
/// outside its lane gets an error. Bare `llm`/`human` = solo mode (all lanes).
const ROLE_LANES: &[(&str, &str, &str)] = &[
    ("builder",   "build",     "constructs the graph: intents, hierarchy, codefiles, IMPLEMENTS links, lifecycle"),
    ("analyzer",  "discovery", "the Socratic loop: grounds RELATES_TO edges with criterion/evidence/verdict"),
    ("fixer",     "fix",       "resolves failing edges (`loom edge fix`) and needs_change intents; re-grounds what it repairs"),
    ("validator", "validate",  "proves intents: runs validations, confirms intents (`loom intent confirm`)"),
    ("quality",   "quality",   "the green gate: defines rules, applies them, records GOVERNS verdicts (`loom rule verdict`)"),
];

fn brownfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the repo root."),
        ("seed intents", "Read the code; add `system` → `component` → `feature` intents (lifecycle defaults to `implemented`). Link with `loom edge hierarchy <parent> <child>`."),
        ("ground to code", "`loom codefile add '<glob>'` then `loom edge implement <intent> <codefile> --locator \"fn …\"`."),
        ("discover", "`loom next` repeatedly: read the code it points to, then record `loom edge explore <a> <b> ground|issue|independent …`."),
        ("fix", "`loom next --mode fix` for failing/stale edges."),
        ("coverage", "`loom coverage` — map or `loom ignore` every file so nothing is missed."),
        ("prove", "`loom validation add …` + `loom edge validates …`, then `loom validate <intent>`."),
        ("gate", "Encode the codebase's norms (e.g. ISO 5055-style reliability/security/maintainability): `loom rule add …`, `loom rule apply <rule> <intent>`, then earn green — `loom next --mode quality` + `loom rule verdict … --status passing|failing --criterion … --evidence …`."),
    ]
}

fn greenfield() -> Vec<(&'static str, &'static str)> {
    vec![
        ("init", "`loom init` in the (empty/new) repo root."),
        ("design as planned intents", "Write the spec AS intents: `loom intent add … --level system|component|feature --lifecycle planned`. Each feature's criterion IS its acceptance contract. Use `--aspect happy|sad|fallback` so error paths are designed in."),
        ("capture architecture", "Relate intents: `loom edge hierarchy` for structure, `loom edge explore … ground` for contracts between components."),
        ("build", "`loom next --mode build` → for each planned LEAF intent: write the code, `loom codefile add`, `loom edge implement`, then `loom intent mark <id> --lifecycle implemented`. Parents are deferred until their children are done, then surface as a roll-up. The criterion you wrote is your test."),
        ("verify", "Once built, `loom next` (discovery) and `loom validate` confirm reality matches the design."),
        ("gate", "Set the quality bar: `loom rule add …` + `loom rule apply`, then earn green with `loom next --mode quality` + `loom rule verdict`."),
    ]
}

fn refactor() -> Vec<(&'static str, &'static str)> {
    vec![
        ("map first if needed", "If the area isn't in the graph yet, do the brownfield steps for it."),
        ("flag what must change", "`loom intent mark <id> --lifecycle needs_change --reason \"…\"`. Set/refresh the criterion to the desired end state; capture rationale (the --reason is recorded as a note). This is the honest 'known issue' state — no faking a verdict."),
        ("build", "`loom next --mode build` surfaces needs_change intents first. Make the minimal change, then `loom intent mark <id> --lifecycle implemented`."),
        ("re-verify", "`loom sync` — it flags everything the change touched (stale relationships, stale quality green, invalidated proofs). Then `loom next --mode fix`, `--mode quality`, and `loom validate` until the compass is green."),
    ]
}

fn resolve_mode(mode: Option<&str>) -> Result<&'static str> {
    if let Some(m) = mode {
        return match m {
            "greenfield" => Ok("greenfield"),
            "brownfield" => Ok("brownfield"),
            "refactor" => Ok("refactor"),
            other => anyhow::bail!("Unknown mode '{}'. Valid: greenfield, brownfield, refactor", other),
        };
    }
    // Auto-detect from the repo: no source on disk → greenfield, else brownfield.
    let cwd = env::current_dir()?;
    Ok(if crate::repo::detect(&cwd).has_source { "brownfield" } else { "greenfield" })
}

pub fn run(mode: Option<&str>, printer: &Printer) -> Result<()> {
    let m = resolve_mode(mode)?;
    let steps = match m {
        "greenfield" => greenfield(),
        "refactor" => refactor(),
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
    println!("Other modes: `loom guide --mode greenfield|brownfield|refactor`. Start: `loom status` · `loom next`.");
    Ok(())
}
