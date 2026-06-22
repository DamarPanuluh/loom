use anyhow::Result;

use crate::db::queries::{
    blocked_validation_summary_from_snapshot, build_candidates_from_snapshot, parse_sync_cause,
    quality_candidates_from_snapshot, review_candidates_from_snapshot,
    scored_candidates_from_snapshot, unexplored_pairs_scored_from_snapshot,
    validate_candidates_from_snapshot, vertical_completeness_from_snapshot, AlignCandidate,
    DiscoveryClassFilter, DoctorReport, GraphState, QuerySnapshot, Smell,
};
use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::{
    fmt_edge_detail, fmt_intent_surface, fmt_pulse, more_marker, note_list_intent_command,
    pulse_json, Printer, SECTION_CAP,
};
use crate::types::{
    EdgeType, GroundingSurface, Hypothesis, IntentSurface, ValidationSurface, WorkItem,
};

mod align;
mod modes;
mod quality;
mod relates;
mod render;
mod review;
mod scoring;

use align::{run_align, run_take_align};
use modes::{run_build, run_prove, run_validate};
use quality::{run_quality, run_take_quality};
use relates::run_relates_with_repo;
use render::run_all;
use review::{run_review, run_take_review};
const QUALITY_EMPTY_MESSAGE: &str =
    "No uninspected, failing, or stale GOVERNS edges — the green gate holds.";
const ALIGN_EMPTY_MESSAGE: &str =
    "No drift suspected — nothing churned under a fresh meaning, no wording past its re-affirmation grace. The interview is done.";
const BATCH_TEMPLATE_TITLE: &str =
    "── Batch template (edit per finding, then paste into `loom batch - <<'EOF' … EOF`) ──";

/// Hints printed above every `--take N` batch template (and emitted in the
/// JSON `batch_template_hints` field). The template is deliberately NOT
/// paste-ready. `confidence` is a placeholder the batch gate rejects unedited,
/// so a verbatim paste stamps zero verdicts, not N false 0.9 grounds (no blind
/// re-ground). The per-op field legend names each op's REQUIRED fields, so a
/// driver who switches op (e.g. `ground` to `independent`) sees that
/// `independent` takes `notes`, not `evidence`, before they fail. The dry-run
/// guardrail is surfaced inline so a large batch can be checked before commit.
const BATCH_TEMPLATE_HINTS: [&str; 3] = [
    "per-op required fields: ground→a,b,confidence(+criterion unless stored; +optional evidence) · issue→a,b,evidence,confidence(+criterion unless stored) · independent→a,b,notes · rule_verdict→rule,intent,status,evidence,confidence(+criterion unless stored)",
    "confidence is a <placeholder> on every line below — fill a real [0,1] judgment per line; a verbatim paste is rejected (no blind re-ground).",
    "validate before committing: paste the same lines through `loom batch - --dry-run` (nothing written).",
];

fn print_batch_template_header() {
    println!("{BATCH_TEMPLATE_TITLE}");
    for hint in BATCH_TEMPLATE_HINTS {
        println!("  # {hint}");
    }
}

struct NextOpts<'a> {
    mode: &'a str,
    all: bool,
    take: usize,
    discovery_class: Option<&'a str>,
    compact: bool,
    /// `loom-dx #4`: Some(note) when `--take` was passed on a
    /// one-command-per-item mode (build/populate/validate/prove) and capped to
    /// a single item. The driver asked for N and got one — the note makes the
    /// cap VISIBLE (a silent cap is the trap this card exists to close).
    /// Carried into each non-bulk renderer so both the human line and the JSON
    /// `take_note` field surface it (human/json parity).
    take_note: Option<String>,
}

/// `loom-dx #6`: the default mode when `--mode` is omitted follows the compass
/// phase — bare `loom next` serves the phase's lane instead of always
/// discovery. The phase is computed from the same snapshot the queues score,
/// so a mapped phase's lane is guaranteed non-empty (`queue_nonempty_for_phase`
/// in stats.rs asserts this invariant). Phases whose action is NOT a
/// `loom next --mode` lane (seed → guide, incomplete/audit → doctor, ground →
/// edge implement/coverage) fall back to discovery — the honest exploration
/// default; `loom status`'s `next_action` carries the real directive for those.
fn phase_default_mode(phase: &str) -> &'static str {
    match phase {
        "build" => "build",
        "fix" => "fix",
        "validate" => "validate",
        "quality" => "quality",
        "discovery" => "discovery",
        _ => "discovery",
    }
}

/// Bare `loom next` in phase=audit: there is no `--mode audit` queue, so echo the
/// compass's own audit directive (which points at `loom smells`) rather than
/// mis-routing the driver to OPTIONAL discovery while green-blocking findings sit
/// unadjudicated.
fn emit_audit_directive(gs: &GraphState, printer: &Printer) -> Result<()> {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "phase": "audit",
            "next_kind": gs.next_kind,
            "next_action": gs.next_action,
            "note": "phase=audit has no `loom next` queue — `loom smells --summary` is the audit surface; every open finding gates green until resolved (fix or a finding-specific decision note).",
        }));
    } else {
        println!("── Next: phase=audit (the green-blocking gate) ──────────────────────");
        let arrow = if gs.next_kind == "directive" {
            "→ Next"
        } else {
            "→ Recommended"
        };
        println!("  {arrow}: {}", gs.next_action);
        println!(
            "  (`loom next` serves no audit queue — `loom smells --summary` is the audit surface; resolve each finding to reach green.)"
        );
    }
    Ok(())
}

/// `loom-dx #4`: stamp the take-cap note into a non-bulk renderer's JSON
/// envelope. `take_note` is Some only when --take was capped to 1; the field
/// pair (`take_note` + `take_capped_to`) makes the cap visible to JSON agents
/// — the audience that most needs it (a silent cap is the trap this closes).
pub(crate) fn inject_take_note(
    mut v: serde_json::Value,
    take_note: Option<&str>,
) -> serde_json::Value {
    if let Some(note) = take_note {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("take_note".to_string(), note.into());
            obj.insert("take_capped_to".to_string(), 1.into());
        }
    }
    v
}

pub fn run(
    mode: Option<&str>,
    all: bool,
    take: Option<usize>,
    discovery_class: Option<&str>,
    compact: bool,
    printer: &Printer,
) -> Result<()> {
    // An explicit `--take 0` (e.g. a programmatically-computed zero-size chunk)
    // used to silently fall back to the single-item schema — a different shape
    // with no take signal. Reject it; omitting --take is the single-item path.
    if take == Some(0) {
        anyhow::bail!(
            "`--take 0` requests zero items — omit --take for the single top item, or pass --take N (N≥1) for a bulk read."
        );
    }
    let take = take.unwrap_or(0);
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    // #6: omit --mode → follow the compass phase.
    let (mode, from_phase) = match mode {
        Some(m) => (m.to_string(), false),
        None => {
            let snap = store.query_snapshot()?;
            let gs = store.graph_state(&snap)?;
            // phase=audit gates green but has no `--mode audit` queue — echo the
            // compass's audit directive (→ `loom smells`).
            if gs.phase == "audit" {
                return emit_audit_directive(&gs, printer);
            }
            // Route by the maturity ladder's FOCUS rung — the authoritative
            // (stage, lane) signal — when its lane is a `--mode` queue; else fall
            // back to the cascade default. Fixes the cascade under-routing (e.g.
            // phase=complete while the ladder focus is Realized · validate).
            let decision_notes = store.notes_by_kind("decision")?;
            let open_smells = if matches!(gs.phase.as_str(), "audit" | "complete") {
                store.smell_report(&snap)?.open
            } else {
                Vec::new()
            };
            let inbox_untriaged = store
                .list_inbox_items(None, None)?
                .iter()
                .filter(|i| i.status == "new")
                .count();
            let export_stale = store.committed_export_stale(&cwd)? == Some(true);
            let focus_lane = crate::db::queries::build_ladder(
                &cwd,
                &snap,
                &gs,
                &decision_notes,
                &open_smells,
                inbox_untriaged,
                export_stale,
            )
            .ladder
            .focus_lane();
            let mode = match focus_lane {
                Some(l @ ("build" | "fix" | "validate" | "quality" | "discovery")) => l.to_string(),
                _ => phase_default_mode(&gs.phase).to_string(),
            };
            (mode, true)
        }
    };
    // #4: --take on a one-command-per-item mode caps to 1 (those queues aren't
    // bulkable) — and the cap is announced, not silent. The bulk modes keep
    // --take as a real bulk read.
    let bulk = matches!(
        mode.as_str(),
        "discovery" | "fix" | "quality" | "align" | "review"
    );
    let (take, take_note) = if take > 0 && !bulk {
        (
            1,
            Some(
                "--take is a bulk read of the discovery/fix/quality/align/review queues; \
                 {mode} resolves one command per item, so serving the top 1. For the full \
                 queue overview: `loom next --all`."
                    .replace("{mode}", &mode),
            ),
        )
    } else {
        (take, None)
    };
    let _ = from_phase; // reserved: the `mode` field already names the lane served.
    run_with_repo(
        &store,
        &cwd,
        &NextOpts {
            mode: &mode,
            all,
            take,
            discovery_class,
            compact,
            take_note,
        },
        printer,
    )
}

fn run_with_repo(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    opts: &NextOpts<'_>,
    printer: &Printer,
) -> Result<()> {
    let mode = opts.mode;
    let all = opts.all;
    let take = opts.take;
    let discovery_class = opts.discovery_class;
    let compact = opts.compact;
    let take_note = opts.take_note.as_deref();
    if all {
        return run_all(db, root, printer);
    }
    if mode == "triage" {
        anyhow::bail!(
            "Mode 'triage' was renamed to 'prove' (it proves proposed hypotheses; \
             'triage' now belongs to Inbox — `loom door \"<utterance>\"` captures \
             user input, then `loom inbox triage` routes it). Run: loom next --mode prove"
        );
    }
    if !matches!(
        mode,
        "discovery"
            | "fix"
            | "build"
            | "populate"
            | "validate"
            | "align"
            | "quality"
            | "review"
            | "prove"
    ) {
        anyhow::bail!(
            "Unknown mode '{}'. Valid values: discovery, fix, build, populate, validate, align, quality, review, prove
\
             discovery = inspect relationships (analyzer) · fix = resolve failures/stale · \
             build = realize planned/needs_change intents (builder) · \
             populate = backfill derived graph structure (builder) · \
             validate = run/repair proofs (validator) · \
             align = re-affirm intent meaning against the USER (validator; serves intents whose code churned since the user last confirmed their meaning — the user↔intent drift check) · \
             quality = earn GOVERNS green (quality) · review = re-inspect LOW-CONFIDENCE verdicts (the tiered double-check; resolves by \
             re-recording with confidence ≥ 0.7 or overturning) · \
             prove = prove PROPOSED hypotheses (analyzer; the pre-decision plane — optional).",
            mode
        );
    }

    // `loom-dx #4`: --take on a one-command-per-item mode used to hard-error.
    // It now caps to 1 (run() already clamped `take` + built `take_note`); the
    // cap is announced below, not silent. The bulk modes keep --take as-is.

    if discovery_class.is_some() && mode != "discovery" {
        anyhow::bail!(
            "--class only applies to generated discovery pairs. Use it with \
             `loom next --mode discovery --class suspected-coupling|impact-map|all`."
        );
    }
    let discovery_class = DiscoveryClassFilter::parse(discovery_class)?;

    if compact && !matches!(mode, "discovery" | "fix") {
        anyhow::bail!(
            "--compact projects a RELATES_TO work item down to its verdict coordinates \
             (intents, edge id, grounded paths, the command) — it serves the discovery/fix \
             queues. The other modes' items are already mode-shaped — use `loom next --mode {mode}`."
        );
    }

    // #4 human legibility: announce the cap once, above whichever non-bulk
    // renderer we dispatch to (JSON paths carry `take_note` in their envelope).
    if !printer.json {
        if let Some(note) = take_note {
            println!("  note: {note}");
            println!();
        }
    }

    match mode {
        "build" => return run_build(db, take_note, printer),
        "populate" => return crate::commands::populate::render_next(db, root, take_note, printer),
        "validate" => return run_validate(db, take_note, printer),
        "align" => {
            return if take > 0 {
                run_take_align(db, take, printer)
            } else {
                run_align(db, printer)
            }
        }
        "quality" => {
            return if take > 0 {
                run_take_quality(db, take, printer)
            } else {
                run_quality(db, printer)
            }
        }
        "review" => {
            return if take > 0 {
                run_take_review(db, take, printer)
            } else {
                run_review(db, printer)
            }
        }
        "prove" => return run_prove(db, take_note, printer),
        _ => {}
    }

    run_relates_with_repo(db, mode, take, discovery_class, compact, printer)
}

/// Bound a sub-list rendered inside a work item at SECTION_CAP.
/// Returns the pre-cap total for the caller's marker/`*_total` fields.
pub(super) fn cap_section<T>(items: &mut Vec<T>) -> usize {
    let total = items.len();
    items.truncate(SECTION_CAP);
    total
}

/// The work-item note pipeline: collapse repeated (kind, text) notes into one
/// surface carrying a count (sync re-flips spam identical transition text —
/// the count IS the information, the copies are not), put notes addressed to
/// `role` first (directed handoffs beat ambient memory; stable within groups,
/// chronological order preserved), cap at SECTION_CAP (addressed notes keep
/// priority; remaining slots go to the NEWEST ambient notes). Returns the
/// surfaces + the pre-cap unique total for the caller's marker/`*_total`.
pub(super) fn note_surfaces(
    notes: Vec<crate::types::Note>,
    role: &str,
) -> (Vec<crate::types::NoteSurface>, usize) {
    // A RESOLVED todo has left the backlog — it no longer surfaces as work. An
    // OPEN todo (and every non-todo note, which never carries a resolution)
    // stays: that persistence is the point — loom holds the string so a compacted
    // agent can't silently drop it. It leaves only when consciously resolved.
    let notes: Vec<crate::types::Note> = notes
        .into_iter()
        .filter(|n| n.resolution.is_empty())
        .collect();
    // Dedup: the first occurrence keeps the slot (input is chronological).
    let mut uniq: Vec<(crate::types::Note, u32)> = Vec::new();
    for n in notes {
        match uniq
            .iter_mut()
            .find(|(u, _)| u.kind == n.kind && u.text == n.text)
        {
            Some((_, c)) => *c += 1,
            None => uniq.push((n, 1)),
        }
    }
    let total = uniq.len();
    uniq.sort_by_key(|(n, _)| if n.audience == role { 0 } else { 1 });
    if total > SECTION_CAP {
        let addressed = uniq.iter().take_while(|(n, _)| n.audience == role).count();
        if addressed >= SECTION_CAP {
            uniq.truncate(SECTION_CAP);
        } else {
            uniq.drain(addressed..total - (SECTION_CAP - addressed));
        }
    }
    let surfaces = uniq
        .into_iter()
        .map(|(n, times)| crate::types::NoteSurface {
            kind: n.kind,
            text: n.text,
            author: n.author,
            audience: n.audience,
            times,
        })
        .collect();
    (surfaces, total)
}

#[cfg(test)]
mod tests {
    use super::scoring::build_suggested_action_compact;
    use super::{note_surfaces, phase_default_mode};

    #[test]
    fn phase_default_mode_maps_each_lane_and_falls_back_to_discovery() {
        // loom-dx #6: the five lanes whose compass phase names a `loom next
        // --mode` queue map to themselves; the rest (seed/incomplete/ground/
        // audit/complete — actions that are NOT a next-mode) fall back to
        // discovery, the honest exploration default.
        for lane in ["build", "fix", "validate", "quality", "discovery"] {
            assert_eq!(phase_default_mode(lane), lane);
        }
        for non_lane in ["seed", "incomplete", "ground", "audit", "complete", "??"] {
            assert_eq!(
                phase_default_mode(non_lane),
                "discovery",
                "{non_lane} has no next-mode lane → discovery fallback"
            );
        }
    }

    fn note(kind: &str, text: &str, audience: &str) -> crate::types::Note {
        crate::types::Note {
            id: format!("{kind}:{text}"),
            kind: kind.to_string(),
            text: text.to_string(),
            author: "loom".to_string(),
            target_kind: "edge".to_string(),
            target_id: "e".to_string(),
            resolution: String::new(),
            audience: audience.to_string(),
            created_at: "t".to_string(),
        }
    }

    #[test]
    fn repeated_notes_collapse_into_a_count() {
        let notes = vec![
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
            note("transition", "needs_reverification → passing", ""),
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
            note(
                "transition",
                "passing → needs_reverification (sync: a.rs changed)",
                "",
            ),
        ];
        let (surfaces, total) = note_surfaces(notes, "analyzer");
        assert_eq!(total, 2, "total counts UNIQUE notes");
        assert_eq!(surfaces.len(), 2);
        assert_eq!(surfaces[0].times, 3, "the flap count is the signal");
        assert_eq!(surfaces[1].times, 1);
    }

    #[test]
    fn addressed_notes_survive_the_cap() {
        // 12 ambient notes + 1 directed handoff buried at the end.
        let mut notes: Vec<_> = (0..12)
            .map(|i| note("commentary", &format!("ambient {i}"), ""))
            .collect();
        notes.push(note("decision", "directed handoff", "analyzer"));
        let (surfaces, total) = note_surfaces(notes, "analyzer");
        assert_eq!(total, 13);
        assert_eq!(surfaces.len(), crate::output::SECTION_CAP);
        assert_eq!(
            surfaces[0].text, "directed handoff",
            "addressed-to-role notes surface first"
        );
        assert_eq!(
            surfaces.last().unwrap().text,
            "ambient 11",
            "remaining slots go to the newest ambient notes"
        );
    }

    #[test]
    fn compact_action_is_one_runnable_line() {
        let mut edge = crate::types::RelatesTo {
            id: "rt:a:b".to_string(),
            from_id: "a".to_string(),
            to_id: "b".to_string(),
            from_name: "A".to_string(),
            to_name: "B".to_string(),
            inspection_status: String::new(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
            kinds: Vec::new(),
            stable: false,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        };
        for status in [
            "unexplored",
            "uninspected",
            "failing",
            "needs_reverification",
        ] {
            edge.inspection_status = status.to_string();
            let action = build_suggested_action_compact(&edge);
            assert!(
                !action.contains('\n'),
                "[{status}] compact action must be one line: {action}"
            );
            assert!(
                action.contains("loom "),
                "[{status}] must carry a runnable command: {action}"
            );
        }
        edge.inspection_status = "failing".to_string();
        assert!(
            build_suggested_action_compact(&edge).contains("rt:a:b"),
            "failing routes through `loom edge fix <id>`"
        );
    }
}
