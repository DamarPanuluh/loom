//! `loom door` — the entrance: capture first, then route.
//!
//! A user utterance arrives at an arbitrary altitude — a story, a complaint,
//! a norm, a term, a question. Door persists that raw language as an InboxItem
//! before any graph noun is created. Loom never interprets it (the doctrine
//! that keeps `loom smells` trustworthy applies verbatim: pure computation in
//! the tool, judgment in the LLM). The door then assembles the ROUTING CONTEXT
//! mechanically: what every plane already knows about the topic, the compass
//! state, and the LANDING MENU — the total enumeration of ways an utterance
//! becomes a graph noun, each an existing command.
//!
//! Two contracts make the corridor clear:
//! - CAPTURE FIRST: every utterance gets a durable InboxItem id before any
//!   graph noun can be created from it.
//! - PROPOSE, THEN ROUTE: door/triage only produce context and command shapes;
//!   existing graph commands still perform the actual mutation.

use anyhow::Result;

use crate::db::queries::{DoorMatches, FindHit, GraphState};
use crate::output::{fmt_pulse, pulse_json, Printer};
use crate::types::InboxItem;

/// The landing menu: (utterance class, what lands, the exact command shape).
/// Total over the noun set — extend it when a new noun is born, never let an
/// utterance class go unlisted.
const LANDINGS: &[(&str, &str, &str)] = &[
    (
        "new desired behavior",
        "Intent (planned)",
        "loom intent add --name … --description … --level feature --lifecycle planned [--aspect happy|sad|fallback]  → then loom edge hierarchy <parent> <child>",
    ),
    (
        "a user journey / story walkthrough",
        "Saga (consumer journey)",
        "write the YAML chain (each step binds to the intent it exercises) → loom saga add <spec.yaml> [--spawn-missing [--under <component>]]  (steps may name not-yet-existing intents — they spawn as planned features)",
    ),
    (
        "complaint about existing behavior",
        "Lifecycle flag",
        "loom intent mark <id> --lifecycle needs_change --reason \"…\"  (the build queue surfaces it first)",
    ),
    (
        "redesign idea / recurring breakage",
        "Hypothesis (pre-decision)",
        "loom hypothesis add --name … --claim … --proposal … --predicted-outcome … --target <intent>  (a DIFFERENT agent proves it via loom next --mode prove)",
    ),
    (
        "a norm or standard to enforce",
        "QualityRule",
        "loom rule add --name … --description … --severity warning|error  → loom rule apply / loom rule verdict",
    ),
    (
        "a term of art / naming decision",
        "VocabTerm",
        "loom vocab add <term> --why \"covers X, NOT Y\"  (collides with a registered term? loom vocab merge)",
    ),
    (
        "the meaning of an existing intent changed",
        "Intent update (ripples)",
        "loom intent update <id> --description … --reason …  (claims earned against the old meaning go stale — that is the point)",
    ),
    (
        "a tradeoff was decided / scope explicitly declined",
        "Decision note",
        "loom note add --intent <id> --kind decision --text \"…\"  (silence and decision must never look alike)",
    ),
    (
        "a question about the system",
        "no landing",
        "answer from the matches above (loom intent show <id> · loom report · loom coverage); nothing lands",
    ),
    (
        "\"go work\" — hand off to autonomous draining",
        "the queues",
        "loom status → loom next --mode <lane>  (human-gated queues wait; batch them for the next conversation)",
    ),
];

const DOCTRINE: &str = "The door captures first, then advises — the raw utterance is now an \
    InboxItem, not graph truth. Normalize it with `loom inbox normalize <id> …`, run the proposed \
    graph command separately, then `loom inbox mark <id> --status routed --reason \"…\"`. Before \
    going autonomous, sweep: every conversational fragment must have landed or been rejected.";

pub fn run(utterance: &str, limit: usize, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    run_with_db(&store, &cwd, utterance, limit, printer)
}

pub fn run_with_db(
    db: &crate::db::sqlite::SqliteGraphStore,
    _root: &std::path::Path,
    utterance: &str,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    let inbox_item = crate::commands::inbox::create_item(
        db,
        utterance.to_string(),
        "user".to_string(),
        Vec::new(),
        Vec::new(),
        None,
    )?;
    let (intents, _match_total) = db.find_intents(utterance, limit)?;
    let planes = db.door_matches(utterance, limit)?;
    let snapshot = db.query_snapshot()?;
    let gs = db.graph_state(&snapshot)?;
    render(utterance, inbox_item, intents, planes, gs, printer)
}

fn render(
    utterance: &str,
    inbox_item: InboxItem,
    intents: Vec<FindHit>,
    planes: DoorMatches,
    gs: GraphState,
    printer: &Printer,
) -> Result<()> {
    let nothing_known = intents.is_empty()
        && planes.vocab.is_empty()
        && planes.sagas.is_empty()
        && planes.rules.is_empty();

    if printer.json {
        printer.print_json(&serde_json::json!({
            "utterance": utterance,
            "inbox_item": inbox_item,
            "matches": {
                "intents": intents.iter().map(|h| serde_json::json!({
                    "id": h.intent.id,
                    "name": h.intent.name,
                    "description": h.intent.description,
                    "level": h.intent.abstraction_level,
                    "lifecycle": h.intent.lifecycle,
                    "score": h.score,
                    "parent_chain": h.parent_chain,
                    "groundings": h.groundings.iter().map(|(path, locator)| serde_json::json!({
                        "path": path, "locator": locator,
                    })).collect::<Vec<_>>(),
                    "stale_edges": h.stale_edges,
                })).collect::<Vec<_>>(),
                "vocab": planes.vocab,
                "sagas": planes.sagas,
                "rules": planes.rules,
            },
            "nothing_known": nothing_known,
            "landings": LANDINGS.iter().map(|(when, lands, command)| serde_json::json!({
                "when": when, "lands": lands, "command": command,
            })).collect::<Vec<_>>(),
            "doctrine": DOCTRINE,
            "next_step": format!(
                "loom inbox normalize {} --kind <kind> --claim \"<normalized claim>\" --route <route_kind> --command \"<exact command or answer>\"",
                inbox_item.id
            ),
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── loom door — \"{utterance}\" ─────────────────────────────────────");
    println!("captured inbox item: {}", inbox_item.id);
    println!();
    println!("WHAT THE GRAPH KNOWS");
    if nothing_known {
        println!("  nothing matched — likely NEW scope. Land it (see the menu) and it");
        println!("  becomes findable; or reformulate (the map may use different words).");
    }
    for h in &intents {
        println!(
            "  intent {:>5.2}  [{}/{}] {}  ({})",
            h.score, h.intent.abstraction_level, h.intent.lifecycle, h.intent.name, h.intent.id
        );
        if !h.parent_chain.is_empty() {
            println!("         under: {}", h.parent_chain.join(" › "));
        }
        if h.stale_edges > 0 {
            println!(
                "         ⚠ {} stale claim(s) — code changed since verification",
                h.stale_edges
            );
        }
    }
    for v in &planes.vocab {
        println!("  vocab  '{}' — {}", v.name, v.detail);
    }
    for s in &planes.sagas {
        println!("  saga   '{}' — {}", s.name, s.detail);
    }
    for r in &planes.rules {
        println!("  rule   '{}' — {}", r.name, r.detail);
    }
    println!();
    println!("THE LANDING MENU (pick ONE per utterance; each is an existing command)");
    for (when, lands, command) in LANDINGS {
        println!("  · {when}  →  {lands}");
        println!("      {command}");
    }
    println!();
    println!("  {DOCTRINE}");
    println!(
        "  Normalize: loom inbox normalize {} --kind <kind> --claim \"…\" --route <route_kind> --command \"…\"",
        inbox_item.id
    );
    println!();
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}
