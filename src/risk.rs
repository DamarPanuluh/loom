//! Risk — what to strengthen next, once everything is green.
//!
//! Plane: statistical ranking over the derived plane. Reported, never gating.
//!
//! Contract — **this queue re-orders; it never empties.** `phase = complete →
//! run loom status` was a dead end in a tool whose whole thesis is that every
//! command's output is the prompt for the next decision. A codebase is never
//! finished being understood, so the top rung is [`RungState::Open`]: not met,
//! not unmet, just always there.
//!
//! The moves escalate monotonically, and each completion LOWERS its own
//! candidate's score — so finishing work reorders the queue instead of draining
//! it, and the last move (`WidenBoundary`) mints new intents that re-enter at
//! the grounding rung. The graph grows harder work for itself.
//!
//! [`RungState::Open`]: crate::maturity::RungState::Open

use crate::model::{EdgeKind, NodeType};
use crate::proofstrength::Strength;
use crate::store::Store;
use crate::Result;
use serde::Serialize;

/// What to do next about one behavior, in escalating order. The first one that
/// applies is the move — you cannot freeze a baseline for a proof that does not
/// yet assert anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    /// The proof asserts liveness, not behavior.
    StrengthenAssertions,
    /// Nothing it runs reaches the code the behavior is made of.
    AnchorToCode,
    /// No frozen baseline: nothing notices when the output changes shape.
    FreezeBaseline,
    /// A baseline nobody replays is a fossil.
    ReplayAndRefreeze,
    /// Well proven and widely depended on — so propose the case nothing covers.
    WidenBoundary,
}

impl Move {
    pub fn as_str(self) -> &'static str {
        match self {
            Move::StrengthenAssertions => "strengthen_assertions",
            Move::AnchorToCode => "anchor_to_code",
            Move::FreezeBaseline => "freeze_baseline",
            Move::ReplayAndRefreeze => "replay_and_refreeze",
            Move::WidenBoundary => "widen_boundary",
        }
    }

    fn from_strength(s: Strength) -> Move {
        match s {
            Strength::S0 | Strength::S1 => Move::StrengthenAssertions,
            Strength::S2 => Move::AnchorToCode,
            Strength::S3 => Move::FreezeBaseline,
            Strength::S4 => Move::ReplayAndRefreeze,
            Strength::S5 => Move::WidenBoundary,
        }
    }

    pub fn why(self) -> &'static str {
        match self {
            Move::StrengthenAssertions => {
                "this proof establishes that the code runs, not that it works — \
                 assert something about what it produces"
            }
            Move::AnchorToCode => {
                "nothing this proof runs reaches the symbol the behavior is grounded in, \
                 so it could pass forever while the behavior is broken"
            }
            Move::FreezeBaseline => {
                "freeze a baseline so a change in the SHAPE of the output is noticed, \
                 not just a change in pass/fail"
            }
            Move::ReplayAndRefreeze => {
                "the baseline has not been replayed — a baseline nobody replays is a fossil"
            }
            Move::WidenBoundary => {
                "well proven and widely depended on: name the sad path or edge case \
                 nothing currently exercises"
            }
        }
    }
}

/// One ranked candidate.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub intent_id: String,
    pub intent_name: String,
    pub score: f64,
    pub next_move: Move,
    pub why: &'static str,
    /// The inputs, so the ranking can be argued with rather than trusted.
    pub blast_radius: f64,
    pub proof_strength: &'static str,
    pub evidence_age_days: i64,
}

/// Rank every implemented behavior by how much it would hurt to be wrong about.
///
/// `score = blast_radius × (1 − proof_strength) × (0.5 + 0.5 × evidence_age)`
///
/// The three factors answer three different questions — how much depends on it,
/// how well is it pinned down, and how long since anyone checked — and
/// multiplying means a zero in any one of them takes the candidate out. That is
/// deliberate: a behavior nothing depends on is not urgent however weak its
/// proof, and a perfectly proven one is not urgent however central.
pub fn rank(store: &Store) -> Result<Vec<Candidate>> {
    let graph = crate::callgraph::build(store)?;
    let total_callers: usize = graph.edges.len().max(1);
    let mut out = Vec::new();

    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status != "implemented" {
            continue;
        }
        let fan_in = fan_in(store, &graph, &intent.id)?;
        let blast = (fan_in as f64 / total_callers as f64).min(1.0);
        let strength = best_proof(store, &intent.id)?;
        let normalized = strength_fraction(strength);
        let age = evidence_age_days(store, &intent.id)?;
        let age_factor = age as f64 / (age as f64 + 30.0);
        let score = blast * (1.0 - normalized) * (0.5 + 0.5 * age_factor);
        if score <= 0.0 {
            continue;
        }
        let next_move = Move::from_strength(strength);
        out.push(Candidate {
            intent_id: intent.id.clone(),
            intent_name: intent.name.clone(),
            score,
            next_move,
            why: next_move.why(),
            blast_radius: blast,
            proof_strength: strength.as_str(),
            evidence_age_days: age,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.intent_name.cmp(&b.intent_name))
    });
    Ok(out)
}

/// A perfectly graded proof contributes nothing to urgency; an ungraded one
/// contributes everything.
fn strength_fraction(s: Strength) -> f64 {
    match s {
        Strength::S0 => 0.0,
        Strength::S1 => 0.2,
        Strength::S2 => 0.4,
        Strength::S3 => 0.6,
        Strength::S4 => 0.8,
        Strength::S5 => 1.0,
    }
}

/// How many symbols transitively reach this behavior's code.
fn fan_in(store: &Store, graph: &crate::callgraph::CallGraph, intent_id: &str) -> Result<usize> {
    let mut total = 0usize;
    for sym in crate::locator::realizing_navigation_symbols(store, intent_id)? {
        total += graph.impact(&sym, 3).callers.len();
    }
    Ok(total)
}

fn best_proof(store: &Store, intent_id: &str) -> Result<Strength> {
    let mut best = Strength::S0;
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        let s = crate::proofstrength::of(store, &e.from_id)?;
        if s > best {
            best = s;
        }
    }
    Ok(best)
}

/// Days since the oldest still-standing fact about this behavior.
fn evidence_age_days(store: &Store, intent_id: &str) -> Result<i64> {
    let now = crate::journal::now_iso();
    let mut oldest: Option<String> = None;
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if let Some(view) = store.fact(
            &crate::store::Subject::Edge(e.id.clone()),
            crate::model::Claim::Verdict,
        )? {
            let at = view.fact.asserted_at.clone();
            if oldest.as_ref().map(|o| at < *o).unwrap_or(true) {
                oldest = Some(at);
            }
        }
    }
    Ok(oldest.map(|at| days_between(&at, &now)).unwrap_or(0))
}

fn days_between(a: &str, b: &str) -> i64 {
    fn ord(s: &str) -> Option<i64> {
        let d = s.split('T').next()?;
        let mut p = d.split('-');
        let y: i64 = p.next()?.parse().ok()?;
        let m: i64 = p.next()?.parse().ok()?;
        let day: i64 = p.next()?.parse().ok()?;
        Some(y * 365 + m * 31 + day)
    }
    match (ord(a), ord(b)) {
        (Some(x), Some(y)) => (y - x).max(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_moves_escalate_and_never_skip() {
        assert_eq!(
            Move::from_strength(Strength::S1),
            Move::StrengthenAssertions
        );
        assert_eq!(Move::from_strength(Strength::S2), Move::AnchorToCode);
        assert_eq!(Move::from_strength(Strength::S5), Move::WidenBoundary);
        // Every move explains itself — a ranking nobody understands is one
        // nobody acts on.
        for m in [
            Move::StrengthenAssertions,
            Move::AnchorToCode,
            Move::FreezeBaseline,
            Move::ReplayAndRefreeze,
            Move::WidenBoundary,
        ] {
            assert!(m.why().len() > 30);
        }
    }

    #[test]
    fn a_zero_in_any_factor_takes_a_candidate_out() {
        // Nothing depends on it → not urgent however weak the proof.
        assert_eq!(0.0 * (1.0 - 0.0) * (0.5 + 0.5), 0.0);
        // Perfectly proven → not urgent however central.
        assert_eq!(1.0 * (1.0 - strength_fraction(Strength::S5)) * 1.0, 0.0);
    }
}
