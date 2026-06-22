//! The maturity ladder — loom's SINGLE ordinal "done".
//!
//! A rung-vector rolled up from gates loom already computes (the vertical spine,
//! the comprehensiveness ledgers, `fully_proven_from_state`). It REPLACES the
//! scattered reads (`phase=complete` / `fully_proven` / standalone
//! `loom complete`) as the user-facing completion vocabulary; their math
//! survives here as rung INPUTS — no new schema, no new queries.
//!
//! Ordered by loom's honesty law `RECORD ≠ DISCHARGE`: **Seeded** is
//! RECORD-complete; **Realized → Production-ready** are progressive DISCHARGE.
//! The ladder is a VECTOR (every rung's true state) with a FOCUS (the lowest
//! unmet rung, where work routes) — never a scalar, because the axes are
//! genuinely independent: a repo can be Hardened before it is Proven (loom
//! itself is). A vacuous dimension (denominator 0) is N/A, auto-cleared, never a
//! 0% block — e.g. `boundary` on a repo with no external-service imports.
//!
//! See `docs/maturity-ladder-proposal.md` for the full design.

use serde::Serialize;

use crate::db::queries::comprehensiveness::Ledger;
use crate::db::queries::smells::Smell;
use crate::db::queries::stats::{CoverageAxis, GraphState};
use std::path::Path;

use crate::db::queries::comprehensiveness as comp;
use crate::db::queries::stats::fully_proven_from_state;
use crate::db::queries::symbol_accountability::symbol_accountability_from_parts_with_notes;
use crate::db::queries::QuerySnapshot;
use crate::types::Note;

/// Per-rung state. `NotApplicable` and `Met` are both "cleared" — focus skips
/// them; only `Partial`/`Unmet` draw the focus and route work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RungStatus {
    /// Gate satisfied.
    Met,
    /// Some sub-gates met, some open (e.g. vertical spine done, proofs owed).
    Partial,
    /// Open — nothing (or nearly nothing) discharged.
    Unmet,
    /// Vacuous dimension (denominator 0) — auto-cleared, never blocks.
    NotApplicable,
}

impl RungStatus {
    /// Cleared = does not draw focus. Met or N/A.
    pub fn cleared(self) -> bool {
        matches!(self, RungStatus::Met | RungStatus::NotApplicable)
    }

    pub fn glyph(self) -> &'static str {
        match self {
            RungStatus::Met => "✓",
            RungStatus::Partial => "◐",
            RungStatus::Unmet => "✗",
            RungStatus::NotApplicable => "—",
        }
    }
}

/// One rung of the ladder: its name, state, a compact progress `detail`
/// (e.g. "46/78"), and the falsifiable `reasons` it is not yet cleared (loud,
/// never silent).
#[derive(Debug, Clone, Serialize)]
pub struct Rung {
    pub name: &'static str,
    pub status: RungStatus,
    pub detail: String,
    pub reasons: Vec<String>,
}

/// The ladder: a vector of rungs + the focus (lowest unmet rung index, where
/// `loom next` routes). `focus == None` ⇒ every rung cleared ⇒ Production-ready.
#[derive(Debug, Clone, Serialize)]
pub struct MaturityLadder {
    pub rungs: Vec<Rung>,
    pub focus: Option<usize>,
}

impl MaturityLadder {
    pub fn focus_rung(&self) -> Option<&Rung> {
        self.focus.and_then(|i| self.rungs.get(i))
    }

    /// One-line vector render: `Seeded ✓ · Realized ◐ 46/78 · Proven ✗ 0/11 …`.
    pub fn vector_line(&self) -> String {
        self.rungs
            .iter()
            .map(|r| {
                if r.detail.is_empty() {
                    format!("{} {}", r.name, r.status.glyph())
                } else {
                    format!("{} {} {}", r.name, r.status.glyph(), r.detail)
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// The focus line: the lowest unmet rung + its blocking reasons, or the
    /// production-ready line when every rung is cleared. ONE source of truth for
    /// both `loom status` and `loom complete` — no duplicated contract string.
    pub fn focus_summary(&self) -> String {
        match self.focus_rung() {
            None => "✓ PRODUCTION-READY — proven, comprehensive, durable.".to_string(),
            Some(r) if r.reasons.is_empty() => format!("focus: {} (in progress)", r.name),
            Some(r) => format!("focus: {} — {}", r.name, r.reasons.join("; ")),
        }
    }
}

/// Roll-up inputs — all already computed by the caller (status / complete), so
/// the ladder re-sequences existing evidence and adds no work.
pub struct LadderInputs<'a> {
    pub gs: &'a GraphState,
    pub entrypoint: &'a CoverageAxis,
    pub boundary: &'a CoverageAxis,
    pub journey: &'a Ledger,
    pub behavioral: &'a Ledger,
    pub open_smells: &'a [Smell],
    pub doc_only_realizations: &'a [String],
    pub inbox_untriaged: usize,
    pub fully_proven_ok: bool,
    pub fully_proven_reasons: &'a [String],
}

/// A CoverageAxis is "cleared" when it is vacuous (no denominator) or full.
fn axis_cleared(a: &CoverageAxis) -> bool {
    a.total == 0 || a.covered >= a.total
}

fn graded(name: &'static str, reasons: Vec<String>, detail: String, any_progress: bool) -> Rung {
    let status = if reasons.is_empty() {
        RungStatus::Met
    } else if any_progress {
        RungStatus::Partial
    } else {
        RungStatus::Unmet
    };
    Rung {
        name,
        status,
        detail,
        reasons,
    }
}

/// Compute the ladder from already-derived evidence.
pub fn maturity_ladder(input: &LadderInputs) -> MaturityLadder {
    let gs = input.gs;
    let cov = &gs.coverage;

    // ---- Rung 1: Seeded (RECORD complete) ----
    // The public surface is claimed and nothing enumerated remains undecomposed.
    // The un-mechanizable "did you seed EVERYTHING" is the grill-me / self-check
    // residue, surfaced elsewhere — never silently passed here.
    let mut seeded_reasons = Vec::new();
    if input.entrypoint.total > 0 && input.entrypoint.covered < input.entrypoint.total {
        seeded_reasons.push(format!(
            "{} public symbol(s) unowned — no claiming intent",
            input.entrypoint.total - input.entrypoint.covered
        ));
    }
    if input.inbox_untriaged > 0 {
        seeded_reasons.push(format!(
            "{} inbox item(s) un-triaged — enumerated but not decomposed",
            input.inbox_untriaged
        ));
    }
    let seeded = graded(
        "Seeded",
        seeded_reasons,
        String::new(),
        input.entrypoint.covered > 0,
    );

    // ---- Rung 2: Realized (DISCHARGE: unit) ----
    // Every leaf is built (vertical spine) and proven by a discriminating
    // programmatic test (G1). A spec marked built (doc-only) is not realized.
    let exec = &cov.proven_executed_leaves;
    let realized_leaves = &cov.realized_leaves;
    let mut realized_reasons = Vec::new();
    if !gs.vertically_complete {
        realized_reasons
            .push("vertical spine incomplete — an ungrounded leaf or unreached file".to_string());
    }
    if realized_leaves.covered > 0 && exec.covered < realized_leaves.covered {
        realized_reasons.push(format!(
            "{} of {} leaves not executed-proven (asserted-only or unproven)",
            realized_leaves.covered - exec.covered,
            realized_leaves.covered
        ));
    }
    if !input.doc_only_realizations.is_empty() {
        realized_reasons.push(format!(
            "{} intent(s) marked built but grounded only to docs (spec-as-built)",
            input.doc_only_realizations.len()
        ));
    }
    let realized = graded(
        "Realized",
        realized_reasons,
        format!("{}/{}", exec.covered, realized_leaves.covered),
        gs.vertically_complete,
    );

    // ---- Rung 3: Proven (DISCHARGE: boundary) ----
    // Every user-visible journey has a passing discriminating boundary proof.
    // Vacuous (no journeys) ⇒ N/A: the adaptive collapse for libraries/CLIs with
    // no user-visible flow — the public API is the boundary, proven at Realized.
    let proven = if input.journey.enumerated == 0 {
        Rung {
            name: "Proven",
            status: RungStatus::NotApplicable,
            detail: "—".to_string(),
            reasons: Vec::new(),
        }
    } else {
        let owed = input.journey.enumerated - input.journey.discharged;
        let mut reasons = Vec::new();
        if owed > 0 {
            reasons.push(format!(
                "{owed} of {} user-visible journey(s) have no passing boundary proof",
                input.journey.enumerated
            ));
        }
        graded(
            "Proven",
            reasons,
            format!("{}/{}", input.journey.discharged, input.journey.enumerated),
            input.journey.discharged > 0,
        )
    };

    // ---- Rung 4: Hardened (DISCHARGE: quality) ----
    // Measured under rules, the grid explored (duplication/coupling/layering
    // detectable), failure paths realized, and zero open smell findings.
    let measured = &cov.measured_pairs;
    let mut hardened_reasons = Vec::new();
    if measured.total > 0 && measured.covered < measured.total {
        hardened_reasons.push(format!(
            "{} rule×intent pair(s) unmeasured",
            measured.total - measured.covered
        ));
    }
    if !gs.horizontally_explored {
        hardened_reasons.push("RELATES_TO grid not fully explored".to_string());
    }
    if input.behavioral.enumerated > 0 && input.behavioral.discharged < input.behavioral.enumerated
    {
        hardened_reasons.push(format!(
            "{} happy leaf/leaves without a realized failure sibling",
            input.behavioral.enumerated - input.behavioral.discharged
        ));
    }
    if !input.open_smells.is_empty() {
        hardened_reasons.push(format!(
            "{} open smell finding(s) — see `loom smells`",
            input.open_smells.len()
        ));
    }
    let hardened = graded(
        "Hardened",
        hardened_reasons,
        String::new(),
        axis_cleared(measured) || gs.horizontally_explored,
    );

    // ---- Rung 5: Production-ready (DISCHARGE complete + durable + deploy-fit) ----
    // The ceiling: every lower rung cleared, the former `fully_proven` gate set
    // holds (incl. export freshness), and every comprehensiveness dimension is
    // DISCHARGED (boundary owned; journey/behavioral covered via lower rungs).
    let mut prod_reasons = Vec::new();
    if !input.fully_proven_ok {
        prod_reasons.extend(input.fully_proven_reasons.iter().cloned());
    }
    if !axis_cleared(input.boundary) {
        prod_reasons.push(format!(
            "{} boundary file(s) without an owning intent",
            input.boundary.total - input.boundary.covered
        ));
    }

    let mut rungs = vec![seeded, realized, proven, hardened];
    let lower_cleared = rungs.iter().all(|r| r.status.cleared());
    let prod_status = if lower_cleared && input.fully_proven_ok && axis_cleared(input.boundary) {
        RungStatus::Met
    } else {
        RungStatus::Unmet
    };
    rungs.push(Rung {
        name: "Production-ready",
        status: prod_status,
        detail: String::new(),
        reasons: prod_reasons,
    });

    let focus = rungs.iter().position(|r| !r.status.cleared());
    MaturityLadder { rungs, focus }
}

/// Everything `loom status` / `loom complete` need to render the ladder + its
/// comprehensiveness detail, assembled ONCE from a snapshot (+ disk for the
/// boundary scan). Pure given its inputs — no store handle — so both commands
/// share one assembly and cannot drift.
pub struct LadderBundle {
    pub ladder: MaturityLadder,
    pub entrypoint: CoverageAxis,
    pub boundary: CoverageAxis,
    pub boundary_owed: Vec<String>,
    pub journey: Ledger,
    pub behavioral: Ledger,
    pub doc_only: Vec<String>,
    pub modeled_pct: usize,
    pub grounded_symbols: usize,
    pub total_symbols: usize,
}

/// Assemble the ladder from a snapshot + the store-derived inputs the caller
/// already has in scope (decision notes, audit-gated open smells, the untriaged
/// inbox count, export staleness). The former `fully_proven` gate set is folded
/// in as the Production-ready rung's input — its math survives, its badge does not.
pub fn build_ladder(
    root: &Path,
    snapshot: &QuerySnapshot,
    gs: &GraphState,
    decision_notes: &[Note],
    open_smells: &[Smell],
    inbox_untriaged: usize,
    export_stale: bool,
) -> LadderBundle {
    let symbol_report = symbol_accountability_from_parts_with_notes(
        &snapshot.codefiles,
        &snapshot.intents,
        &snapshot.implements,
        decision_notes,
    );
    let entrypoint = comp::entrypoint_coverage(&symbol_report);
    let (boundary, boundary_owed) = comp::boundary_scan_from_disk(root, snapshot);
    let journey = comp::journey_ledger_from_snapshot(snapshot);
    let behavioral = comp::behavioral_ledger_from_snapshot(snapshot);
    let doc_only = comp::doc_only_realizations(snapshot);

    let (mut fp_ok, mut fp_reasons) =
        fully_proven_from_state(gs, snapshot, open_smells, &entrypoint, inbox_untriaged);
    if export_stale {
        fp_ok = false;
        fp_reasons.push("committed loom.graph.json is STALE — `loom export`".to_string());
    }

    let ladder = maturity_ladder(&LadderInputs {
        gs,
        entrypoint: &entrypoint,
        boundary: &boundary,
        journey: &journey,
        behavioral: &behavioral,
        open_smells,
        doc_only_realizations: &doc_only,
        inbox_untriaged,
        fully_proven_ok: fp_ok,
        fully_proven_reasons: &fp_reasons,
    });

    let total_symbols = symbol_report.summary.total_symbols;
    let grounded_symbols = symbol_report.summary.grounded;
    let modeled_pct = (grounded_symbols * 100)
        .checked_div(total_symbols)
        .unwrap_or(0);

    LadderBundle {
        ladder,
        entrypoint,
        boundary,
        boundary_owed,
        journey,
        behavioral,
        doc_only,
        modeled_pct,
        grounded_symbols,
        total_symbols,
    }
}

#[cfg(test)]
mod tests;
