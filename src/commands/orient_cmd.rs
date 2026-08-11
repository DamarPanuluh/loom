//! Orientation command family — welcome, session, guide.
//!
//! Plane: read-only human/operator orientation over the compass and queues.
//! A translation layer — never new routing logic.

use super::*;

/// Plain-English, jargon-free orientation for a human first landing on loom
/// (also what bare `loom` prints). A translation layer over the compass — never
/// new logic — so it can't drift from what `loom status`/`loom next` route to.
pub(crate) fn welcome(graph: Option<&Path>, json: bool) -> Result<()> {
    // A missing graph is not an error here — the human simply hasn't started.
    let store = match super::open_read(graph) {
        Ok(s) => s,
        Err(_) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "initialized": false,
                        "intro": WELCOME_INTRO,
                        "idea_help": PARTIAL_IDEA_HELP,
                        "get_started": [
                            "loom init",
                            "loom door \"the user journey this codebase should support\""
                        ],
                    }))?
                );
            } else {
                print_welcome_intro();
                println!();
                println!("  No loom graph here yet.");
                println!("  → Get started:  loom init");
                println!("                  then  loom door \"the user journey this codebase should support\"");
                println!();
                println!("  Go deeper:  loom guide");
            }
            return Ok(());
        }
    };

    let active = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)?
        .iter()
        .filter(|n| n.status != "deprecated")
        .count();
    let journeys = store.list_nodes(Some(NodeType::Journey), usize::MAX)?.len();
    let ladder = crate::maturity::ladder(&store)?;
    let (headline, why) = phase_in_plain_english(&ladder.phase);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "initialized": true,
                "intro": WELCOME_INTRO,
                "idea_help": PARTIAL_IDEA_HELP,
                "intents": active,
                "journeys": journeys,
                "phase": ladder.phase,
                "state": headline,
                "next_command": ladder.next_command,
                "why": why,
            }))?
        );
        return Ok(());
    }

    print_welcome_intro();
    println!();
    println!("  Where you are now:");
    println!("    {journeys} Journey root(s), {active} technical intent(s).  {headline}");
    println!();
    println!("  → Do this next:  {}", ladder.next_command);
    println!("    {why}");
    println!();
    println!("  Go deeper:  loom status (the ladder)   loom guide (full protocol)");
    println!("  New idea?   loom door \"the journey you want a user to complete\"");
    println!();
    println!("  (run `loom --help` to see every command)");
    Ok(())
}

const WELCOME_INTRO: &str = "loom — a living map from authored user Journeys to code and proof.";
const PARTIAL_IDEA_HELP: &str = "You can start with a partial journey. Loom helps shape the authored root, derive technical Intents, surface a real CLI, and ask for your judgment one understandable question at a time.";

fn print_welcome_intro() {
    println!("{WELCOME_INTRO}");
    println!("  {PARTIAL_IDEA_HELP}");
    println!();
    println!("  Every Journey is an authored root. Loom derives the technical Intents, links them");
    println!("  to the code and surfaced CLI, tracks what's proven, and always points you");
    println!("  at the single next thing worth doing. You climb a ladder:");
    println!();
    println!("    author the Journey → derive → build → surface → prove → keep it clean");
}

/// Translate a compass phase into a human headline + the reason to act. The
/// phase strings are owned by `maturity::compass`; keep this in step with them.
fn phase_in_plain_english(phase: &str) -> (&'static str, &'static str) {
    match phase {
        "seed" => (
            "No Journey root is authored yet.",
            "Author the user behavior first, then register it with `loom journey add <spec>`.",
        ),
        "derive" => (
            "Some Journey steps have no accepted technical meaning yet.",
            "Derive the smallest falsifiable technical Intents, then ask the human to accept the exact manifest.",
        ),
        "fix" => (
            "Something that was true has broken.",
            "Repair the failing claim first — everything downstream leans on it.",
        ),
        "build" => (
            "Some intents have no working code yet.",
            "Build the next one; loom hands you the intent and what it needs.",
        ),
        "surface" => (
            "A derived Journey has no reusable consumer CLI yet.",
            "Build the real CLI in the target repo and accept its hash-bound surface manifest.",
        ),
        "coverage" => (
            "Some code isn't tied to any intent.",
            "Connect each unowned file to the intent it serves (or ignore it).",
        ),
        "validate" => (
            "Some code is written but not yet proven to work.",
            "Pick up an implemented intent and confirm it actually does what it claims.",
        ),
        "quality" => (
            "The build is proven; now hold it against your quality rules.",
            "Judge the next rule against the intent it applies to.",
        ),
        "analyze" => (
            "There are relationships worth understanding.",
            "Inspect the next pair and record what the code actually shows.",
        ),
        "review" => (
            "Some verdicts were recorded with honest uncertainty.",
            "Re-inspect the least confident one independently and settle it.",
        ),
        "prove" => (
            "There are proposed changes nobody has tested yet.",
            "Take the next hypothesis and find out whether it holds.",
        ),
        "elaborate" => (
            "Some user-visible ideas are only half-described.",
            "Fill in the sad paths, prerequisites, and proofs around the next one.",
        ),
        "ratify" => (
            "The graph found behavior your judgment hasn't spoken to.",
            "Keep it or kill it — loom has already gathered the evidence.",
        ),
        "audit" => (
            "There are open issues or code smells to look at.",
            "Work through what loom flagged — fix each, or consciously accept it.",
        ),
        "triage" => (
            "There are findings waiting on a decision.",
            "Confirm each into work, or dismiss it with a reason.",
        ),
        "deepen" => (
            "Everything owed is done; now the graph gets harder on itself.",
            "Strengthen the weakest proof under the code most depended on.",
        ),
        "export" => (
            "Your graph has changes that aren't in the shareable snapshot yet.",
            "Export it so the committed graph matches reality.",
        ),
        "complete" => (
            "You're all caught up — built, proven, and clean.",
            "Keep coding; run `loom sync` after changes and loom will surface what's next.",
        ),
        _ => ("", "Run `loom status` to see the full ladder."),
    }
}

pub(crate) fn session(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    // One source of truth for the counts: the same pulse every work item and
    // mutating command emits. Session only adds the offer framing on top.
    let pulse = crate::workitem::graph_state(&store)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?.len();
    let journeys = store.list_nodes(Some(NodeType::Journey), usize::MAX)?.len();
    let codefiles = store
        .list_nodes(Some(NodeType::CodeFile), usize::MAX)?
        .len();
    let open_axes: usize = crate::completeness::all_scorecards(&store)?
        .iter()
        .filter(|c| c.visibility.as_deref() == Some("user_visible"))
        .map(|c| c.open)
        .sum();
    let ladder = crate::maturity::ladder(&store)?;
    if json {
        // Serialize the rungs directly so the derived `blocked`/`blocked_by`
        // fields stay in sync with `loom status` and can't drift.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "graph_state": pulse,
                "intents": intents,
                "journeys": journeys,
                "codefiles": codefiles,
                "open_completeness_axes": open_axes,
                "phase": ladder.phase,
                "recommended": ladder.next_command,
                "capture_entry": "loom door \"<utterance>\" — route a new topic/story/change toward a Journey root",
                "bootstrap_suggest": if journeys == 0 && intents == 0 && codefiles > 0 {
                    Some("loom bootstrap suggest — recover behavior clues from codefiles/tests/README before authoring Journeys")
                } else {
                    None
                },
                "rungs": ladder.rungs,
            }))?
        );
        return Ok(());
    }
    println!("what do you want from this session? offers:");
    println!(
        "  - recommended: {}              (phase: {})",
        ladder.next_command, ladder.phase
    );
    // The offer IS the ladder's gate — one decision structure. Before the lane
    // table this was a hand-maintained if-chain that could disagree with both
    // the compass and the queue depths.
    let open_rungs: Vec<&crate::maturity::Rung> = ladder
        .rungs
        .iter()
        .filter(|r| r.state == crate::maturity::RungState::Unmet && r.lane.serves_items())
        .collect();
    if let Some(gate) = open_rungs.first() {
        println!(
            "  - {} — {}   [{}]",
            gate.name,
            gate.detail,
            gate.lane.next_command()
        );
        for r in open_rungs.iter().skip(1).take(2) {
            println!("  - then {}: {}", r.name, r.detail);
        }
    } else if journeys == 0 && codefiles == 0 {
        println!("  - fresh graph — no Journey root authored yet. Start here:");
        println!("      loom guide                  the driving loop + roles");
        println!("      loom guide --role monitor   watch an upstream you depend on");
        println!("      loom journey add <spec>     register the authored user Journey root");
    } else if journeys == 0 && codefiles > 0 {
        println!("  - code registered, no Journey root yet — recover clues, then author one:");
        println!(
            "      loom bootstrap suggest      Proposal of behavior clues from code/tests/README"
        );
        println!("      loom journey add <spec>     register the human-authored Journey root");
    } else {
        println!("  - graph is settled; map more, or just get to work");
    }
    if pulse.open_questions > 0 {
        println!(
            "  - {} question(s) waiting for YOUR answer  [loom question list --status open]",
            pulse.open_questions
        );
    }
    if pulse.inbox > 0 {
        println!(
            "  - {} inbox item(s) to triage          [loom inbox list --status new]",
            pulse.inbox
        );
    }
    if pulse.low_confidence > 0 {
        println!(
            "  - re-inspect {} low-confidence verdict(s) [loom next --mode review]",
            pulse.low_confidence
        );
    }
    if open_axes > 0 {
        println!(
            "  - grow {open_axes} open completeness axis(es) around user-visible ideas [loom next --mode elaborate]"
        );
    }
    println!(
        "  - got a topic/story/change in mind?  loom door \"<utterance>\"   (capture + landing menu)"
    );
    Ok(())
}
fn truth_axis_matrix() -> Vec<serde_json::Value> {
    crate::truth::TRUTH_AXES
        .iter()
        .map(|axis| {
            let g = axis.gap();
            serde_json::json!({
                "axis": g.axis.as_str(),
                "missing_form": g.missing_form,
                "correct_when": g.correct_when,
                "authoritative_write": g.authoritative_write,
                "forbidden_write": g.forbidden_write,
                "after_write": g.after_write,
            })
        })
        .collect()
}
fn operator_loops() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "mode": "seeding",
            "purpose": "turn ambiguous product understanding into authored Journey roots and accepted projections",
            "caller": "user or orchestrator chooses this when using a stronger model or human operator",
            "prefer": [
                "loom door <utterance>",
                "loom journey add <spec>",
                "loom next --mode derive",
                "loom next --mode build",
                "loom next --mode surface",
                "loom rule seed <pack>"
            ],
            "creates": [
                "authored Journey roots",
                "human-approved technical Intent derivations",
                "real target-repository CLI surfaces",
                "scenario families",
                "prerequisite edges",
                "interface boundaries",
                "validations",
                "product questions",
                "human-authorized Journey exemptions"
            ],
            "forbidden": [
                "answering product questions for the human",
                "marking proofs passed without observed runs",
                "using prose summaries instead of graph artifacts"
            ],
        }),
        serde_json::json!({
            "mode": "draining",
            "purpose": "close already-routed graph gaps one packet at a time",
            "caller": "user or orchestrator chooses this when using a cheaper/bounded model or automation",
            "prefer": [
                "loom next",
                "loom next --mode fix",
                "loom next --mode derive",
                "loom next --mode surface",
                "loom next --mode validate",
                "loom next --mode quality",
                "loom next --mode analyze",
                "loom next --mode review",
                "loom validation run <intent>",
                "loom journey compile <journey> --profile proof",
                "loom journey run <journey> --profile proof",
                "loom export --check"
            ],
            "closes": [
                "failing/stale implementation claims",
                "unrun validations",
                "stale compiled Journey proofs",
                "unmeasured quality rules",
                "uninspected relationships",
                "low-confidence review items",
                "export freshness"
            ],
            "forbidden": [
                "inventing broad product structure",
                "expanding beyond the packet",
                "silently waiving missing meaning"
            ],
        }),
    ]
}

fn print_operator_loops() {
    println!("Operator modes — caller chooses the mode/model; evidence still proves truth:");
    println!("  seeding   use a stronger model/human to turn ambiguous understanding into graph artifacts");
    println!("            author Journey roots, accept technical derivations, build CLI surfaces, and route product questions");
    println!(
        "            do not answer product questions or mark proofs passed without observed runs"
    );
    println!(
        "  draining  use a bounded/cheaper model to close already-routed gaps one packet at a time"
    );
    println!("            compile/run Journey proof profiles, inspect stated claims, and record observed evidence or blockers");
    println!("            do not invent broad product structure or expand beyond the packet");
    println!("  invariant mode routes work; role controls writes; evidence determines truth.");
}

pub(crate) fn guide(role: Option<&str>, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "role": role,
                "commands": ["loom sync", "loom next --all", "loom status", "loom coverage", "loom doctor", "loom export --check", "loom door", "loom finding add", "loom question add"],
                "intake": {
                    "human_or_external_input": "loom door \"<utterance>\" — capture raw input, route it to an existing or newly authored Journey",
                    "evidence_backed_observation": "loom finding add \"<claim>\" --source code_audit --file <codefile> --evidence \"…\" --impact \"…\" --confidence <n>",
                    "product_question": "loom question add \"<question>\" --intent <intent>",
                    "structured_plan": "loom proposal add --title '…' (--file <path> | --text '…') — decompose into adoptable items",
                    "falsifiable_design_claim": "loom hypothesis add --name '…' --claim '…' --target <intent> — prove supported|refuted before it becomes work",
                    "timeboxed_activity": "loom task add '<title>' --kind spike --target '<intent>' — close with a result (lands as a note on the target intent); targetless stays diary-only"
                },
                "roles": ["builder", "analyzer", "fixer", "validator", "quality", "rectify", "monitor"],
                // Derived from the lane table so this can never drift from the
                // ladder it describes.
                "rung_gates": crate::lane::Lane::LADDER.iter().map(|l| l.rung()).collect::<Vec<_>>(),
                "lanes": crate::lane::Lane::LADDER.iter().map(|l| serde_json::json!({
                    "lane": l.as_str(),
                    "rung": l.rung(),
                    "axis": l.axis().as_str(),
                    "human_only": l.requires_human_decision(),
                })).collect::<Vec<_>>(),
                "closeout": ["loom coverage", "loom doctor", "loom next --all", "loom export", "loom export --check"],
                "operator_loops": operator_loops(),
                "truth_axes": truth_axis_matrix(),
            }))?
        );
        return Ok(());
    }
    match role {
        None => {
            println!("loom — driving protocol (the loop):");
            println!("  loom sync       recompute the structural plane after code changes");
            println!("  loom next --all show every lane queue + compass");
            println!("  loom next       serve one work item + its prompt contract");
            println!("  loom status     rung ladder + the single next move");
            println!(
                "  loom door       capture a raw utterance and route it toward a Journey root"
            );
            println!("Capture routing — pick the entrance by input shape:");
            println!("  human/external input             loom door \"<utterance>\"        capture raw input; route to a Journey, then mark routed");
            println!("  evidence-backed code/tool smell  loom finding add \"<claim>\" ... capture for finding triage");
            println!("  product decision needed          loom question add \"<question>\" --intent <intent>");
            println!("  structured plan / RFC              loom proposal add               decompose into adoptable items");
            println!("  falsifiable design claim           loom hypothesis add             prove supported|refuted, then adopt");
            println!("  timeboxed activity                 loom task add --target          close with a result; lands as a note on the target intent (targetless = diary-only)");
            println!(
                "Closeout gates: loom coverage; loom doctor; loom next --all; loom export --check."
            );
            print_operator_loops();
            println!();
            println!("Truth forms — fill the one that is stale/missing (loom next names it):");
            for axis in crate::truth::TRUTH_AXES {
                let g = axis.gap();
                println!("  {:<15} {}", g.axis.as_str(), g.missing_form);
                println!("      correct when: {}", g.correct_when);
                println!("      make true:    {}", g.authoritative_write);
                println!("      then:         {}", g.after_write);
            }
            println!("Roles: builder | analyzer | fixer | validator | quality | rectify (see `loom guide --role`).");
            println!("Integration monitoring (watch an upstream you depend on): loom guide --role monitor");
            Ok(())
        }
        Some("monitor") => {
            guide_monitor();
            Ok(())
        }
        Some(r) => {
            let (mindset, allowed, forbidden, axis) = match r {
                "builder" => (
                    "Use Loom first to understand why, likely files/entities, and prior evidence; then inspect relevant code before editing. Functions are locators, not intents.",
                    "loom status; loom next --all; loom intent show <intent>; loom codefile list; loom codefile show <file>; edit code; loom edge implement; loom intent update <intent> --lifecycle implemented --reason '…'; loom sync",
                    "loom rule verdict passing; loom validation verdict passed",
                    crate::truth::TruthAxis::Implementation,
                ),
                "analyzer" => (
                    "Read both sides; hypothesis first; record exactly what the code shows. Also triages findings — record needed/justified/rejected/deferred/blocked/duplicate/resolved with a reason. Use resolved only after observing the repair. Serves the review queue too: re-inspect low-confidence verdicts independently before reading the recorded evidence.",
                    "loom edge explore <a> <b> ground|issue|independent; loom edge verdict <edge_id> ground|issue|independent (non-relates claims); loom finding verdict <id> needed|justified|rejected|deferred|blocked|duplicate|resolved --reason '…'",
                    "edit code; verdict from name similarity; inheriting a prior verdict's confidence",
                    crate::truth::TruthAxis::Verdict,
                ),
                "fixer" => (
                    "Use Loom first to understand the stale/failing criterion, linked entities, likely files, and prior evidence; then inspect relevant code before repairing the root cause. Findings judged `needed` are queued work — consult `loom finding list --state needed`. After the fix, `loom sync` re-opens the claim and its owning lane re-measures — do not record the verdict yourself.",
                    "loom status; loom next --all; loom edge show <edge_id>; loom intent show <linked intent>; loom codefile show <file>; edit code; loom sync; re-ground; loom finding list --state needed",
                    "suppress the symptom; record the passing verdict from the fixer hat",
                    crate::truth::TruthAxis::Implementation,
                ),
                "validator" => (
                    "Run or honestly mark proofs; never edit code to make a proof pass.",
                    "run validation; loom validation run <intent>; loom validation verdict <validation> passed|failed|blocked --evidence '…'",
                    "edit code; mark passed without observed proof",
                    crate::truth::TruthAxis::Proof,
                ),
                "quality" => (
                    "Measure a rule against an intent at the highest honest altitude. Follow the rule's inspection_guide and evidence_template from the work packet; do not invent your own protocol.",
                    "loom rule verdict <rule> <intent> passing|failing|independent --criterion '…' --evidence '…' --confidence <n>",
                    "edit code; mark passing without inspecting; mark independent without evidence",
                    crate::truth::TruthAxis::Verdict,
                ),
                "rectify" => (
                    "Clear NEEDLESS ratify friction without deciding wantedness. Fix false duplicates (scenario_of / retire), demote mis-marked visibility to internal, or escalate real product calls to human ratify. Never invent a yes.",
                    "loom next --mode rectify; loom intent update --visibility internal; loom intent update --rectify escalated; loom edge relate scenario-of; loom intent retire --replaced-by",
                    "loom intent ratify; loom intent reject; supplying --human-decision; editing code to silence a divergence",
                    crate::truth::TruthAxis::Intent,
                ),
                other => bail!("unknown role '{other}'"),
            };
            println!("role: {r}");
            println!("  mindset:   {mindset}");
            println!(
                "  axis:      {} — correct when {}",
                axis.as_str(),
                axis.gap().correct_when
            );
            println!("  allowed:   {allowed}");
            println!("  forbidden: {forbidden}");
            println!("  honesty:   confidence below {} (the default policy cutoff) routes the verdict to review — uncertainty is honest, a confident guess corrupts the graph", crate::policy::DEFAULT_REVIEW_CONFIDENCE_FLOOR);
            println!("  set: export LOOM_AGENT=llm:{r}");
            Ok(())
        }
    }
}
fn guide_monitor() {
    println!("loom — integration monitoring (watch an upstream you depend on):");
    println!(
        "  Goal: when an upstream you consume changes, loom resets the contracts that exercise it,"
    );
    println!("  so `loom sync` tells you exactly what needs re-checking. This is your own graph.");
    println!(
        "  Pass intents/validations/surfaces by NAME (the quoted string) or by the short [id]."
    );
    println!();
    println!("  1. Get the upstream's files onto disk under vendor/<name>/ . If it is a git repo,");
    println!("     a submodule keeps it pinned; otherwise just copy/vendor the files in:");
    println!(
        "       git submodule add <upstream-url> vendor/<name>     # or vendor the files by hand"
    );
    println!("  2. Register the upstream files you depend on:");
    println!("       loom codefile add 'vendor/<name>/**/*.rs'");
    println!("  3. Name what YOUR code needs from the upstream as an intent (this CREATES it):");
    println!("       loom intent add --name \"<what your service relies on>\"");
    println!("  4. Declare each integration point you consume as a surface, bound to its file:");
    println!(
        "       loom surface add --name <Point> --kind sdk_method --codefile vendor/<name>/<file>"
    );
    println!("       (kinds: http | cli | ui_route | message_topic | sdk_method | internal_module | storage)");
    println!("  5. Put the point under contract — a validation that exercises the surface,");
    println!("     linked to the intent from step 3:");
    println!("       loom validation add --name \"<what you rely on>\" --type contract --intent \"<intent from step 3>\"");
    println!("       loom edge call \"<validation name>\" \"<surface name>\"");
    println!("  6. Baseline: sync, then record that the contract holds right now:");
    println!("       loom sync");
    println!("       loom validation verdict \"<validation name>\" passed --evidence \"<how you verified it>\"");
    println!("  7. Later, after the upstream moves (re-pull, rescan for new files, then sync):");
    println!(
        "       git submodule update --remote vendor/<name>     # or update the vendored files"
    );
    println!("       loom codefile rescan     # register any endpoints the upstream just added");
    println!("       loom sync     # → 'integration: N upstream surface(s) changed → M contract(s) need re-verification'");
    println!(
        "       loom next --mode validate     # re-verify each contract against the new upstream"
    );
    println!();
    println!("  Check every integration point is under contract:  loom surface gaps");
}
