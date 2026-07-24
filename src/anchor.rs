//! Anchor floors — how strongly each kind of fact must be anchored before loom
//! will let it settle.
//!
//! Plane: pure policy table. No store access, no I/O.
//!
//! Contract: `assert_fact` refuses a write whose evidence is weaker than the
//! floor for its (claim, edge kind, state), and the refusal names the command
//! that would produce the missing anchor. A fact below its floor does not merely
//! look weaker — it does not settle at all, so its lane keeps serving it.
//!
//! The floors start at [`Verification::Claimed`] (behavior-preserving) and are
//! raised one category at a time, each with its own regression, so a floor is
//! never turned on faster than the machinery that can satisfy it.

use crate::model::{Claim, EdgeKind, GroundingRole, Verification};

/// What a fact must clear to settle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Floor {
    pub required: Verification,
    /// What to run or cite to reach it. Shown verbatim in the refusal, so a
    /// worker never has to guess what loom wanted.
    pub remedy: &'static str,
}

impl Floor {
    const fn new(required: Verification, remedy: &'static str) -> Floor {
        Floor { required, remedy }
    }
}

/// Settling states — the ones that assert something is TRUE. `uninspected` and
/// `needs_reverification` assert nothing, so they carry no floor.
pub fn is_settling(state: &str) -> bool {
    matches!(
        state,
        "passing"
            | "failing"
            | "independent"
            | "passed"
            | "failed"
            | "justified"
            | "rejected"
            | "duplicate"
            | "deferred"
            | "resolved"
            | "ratified"
    )
}

/// The floor for one assertion.
///
/// `blocked` is always [`Verification::Claimed`]: an honestly blocked proof is a
/// real record with a real reason, and forcing an anchor onto it would just
/// teach workers to fabricate one. It stays visible and never counts as green.
pub fn required(
    claim: Claim,
    edge_kind: Option<EdgeKind>,
    role: Option<GroundingRole>,
    state: &str,
) -> Floor {
    // An open problem needs no proof; a blocked one needs a reason, not an anchor.
    if matches!(
        state,
        "blocked" | "needed" | "uninspected" | "needs_reverification"
    ) {
        return Floor::new(Verification::Claimed, "record the concrete blocker");
    }
    match claim {
        Claim::Verdict => verdict_floor(edge_kind, role, state),
        Claim::Adjudication => adjudication_floor(state),
        Claim::Ratification => Floor::new(
            CURRENT_RATIFICATION,
            "ratify from a recorded human utterance (loom drive) or cite the decision \
             record that asked for it",
        ),
        Claim::Observation => Floor::new(Verification::Claimed, "cite the file:line you observed"),
    }
}

fn verdict_floor(edge_kind: Option<EdgeKind>, role: Option<GroundingRole>, _state: &str) -> Floor {
    match edge_kind {
        // A realizing grounding is checkable: the locator must resolve to a live
        // symbol. loom re-resolves it itself, so this floor is reachable without
        // the worker doing anything but naming the symbol correctly.
        Some(EdgeKind::Implements) if role != Some(GroundingRole::Realizes) => Floor::new(
            CURRENT_SEAM_GROUNDING,
            "point --locator at the seam this file uses",
        ),
        Some(EdgeKind::Implements) => Floor::new(
            CURRENT_GROUNDING,
            "point --locator at the symbol that performs the behavior",
        ),
        Some(EdgeKind::Validates) => Floor::new(
            CURRENT_PROOF,
            "run the proof through loom (`loom validation run`), never report its outcome",
        ),
        Some(EdgeKind::Governs) => Floor::new(
            CURRENT_QUALITY,
            "give the rule `patterns` so loom can scan for itself, or cite a span per \
             realizing file",
        ),
        // Taxonomy, not a claim about code.
        Some(EdgeKind::Hierarchy) => Floor::new(Verification::Claimed, "state the decomposition"),
        _ => Floor::new(
            CURRENT_RELATIONSHIP,
            "cite a span in a file realizing each endpoint",
        ),
    }
}

fn adjudication_floor(state: &str) -> Floor {
    match state {
        // "I fixed it" is re-checkable: loom re-runs the finding's own predicate.
        "resolved" => Floor::new(
            CURRENT_RESOLVED_FINDING,
            "fix it, then let loom re-run the detector",
        ),
        _ => Floor::new(
            CURRENT_FINDING,
            "cite the span in the flagged file, or a journal entry",
        ),
    }
}

// ---------------------------------------------------------------------------
// The dial. Each constant is raised in its own commit, with its own regression,
// once the machinery that can satisfy it exists. Until then loom records these
// facts honestly at whatever strength their evidence earns — it just does not
// yet REFUSE the weak ones.
// ---------------------------------------------------------------------------

const CURRENT_GROUNDING: Verification = Verification::Claimed;
const CURRENT_SEAM_GROUNDING: Verification = Verification::Claimed;
// STAGED: the machinery behind this is complete — `loom validation run` now
// produces a RunRecord and `mark_validation` anchors the verdict to it, so
// flipping this to `Verified` refuses caller-reported proof outcomes with
// "run the proof through loom, never report its outcome". Flipped together
// with the ~20 test fixtures that still hand-record a passing `validates`
// verdict; `common::prove` is the honest replacement for them.
const CURRENT_PROOF: Verification = Verification::Claimed;
const CURRENT_QUALITY: Verification = Verification::Claimed;
const CURRENT_RELATIONSHIP: Verification = Verification::Claimed;
const CURRENT_FINDING: Verification = Verification::Claimed;
const CURRENT_RESOLVED_FINDING: Verification = Verification::Claimed;
const CURRENT_RATIFICATION: Verification = Verification::Claimed;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_blocked_states_never_demand_an_anchor() {
        for state in ["blocked", "needed", "uninspected", "needs_reverification"] {
            assert_eq!(
                required(Claim::Adjudication, None, None, state).required,
                Verification::Claimed,
                "{state} asserts nothing is true"
            );
        }
    }

    #[test]
    fn every_floor_names_a_remedy() {
        for claim in Claim::ALL {
            for state in ["passing", "failing", "justified", "resolved", "ratified"] {
                let floor = required(*claim, Some(EdgeKind::Implements), None, state);
                assert!(
                    floor.remedy.len() > 10,
                    "{claim:?}/{state} must say how to reach its floor"
                );
            }
        }
    }

    #[test]
    fn hierarchy_is_taxonomy_not_a_claim_about_code() {
        assert_eq!(
            required(Claim::Verdict, Some(EdgeKind::Hierarchy), None, "passing").required,
            Verification::Claimed
        );
    }
}
