//! `loom session` — turn zero, before any utterance exists.
//!
//! `loom door` captures and routes a user statement; this command handles the
//! ABSENCE of one: the user said "use loom" / "loom session" / "loom mode" and
//! stopped.
//! Loom cannot read minds, and the doctrine that keeps `loom smells`
//! trustworthy applies here too — pure computation in the tool, judgment in
//! the LLM. So the tool computes the OFFER MENU: every way this session could
//! be spent, each offer backed by a live queue and its count, with exactly
//! one marked recommended. The LLM's job is to ASK — one question, in the
//! user's language, recommendation first — never to guess.
//!
//! The recommendation order encodes one scarcity fact: the USER's presence.
//! Align drift, hypothesis rulings, and blocked proofs are the queues the
//! agent cannot drain alone (the graph cannot read heads, judge adoption, or
//! conjure credentials) — while the user is here, those come first. Build,
//! repair, and discovery can all happen after the user leaves.

use anyhow::Result;

use crate::db::queries::uninspected_outside_queues_from_snapshot;
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{fmt_pulse, pulse_json, Printer};

/// One way to spend the session: the question to put to the user (their
/// language, no jargon), why it is on the table (live counts), and what the
/// LLM runs when the user picks it.
struct Offer {
    ask: String,
    why: String,
    then: &'static str,
}

/// The numbers the menu derives from — separated from the DB so the
/// offer/recommendation logic is testable as pure computation.
struct SessionCounts {
    intents: i64,
    planned: i64,
    needs_change: i64,
    /// failing + needs_reverification across all inspected edge types.
    broken: i64,
    priority_unexplored_pairs: i64,
    align: i64,
    /// Supported hypotheses awaiting the adopt/reject ruling.
    rulings: i64,
    /// Proofs marked blocked — an external prerequisite only the user can provide.
    blocked: i64,
    /// Pending manual_check proofs — the human-judged residue (does it look/feel
    /// right?) a machine cannot settle, incl. visual/aesthetic confirmation. A
    /// manual_check is human-inspected by definition; it's user-gated like align.
    visual_confirm: i64,
    sagas: i64,
    phase: String,
    has_source: bool,
}

const ASK: &str = "What do you want from this session?";

const DOCTRINE: &str = "One question, in the user's language, recommendation first — an offer, \
    never a quiz. If code or the graph can answer it, don't ask (explore instead). A free-form \
    answer is captured through `loom door \"<their words>\"` and then normalized with \
    `loom inbox triage`; \"you decide\" means take the recommended offer and go. Land every \
    conversational fragment before going autonomous — conversation residue is the failure mode.";

fn print_ask_line() {
    println!("ASK: \"{ASK}\" — and offer:");
}

/// Build the menu + the recommended index. Pure: counts in, offers out.
fn offers(c: &SessionCounts) -> (Vec<Offer>, usize) {
    // An initialized but EMPTY graph: nothing to align, fix, or drain — the
    // only two honest offers are "capture your head" and "map the code".
    if c.intents == 0 {
        let mut menu = vec![Offer {
            ask: "Tell me what this product should do — start with the ONE capability that matters most; I'll ask one thing at a time and we'll seed them together".into(),
            why: "empty graph: no intents yet — the interview seeds them ATOMICALLY (one falsifiable criterion each, so no idea arrives too big to verify)".into(),
            then: "loom guide --mode seed  (elicit: one question at a time, land every answer)",
        }];
        if c.has_source {
            menu.push(Offer {
                ask: "Want me to map this codebase first and tell you what it does?".into(),
                why: "source exists on disk but the graph knows none of it".into(),
                then: "loom guide --mode brownfield → loom next  (map, then verify)",
            });
        }
        menu.push(Offer {
            ask: "Or just tell me what's on your mind".into(),
            why: "their words, any altitude — story, complaint, term, question".into(),
            then: "loom door \"<their words>\"  (captures it in Inbox, then routes)",
        });
        // Code on disk → mapping grounds the interview; no code → interview is all there is.
        return (menu, if c.has_source { 1 } else { 0 });
    }

    let mut menu = Vec::with_capacity(9);
    let mut recommended = usize::MAX;
    // The next push becomes the recommendation — first marker wins.
    macro_rules! recommend_next {
        () => {
            if recommended == usize::MAX {
                recommended = menu.len();
            }
        };
    }

    // --- User-gated queues first: only the user can answer these. ---------
    if c.align > 0 {
        recommend_next!();
        menu.push(Offer {
            ask: "Want me to check we're still aligned — that the map still says what you want?".into(),
            why: format!(
                "{} drift suspect(s): meanings that changed or went unaffirmed since you last confirmed them",
                c.align
            ),
            then: "loom next --mode align  (re-affirm or redefine, one meaning at a time)",
        });
    }
    if c.rulings > 0 {
        recommend_next!();
        menu.push(Offer {
            ask: "Some improvement proposals are proven and waiting on your ruling — decide now?".into(),
            why: format!("{} supported hypothesis(es) await adopt/reject", c.rulings),
            then: "loom hypothesis list --status supported → loom hypothesis adopt <id> --spawned <planned-intent> | reject <id> --reason \"…\"",
        });
    }
    if c.blocked > 0 {
        recommend_next!();
        menu.push(Offer {
            ask: "Some proofs are blocked on something only you can provide — unblock them?".into(),
            why: format!(
                "{} blocked proof(s), each with its recorded prerequisite (env, credentials, a live target)",
                c.blocked
            ),
            then: "loom validation list  (blocked ones name their prerequisite) → provide it → loom validate --all",
        });
    }
    if c.visual_confirm > 0 {
        recommend_next!();
        menu.push(Offer {
            ask: "Some manual confirmations await your judgment — including the visual/aesthetic 'does it look right?' pass. Go through them?".into(),
            why: format!(
                "{} pending manual_check proof(s) — the human-judged residue a machine can't settle, batched for when you're here",
                c.visual_confirm
            ),
            then: "loom validation list  (manual_check, not_run) → loom validation mark <id> --result passed|failed --evidence \"…\"",
        });
    }

    // --- The standing offers. ---------------------------------------------
    if c.planned + c.needs_change > 0 {
        if c.phase == "realize" {
            recommend_next!();
        }
        menu.push(Offer {
            ask: "Want me to keep building?".into(),
            why: format!("{} planned · {} needs_change", c.planned, c.needs_change),
            then: "loom next --mode build  (construct, ground, hand off for verification)",
        });
    }
    if c.phase == "green" {
        // Everything green: enrichment is the best standing offer.
        recommend_next!();
    }
    menu.push(Offer {
        ask: "Want me to propose a user story and prove it end-to-end?".into(),
        why: if c.sagas > 0 {
            format!("{} saga(s) exist — composition coverage can grow", c.sagas)
        } else {
            "no sagas yet — nothing proves the steps compose for a real consumer".into()
        },
        then: "draft the YAML chain → loom saga add <spec.yaml> [--spawn-missing] → loom saga run <name>",
    });
    if c.broken + c.priority_unexplored_pairs > 0 {
        menu.push(Offer {
            ask: "Want me to close gaps — repair broken claims, deepen the map?".into(),
            why: format!(
                "{} failing/stale claim(s) · {} priority unexplored pair(s)",
                c.broken, c.priority_unexplored_pairs
            ),
            then: "loom next --all  (the closeout view) → drain the served lanes",
        });
    }
    menu.push(Offer {
        ask: "Want a read on where things stand?".into(),
        why: "answers only — nothing lands".into(),
        then: "loom status · loom report · loom smells",
    });
    menu.push(Offer {
        ask: "Something specific on your mind? Just say it".into(),
        why: "their words, any altitude — story, complaint, term, question".into(),
        then: "loom door \"<their words>\"  (captures it in Inbox, then routes)",
    });
    // Nothing user-gated, no build backlog, not complete → honest default:
    // the agent can work alone; recommend exactly that.
    recommend_next!();
    menu.push(Offer {
        ask: "Or should I just get to work?".into(),
        why: "everything currently queued is drainable without you".into(),
        then:
            "loom status → loom next --mode <lane>  (user-gated queues wait for your next session)",
    });
    (menu, recommended)
}

/// The pre-init menu: no graph exists here at all. Computed without a DB.
fn offers_uninitialized(has_source: bool, has_export: bool) -> (Vec<Offer>, usize) {
    let mut menu = Vec::with_capacity(3);
    if has_export {
        menu.push(Offer {
            ask: "A committed loom graph travels with this repo — restore it?".into(),
            why: "loom.graph.json exists but no .loom/ database does".into(),
            then: "loom import loom.graph.json → loom sync  (re-checks every claim against today's code)",
        });
    }
    if has_source {
        menu.push(Offer {
            ask: "Want me to map this codebase and tell you what it does?".into(),
            why: "source exists on disk; no graph yet".into(),
            then: "loom init . → loom guide --mode brownfield",
        });
    }
    menu.push(Offer {
        ask: "Tell me what you want to build — I'll capture it as we talk".into(),
        why: "start from your head; the interview seeds the graph".into(),
        then: "loom init . → loom guide --mode seed",
    });
    (menu, 0) // built in priority order: import > map > interview
}

fn print_menu(menu: &[Offer], recommended: usize) {
    for (i, o) in menu.iter().enumerate() {
        let marker = if i == recommended { '▸' } else { '·' };
        println!("  {marker} \"{}\"", o.ask);
        println!("      why:  {}", o.why);
        println!("      then: {}", o.then);
    }
}

fn menu_json(menu: &[Offer], recommended: usize) -> serde_json::Value {
    menu.iter()
        .enumerate()
        .map(|(i, o)| {
            serde_json::json!({
                "offer": o.ask, "why": o.why, "then": o.then,
                "recommended": i == recommended,
            })
        })
        .collect::<Vec<_>>()
        .into()
}

pub fn run(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let has_source = crate::repo::detect(&cwd)?.has_source;

    // Pre-init is a first-class state, not an error: "use loom" is exactly
    // when a graph might not exist yet.
    if !crate::db::loom_dir(&cwd).exists() {
        let has_export = cwd.join("loom.graph.json").is_file();
        let (menu, recommended) = offers_uninitialized(has_source, has_export);
        if printer.json {
            printer.print_json(&serde_json::json!({
                "directive": "No graph here yet. Ask the user ONE question before acting; lead with the recommended offer.",
                "ask": ASK,
                "offers": menu_json(&menu, recommended),
                "doctrine": DOCTRINE,
                "graph_state": serde_json::Value::Null,
            }));
            return Ok(());
        }
        println!("── loom session — turn zero (no graph yet) ────────────────────────");
        println!();
        println!("  The user opened a loom session but no graph exists here. Don't");
        println!("  guess — ASK, then set up accordingly.");
        println!();
        print_ask_line();
        print_menu(&menu, recommended);
        println!();
        println!("  {DOCTRINE}");
        return Ok(());
    }

    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    printer: &Printer,
) -> Result<()> {
    let has_source = crate::repo::detect(root)?.has_source;
    let snapshot = db.query_snapshot()?;
    let gs = db.graph_state(&snapshot)?;

    let broken = snapshot
        .relates
        .iter()
        .map(|e| e.inspection_status.as_str())
        .chain(
            snapshot
                .implements
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .governs
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .chain(
            snapshot
                .validates
                .iter()
                .map(|e| e.inspection_status.as_str()),
        )
        .filter(|s| *s == "failing" || *s == "needs_reverification")
        .count() as i64;
    // Same agenda computation as `loom status` (the oscillation summary):
    // supported hypotheses still in the prove queue are the prover's, not the
    // user's — only the remainder awaits a ruling.
    let prove = db.prove_candidates(&snapshot)?;
    let in_prove: std::collections::HashSet<&str> =
        prove.iter().map(|(h, _)| h.id.as_str()).collect();
    let rulings = db
        .list_hypotheses(Some("supported"))?
        .iter()
        .filter(|h| !in_prove.contains(h.id.as_str()))
        .count() as i64;
    let outside = uninspected_outside_queues_from_snapshot(&snapshot);

    let counts = SessionCounts {
        intents: gs.intents,
        planned: snapshot
            .intents
            .iter()
            .filter(|i| i.lifecycle == "planned")
            .count() as i64,
        needs_change: snapshot
            .intents
            .iter()
            .filter(|i| i.lifecycle == "needs_change")
            .count() as i64,
        broken,
        priority_unexplored_pairs: gs.priority_unexplored_pairs,
        align: db.align_candidate_count(&snapshot)?,
        rulings,
        blocked: outside.blocked_validations,
        visual_confirm: snapshot
            .validations
            .iter()
            .filter(|v| {
                v.validation_type == "manual_check"
                    && (v.last_result.is_empty() || v.last_result == "not_run")
            })
            .count() as i64,
        sagas: snapshot
            .validations
            .iter()
            .filter(|v| v.validation_type == "saga")
            .count() as i64,
        phase: gs.phase.clone(),
        has_source,
    };
    let (menu, recommended) = offers(&counts);

    if printer.json {
        printer.print_json(&serde_json::json!({
            "directive": "The user opened a loom session without stating a goal. Ask ONE question before acting; lead with the recommended offer.",
            "ask": ASK,
            "offers": menu_json(&menu, recommended),
            "doctrine": DOCTRINE,
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!("── loom session — turn zero ───────────────────────────────────────");
    println!();
    println!("  The user opened a loom session without stating a goal. Loom cannot");
    println!("  read minds — ASK before acting.");
    println!();
    print_ask_line();
    print_menu(&menu, recommended);
    println!();
    println!("  {DOCTRINE}");
    println!();
    println!("  {}", fmt_pulse(&gs));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{offers, offers_uninitialized, SessionCounts};

    fn counts() -> SessionCounts {
        SessionCounts {
            intents: 10,
            planned: 0,
            needs_change: 0,
            broken: 0,
            priority_unexplored_pairs: 0,
            align: 0,
            rulings: 0,
            blocked: 0,
            visual_confirm: 0,
            sagas: 0,
            phase: "harden".into(),
            has_source: true,
        }
    }

    /// The user's presence is the scarce resource: any user-gated queue beats
    /// the build backlog, in align > rulings > blocked order.
    #[test]
    fn user_gated_work_outranks_build() {
        let mut c = counts();
        c.planned = 5;
        c.phase = "realize".into();
        c.align = 2;
        c.rulings = 1;
        c.blocked = 1;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("--mode align"));

        c.align = 0;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("hypothesis"));

        c.rulings = 0;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("validation list"));

        c.blocked = 0;
        c.visual_confirm = 2;
        let (menu, rec) = offers(&c);
        assert!(
            menu[rec].then.contains("manual_check"),
            "pending manual confirmations are user-gated, ahead of the build backlog"
        );

        c.visual_confirm = 0;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("--mode build"));
    }

    /// Nothing user-gated, no build backlog, work remains → the honest
    /// recommendation is autonomous handoff, and the menu still carries the
    /// gap-closing offer.
    #[test]
    fn drainable_backlog_recommends_handoff() {
        let mut c = counts();
        c.broken = 3;
        c.priority_unexplored_pairs = 7;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("loom next --mode <lane>"));
        assert!(menu.iter().any(|o| o.then.contains("loom next --all")));
    }

    /// All green → enrichment: propose a saga rather than idle.
    #[test]
    fn complete_graph_recommends_saga_proposal() {
        let mut c = counts();
        c.phase = "green".into();
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("loom saga add"));
    }

    /// Empty graph: interview or map, picked by whether source exists; the
    /// count-gated offers (align/build/gaps) must not appear.
    #[test]
    fn empty_graph_routes_by_source() {
        let mut c = counts();
        c.intents = 0;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("brownfield"));
        c.has_source = false;
        let (menu, rec) = offers(&c);
        assert!(menu[rec].then.contains("--mode seed"));
        assert!(menu.iter().all(|o| !o.then.contains("--mode build")));
        // R6: the seed offer scaffolds a novice — ONE capability at a time,
        // seeded atomically — instead of the terse "tell me what this should be".
        assert!(
            menu[rec].ask.contains("ONE capability")
                && menu[rec].ask.contains("one thing at a time"),
            "the seed offer scaffolds the interview: {}",
            menu[rec].ask
        );
        assert!(
            menu[rec].why.contains("ATOMICALLY"),
            "the seed offer promises atomic seeding: {}",
            menu[rec].why
        );
    }

    /// Pre-init: a committed export outranks mapping outranks interviewing.
    #[test]
    fn uninitialized_prefers_import() {
        let (menu, rec) = offers_uninitialized(true, true);
        assert!(menu[rec].then.contains("loom import"));
        let (menu, rec) = offers_uninitialized(true, false);
        assert!(menu[rec]
            .then
            .contains("loom init . → loom guide --mode brownfield"));
        let (menu, rec) = offers_uninitialized(false, false);
        assert!(menu[rec].then.contains("--mode seed"));
    }
}
