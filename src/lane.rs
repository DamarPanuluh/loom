//! Lane — the single routing table.
//!
//! Plane: pure data + pure projections over [`LadderInputs`]. No store access
//! lives here (the one gather is `LadderInputs::gather`, in `maturity`).
//!
//! Contract: **every rung has a lane, every lane has a rung, and a rung is unmet
//! iff its lane's queue is non-empty.** Before this module the ladder
//! (`maturity::build_rungs`) and the compass (`maturity::compass`) were two
//! independent if-chains over overlapping inputs, kept in agreement only by
//! comments — so the compass could point at a lane that `loom next --mode <m>`
//! would not serve, and three lanes (`prove`, `review`, `elaborate`) had no rung
//! at all. `LADDER` order IS rung order IS default-`next` priority. There is no
//! second ordering anywhere in the codebase.

use crate::truth::TruthAxis;
use serde::Serialize;

/// A unit of work with one queue, one rung, and one truth axis.
///
/// Ordering derives `Ord` for use as a `BTreeMap` key; the meaningful order is
/// [`Lane::LADDER`], not the derived one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Seed,
    Fix,
    Build,
    Coverage,
    Validate,
    Quality,
    Analyze,
    Review,
    Triage,
    Prove,
    Elaborate,
    /// Where the graph's evidence and the human's recorded judgment disagree.
    /// Human-presence work; never served by plain `loom next`.
    Divergence,
    Audit,
    Export,
    /// Post-floor risk work: never completes by design. Last in
    /// [`Lane::LADDER`], and permanently [`crate::maturity::RungState::Open`].
    Deepen,
}

impl Lane {
    /// Ladder order. The index in this slice IS the rung index.
    pub const LADDER: &'static [Lane] = &[
        Lane::Seed,
        Lane::Fix,
        Lane::Build,
        Lane::Coverage,
        Lane::Validate,
        Lane::Quality,
        Lane::Analyze,
        Lane::Review,
        Lane::Triage,
        Lane::Prove,
        Lane::Elaborate,
        Lane::Divergence,
        Lane::Audit,
        Lane::Export,
        // Always last, and never met: a codebase is never finished being
        // understood, so the top of the ladder is a standing invitation rather
        // than a finish line.
        Lane::Deepen,
    ];

    /// The maturity rung this lane closes.
    pub fn rung(self) -> &'static str {
        match self {
            Lane::Seed => "seeded",
            Lane::Fix => "repaired",
            Lane::Build => "grounded",
            Lane::Coverage => "covered",
            Lane::Validate => "proven",
            Lane::Quality => "measured",
            Lane::Analyze => "inspected",
            Lane::Review => "reviewed",
            Lane::Triage => "triaged",
            Lane::Prove => "investigated",
            Lane::Elaborate => "elaborated",
            Lane::Divergence => "converged",
            Lane::Audit => "sound",
            Lane::Export => "published",
            Lane::Deepen => "deepening",
        }
    }

    /// The lane's stable wire name: the compass phase and the `--mode` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Seed => "seed",
            Lane::Fix => "fix",
            Lane::Build => "build",
            Lane::Coverage => "coverage",
            Lane::Validate => "validate",
            Lane::Quality => "quality",
            Lane::Analyze => "analyze",
            Lane::Review => "review",
            Lane::Triage => "triage",
            Lane::Prove => "prove",
            Lane::Elaborate => "elaborate",
            // `ratify` is the operator-facing name for the divergence queue.
            Lane::Divergence => "ratify",
            Lane::Audit => "audit",
            Lane::Export => "export",
            Lane::Deepen => "deepen",
        }
    }

    pub fn parse(s: &str) -> Option<Lane> {
        match s {
            "seed" => Some(Lane::Seed),
            "fix" => Some(Lane::Fix),
            "build" => Some(Lane::Build),
            "coverage" => Some(Lane::Coverage),
            "validate" => Some(Lane::Validate),
            "quality" => Some(Lane::Quality),
            "analyze" | "discovery" => Some(Lane::Analyze),
            "review" => Some(Lane::Review),
            "triage" => Some(Lane::Triage),
            "prove" => Some(Lane::Prove),
            "elaborate" => Some(Lane::Elaborate),
            "ratify" | "divergence" => Some(Lane::Divergence),
            "audit" => Some(Lane::Audit),
            "export" => Some(Lane::Export),
            "deepen" => Some(Lane::Deepen),
            _ => None,
        }
    }

    /// The single suggested next command when this lane is the compass gate.
    pub fn next_command(self) -> String {
        match self {
            Lane::Seed => "loom door \"<what should this codebase do>\" or loom intent add".into(),
            Lane::Coverage => "loom coverage".into(),
            Lane::Audit => "loom audit".into(),
            Lane::Export => "loom export && loom export --check".into(),
            other => format!("loom next --mode {}", other.as_str()),
        }
    }

    /// Which form of truth this lane makes true. One axis per lane — the old
    /// `axis_for_phase` string match had to disambiguate an overloaded `audit`
    /// arm by grepping the command text.
    pub fn axis(self) -> TruthAxis {
        match self {
            Lane::Seed | Lane::Elaborate | Lane::Divergence => TruthAxis::Intent,
            Lane::Fix | Lane::Build | Lane::Coverage => TruthAxis::Implementation,
            Lane::Validate => TruthAxis::Proof,
            Lane::Quality | Lane::Analyze | Lane::Review | Lane::Prove => TruthAxis::Verdict,
            Lane::Triage | Lane::Audit => TruthAxis::Signal,
            Lane::Export => TruthAxis::Projection,
            Lane::Deepen => TruthAxis::Risk,
        }
    }

    /// Disabled on an observed graph: a monitor cannot change the upstream it
    /// watches (docs/commands.md:90).
    pub fn observed_disabled(self) -> bool {
        matches!(
            self,
            Lane::Build | Lane::Fix | Lane::Coverage | Lane::Elaborate
        )
    }

    /// Served by plain `loom next` only when a human is at the terminal — an
    /// LLM driver must never be routed into a write it is denied (INV-8).
    pub fn human_only(self) -> bool {
        matches!(self, Lane::Divergence)
    }

    /// Whether `loom next --mode <lane>` compiles a work packet for this lane.
    /// Lanes that route to a whole-graph command instead (`loom door`,
    /// `loom doctor`, `loom export`) serve no per-item packet.
    pub fn serves_items(self) -> bool {
        !matches!(self, Lane::Seed | Lane::Export)
    }

    /// This lane's queue depth. `Unmet ⟺ depth > 0`, so this is also the rung
    /// predicate: there is no second definition anywhere.
    pub fn depth(self, c: &LadderInputs) -> usize {
        if c.observed && self.observed_disabled() {
            return 0;
        }
        match self {
            Lane::Seed => usize::from(c.active == 0),
            Lane::Fix => c.failing,
            Lane::Build => c.planned + c.ungrounded,
            Lane::Coverage => c.unowned_codefiles,
            // The rung and the queue agree only because `validate_item` also
            // serves an implemented intent that has no registered proof at all;
            // before the unification the compass pointed here and the lane
            // returned nothing.
            Lane::Validate => {
                c.stale_validates
                    + c.uninspected_validates
                    + c.unproven_implemented
                    + c.open_journey_proof_smells
            }
            Lane::Quality => c.stale_governs + c.uninspected_governs + c.unmeasured_quality_pairs,
            Lane::Analyze => {
                c.failing_exemplars
                    + c.open_research
                    + c.stale_relationships
                    + c.uninspected_relationships
            }
            Lane::Review => c.low_confidence,
            Lane::Triage => c.triage_findings + c.inbox_new,
            Lane::Prove => c.proposed_hypotheses,
            Lane::Elaborate => c.open_elaborations,
            Lane::Divergence => c.divergences,
            Lane::Audit => c.doctor_issues + c.open_smells + c.audit_findings,
            Lane::Export => usize::from(!c.export_fresh),
            Lane::Deepen => 0,
        }
    }

    /// The rung's one-line detail. Reports the same numbers `depth` sums, so a
    /// reader can always see why a rung is unmet.
    pub fn detail(self, c: &LadderInputs) -> String {
        match self {
            Lane::Seed => format!("{} active intent(s)", c.active),
            Lane::Fix => format!("{} failing claim(s)", c.failing),
            Lane::Build => format!(
                "{} unrealized, {} ungrounded implemented intent(s)",
                c.planned, c.ungrounded
            ),
            Lane::Coverage => format!("{} unowned codefile(s)", c.unowned_codefiles),
            Lane::Validate => format!(
                "{} registered: {} passed, {} failed, {} blocked, {} not_run, \
                 {} unproven implemented intent(s), {} unrun/stale proof edge(s){}",
                c.validations.registered,
                c.validations.passed,
                c.validations.failed,
                c.validations.blocked,
                c.validations.not_run,
                c.unproven_implemented,
                c.stale_validates + c.uninspected_validates,
                if c.open_journey_proof_smells > 0 {
                    format!(", {} journey proof gap(s)", c.open_journey_proof_smells)
                } else {
                    String::new()
                }
            ),
            Lane::Quality => format!(
                "{} stale, {} uninspected, {} never-measured rule pair(s)",
                c.stale_governs, c.uninspected_governs, c.unmeasured_quality_pairs
            ),
            Lane::Analyze => format!(
                "{} stale, {} uninspected relationship claim(s)",
                c.stale_relationships, c.uninspected_relationships
            ),
            Lane::Review => format!("{} verdict(s) below the review floor", c.low_confidence),
            Lane::Triage => format!(
                "{} unjudged/stale finding(s), {} new inbox item(s)",
                c.triage_findings, c.inbox_new
            ),
            Lane::Prove => format!("{} proposed hypothesis(es)", c.proposed_hypotheses),
            Lane::Elaborate => format!(
                "{} user-visible idea(s) with open completeness axes",
                c.open_elaborations
            ),
            Lane::Divergence => format!(
                "{} divergence(s) where judgment and evidence disagree",
                c.divergences
            ),
            Lane::Audit => format!(
                "{} doctor issue(s), {} open smell(s), {} self-audit finding(s)",
                c.doctor_issues, c.open_smells, c.audit_findings
            ),
            Lane::Export => if c.export_fresh {
                "loom.graph.json fresh"
            } else {
                "loom.graph.json missing or stale"
            }
            .into(),
            // Never "0 left": this lane re-ranks rather than drains, so the
            // detail says what is at the top, not how much is outstanding.
            Lane::Deepen => format!("{} behavior(s) worth strengthening", c.risk_candidates),
        }
    }

    /// `NotApplicable` when this lane's machinery does not exist yet, so the
    /// ladder never counts absent machinery as failure. `NotApplicable` is
    /// transparent to the gate — it can never block a rung above it.
    pub fn not_applicable(self, c: &LadderInputs) -> bool {
        match self {
            // Seeding is always applicable: an empty graph is the one thing
            // this rung exists to report.
            Lane::Seed => false,
            // Proof is not owed until something is implemented.
            Lane::Validate => c.implemented == 0,
            Lane::Deepen => c.codefiles == 0,
            _ => c.active == 0,
        }
    }
}

/// Every scalar the ladder, the compass, and the queue depths are computed
/// from — gathered exactly once per `loom status` / `loom next` (see
/// `LadderInputs::gather` in `maturity`). One gather means a lane can never be
/// counted twice by two predicates that drift apart.
#[derive(Debug, Clone, Default)]
pub struct LadderInputs {
    pub observed: bool,
    pub active: usize,
    pub implemented: usize,
    pub codefiles: usize,
    pub planned: usize,
    pub ungrounded: usize,
    pub unowned_codefiles: usize,
    pub failing: usize,
    pub failing_exemplars: usize,
    pub open_research: usize,
    pub stale_relationships: usize,
    pub uninspected_relationships: usize,
    pub stale_governs: usize,
    pub uninspected_governs: usize,
    pub stale_validates: usize,
    pub uninspected_validates: usize,
    pub unmeasured_quality_pairs: usize,
    pub validations: crate::maturity::ValidationSummary,
    pub unproven_implemented: usize,
    pub open_journey_proof_smells: usize,
    pub low_confidence: usize,
    pub triage_findings: usize,
    pub inbox_new: usize,
    pub proposed_hypotheses: usize,
    pub open_elaborations: usize,
    pub divergences: usize,
    pub doctor_issues: usize,
    pub open_smells: usize,
    /// Self-fabrication signatures found in this graph's own record.
    pub audit_findings: usize,
    /// Behaviors the risk ranking has something to say about. Never gates —
    /// `deepen` is `Open`, not unmet.
    pub risk_candidates: usize,
    pub export_fresh: bool,
}

/// Per-lane backlog depths, keyed by the lane's wire name so the JSON surface
/// grows with the lane table instead of a hand-maintained struct.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(transparent)]
pub struct QueueDepths(std::collections::BTreeMap<&'static str, usize>);

impl QueueDepths {
    pub fn from_inputs(c: &LadderInputs) -> Self {
        let mut map = std::collections::BTreeMap::new();
        for lane in Lane::LADDER {
            map.insert(lane.as_str(), lane.depth(c));
        }
        QueueDepths(map)
    }

    pub fn get(&self, lane: Lane) -> usize {
        self.0.get(lane.as_str()).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.0.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_a_bijection_with_rungs_and_modes() {
        let mut rungs = std::collections::BTreeSet::new();
        let mut modes = std::collections::BTreeSet::new();
        for lane in Lane::LADDER {
            assert!(rungs.insert(lane.rung()), "duplicate rung {}", lane.rung());
            assert!(
                modes.insert(lane.as_str()),
                "duplicate lane name {}",
                lane.as_str()
            );
            assert_eq!(
                Lane::parse(lane.as_str()),
                Some(*lane),
                "{} does not round-trip",
                lane.as_str()
            );
            assert!(!lane.next_command().is_empty());
        }
    }

    #[test]
    fn depth_is_zero_for_disabled_lanes_on_an_observed_graph() {
        let c = LadderInputs {
            observed: true,
            active: 3,
            planned: 5,
            ungrounded: 2,
            unowned_codefiles: 4,
            failing: 7,
            open_elaborations: 1,
            ..Default::default()
        };
        assert_eq!(Lane::Build.depth(&c), 0);
        assert_eq!(Lane::Fix.depth(&c), 0);
        assert_eq!(Lane::Coverage.depth(&c), 0);
        assert_eq!(Lane::Elaborate.depth(&c), 0);
    }

    #[test]
    fn every_lane_that_serves_items_is_reachable_by_mode() {
        for lane in Lane::LADDER {
            if lane.serves_items() {
                assert_eq!(Lane::parse(lane.as_str()), Some(*lane));
            }
        }
        // The divergence queue is addressed as `ratify` by operators.
        assert_eq!(Lane::parse("ratify"), Some(Lane::Divergence));
        assert_eq!(Lane::Divergence.as_str(), "ratify");
    }
}
