use super::*;
use crate::db::queries::comprehensiveness::Ledger;
use crate::db::queries::stats::{Coverage360, CoverageAxis, GraphState};

fn axis(covered: i64, total: i64) -> CoverageAxis {
    CoverageAxis { covered, total }
}

fn ledger(enumerated: usize, discharged: usize) -> Ledger {
    Ledger {
        enumerated,
        discharged,
        owed: Vec::new(),
    }
}

/// The fixture: loom's own frozen graph (2026-06-22). The ladder MUST reproduce
/// the validated rung-vector — Seeded ✓ · Realized ◐ 46/78 · Proven ✗ 0/11 ·
/// Hardened ✓ · Production-ready ✗, focus Realized — or the roll-up is wrong.
#[test]
fn reproduces_loom_frozen_rung_vector() {
    let gs = GraphState {
        vertically_complete: true,
        horizontally_explored: true,
        coverage: Coverage360 {
            realized_leaves: axis(78, 78),
            proven_executed_leaves: axis(46, 78),
            measured_pairs: axis(2112, 2112),
            ..Default::default()
        },
        ..Default::default()
    };
    let entrypoint = axis(856, 856);
    let boundary = axis(0, 0);
    let journey = ledger(11, 0);
    let behavioral = ledger(8, 8);
    let doc_only = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let fp_reasons = vec![
        "phase is 'audit', not 'complete'".to_string(),
        "32 of 78 realized leaves are not EXECUTED-proven".to_string(),
    ];
    let input = LadderInputs {
        gs: &gs,
        entrypoint: &entrypoint,
        boundary: &boundary,
        journey: &journey,
        behavioral: &behavioral,
        open_smells: &[],
        doc_only_realizations: &doc_only,
        inbox_untriaged: 0,
        source_corpus_unresolved: 0,
        planned_leaf_debt: 0,
        fully_proven_ok: false,
        fully_proven_reasons: &fp_reasons,
    };
    let ladder = maturity_ladder(&input);

    assert_eq!(ladder.rungs.len(), 5);
    assert_eq!(ladder.rungs[0].name, "Seeded");
    assert_eq!(ladder.rungs[0].status, RungStatus::Met);
    assert_eq!(ladder.rungs[1].name, "Realized");
    assert_eq!(ladder.rungs[1].status, RungStatus::Partial);
    assert_eq!(ladder.rungs[1].detail, "46/78 implemented leaves");
    assert_eq!(ladder.rungs[2].name, "Proven");
    assert_eq!(ladder.rungs[2].status, RungStatus::Unmet);
    assert_eq!(ladder.rungs[2].detail, "0/11");
    assert_eq!(ladder.rungs[3].name, "Hardened");
    assert_eq!(ladder.rungs[3].status, RungStatus::Met);
    assert_eq!(ladder.rungs[4].name, "Production-ready");
    assert_eq!(ladder.rungs[4].status, RungStatus::Unmet);
    assert_eq!(ladder.focus, Some(1));

    // Realized carries BOTH gaps loudly — the proof gap and the doc-only gap.
    assert!(ladder.rungs[1]
        .reasons
        .iter()
        .any(|r| r.contains("not executed-proven")));
    assert!(ladder.rungs[1].reasons.iter().any(|r| r.contains("doc")));

    let line = ladder.vector_line();
    assert!(line.contains("Realized ◐ 46/78"), "{line}");
    assert!(line.contains("Proven ✗ 0/11"), "{line}");
}

/// A pure library (no user-visible journeys): Proven collapses to N/A — the
/// adaptive 4-rung shape, driven solely by `journey.enumerated == 0`.
#[test]
fn library_collapses_proven_to_not_applicable() {
    let gs = GraphState {
        vertically_complete: true,
        horizontally_explored: true,
        coverage: Coverage360 {
            realized_leaves: axis(10, 10),
            proven_executed_leaves: axis(10, 10),
            measured_pairs: axis(20, 20),
            ..Default::default()
        },
        ..Default::default()
    };
    let entrypoint = axis(40, 40);
    let boundary = axis(0, 0);
    let journey = ledger(0, 0);
    let behavioral = ledger(4, 4);
    let input = LadderInputs {
        gs: &gs,
        entrypoint: &entrypoint,
        boundary: &boundary,
        journey: &journey,
        behavioral: &behavioral,
        open_smells: &[],
        doc_only_realizations: &[],
        inbox_untriaged: 0,
        source_corpus_unresolved: 0,
        planned_leaf_debt: 0,
        fully_proven_ok: true,
        fully_proven_reasons: &[],
    };
    let ladder = maturity_ladder(&input);
    assert_eq!(ladder.rungs[2].name, "Proven");
    assert_eq!(ladder.rungs[2].status, RungStatus::NotApplicable);
    // N/A counts as cleared, so a fully-proven library is Production-ready.
    assert_eq!(ladder.rungs[4].status, RungStatus::Met);
    assert_eq!(ladder.focus, None);
}

/// Everything discharged ⇒ Production-ready Met, focus None.
#[test]
fn all_green_is_production_ready() {
    let gs = GraphState {
        vertically_complete: true,
        horizontally_explored: true,
        coverage: Coverage360 {
            realized_leaves: axis(5, 5),
            proven_executed_leaves: axis(5, 5),
            measured_pairs: axis(9, 9),
            ..Default::default()
        },
        ..Default::default()
    };
    let entrypoint = axis(12, 12);
    let boundary = axis(3, 3);
    let journey = ledger(2, 2);
    let behavioral = ledger(2, 2);
    let input = LadderInputs {
        gs: &gs,
        entrypoint: &entrypoint,
        boundary: &boundary,
        journey: &journey,
        behavioral: &behavioral,
        open_smells: &[],
        doc_only_realizations: &[],
        inbox_untriaged: 0,
        source_corpus_unresolved: 0,
        planned_leaf_debt: 0,
        fully_proven_ok: true,
        fully_proven_reasons: &[],
    };
    let ladder = maturity_ladder(&input);
    assert!(ladder.rungs.iter().all(|r| r.status.cleared()));
    assert_eq!(ladder.rungs[4].status, RungStatus::Met);
    assert_eq!(ladder.focus, None);
}

/// The rung-vector property: focus is the LOWEST unmet rung even when a HIGHER
/// rung is already met (loom is Hardened ✓ before it is Realized).
#[test]
fn focus_is_lowest_unmet_despite_higher_rung_met() {
    let gs = GraphState {
        vertically_complete: true,
        horizontally_explored: true,
        coverage: Coverage360 {
            realized_leaves: axis(78, 78),
            proven_executed_leaves: axis(46, 78), // Realized partial
            measured_pairs: axis(2112, 2112),     // Hardened inputs met
            ..Default::default()
        },
        ..Default::default()
    };
    let entrypoint = axis(856, 856);
    let boundary = axis(0, 0);
    let journey = ledger(11, 0);
    let behavioral = ledger(8, 8);
    let input = LadderInputs {
        gs: &gs,
        entrypoint: &entrypoint,
        boundary: &boundary,
        journey: &journey,
        behavioral: &behavioral,
        open_smells: &[],
        doc_only_realizations: &[],
        inbox_untriaged: 0,
        source_corpus_unresolved: 0,
        planned_leaf_debt: 0,
        fully_proven_ok: false,
        fully_proven_reasons: &[],
    };
    let ladder = maturity_ladder(&input);
    assert_eq!(ladder.rungs[3].status, RungStatus::Met); // Hardened met...
    assert_eq!(ladder.focus, Some(1)); // ...yet focus is Realized (lower).
}
