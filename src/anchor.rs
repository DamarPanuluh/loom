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
/// Extra shape the floor needs beyond the edge kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shape {
    /// This proof has a command loom can execute. A `manual_check` does not:
    /// demanding a Run for one would make it unrecordable, which is worse than
    /// recording it honestly as attested-but-not-observed.
    pub runnable_proof: bool,
    /// This quality rule carries patterns loom can scan for itself.
    pub scannable_rule: bool,
}

pub fn required(
    claim: Claim,
    edge_kind: Option<EdgeKind>,
    role: Option<GroundingRole>,
    state: &str,
) -> Floor {
    required_for(claim, edge_kind, role, state, Shape::default())
}

pub fn required_for(
    claim: Claim,
    edge_kind: Option<EdgeKind>,
    role: Option<GroundingRole>,
    state: &str,
    shape: Shape,
) -> Floor {
    // An open problem needs no proof; a blocked one needs a reason, not an anchor.
    if matches!(
        state,
        "blocked" | "needed" | "uninspected" | "needs_reverification"
    ) {
        return Floor::new(Verification::Claimed, "record the concrete blocker");
    }
    match claim {
        Claim::Verdict => verdict_floor(edge_kind, role, state, shape),
        Claim::Adjudication => adjudication_floor(state),
        Claim::Ratification => Floor::new(
            CURRENT_RATIFICATION,
            "ratify from a recorded human utterance (loom drive) or cite the decision \
             record that asked for it",
        ),
        Claim::Observation => Floor::new(Verification::Claimed, "cite the file:line you observed"),
    }
}

fn verdict_floor(
    edge_kind: Option<EdgeKind>,
    role: Option<GroundingRole>,
    _state: &str,
    shape: Shape,
) -> Floor {
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
        // A runnable proof must be RUN. A manual check cannot be, so it settles
        // at `cited` — attested, visibly weaker than observed, and honest about
        // which it is. Making both look the same is how a graph ends up
        // reporting 59 proofs when it has watched 5.
        Some(EdgeKind::Validates) if shape.runnable_proof => Floor::new(
            CURRENT_PROOF,
            "run the proof through loom (`loom validation run`), never report its outcome",
        ),
        Some(EdgeKind::Validates) => Floor::new(
            CURRENT_MANUAL_PROOF,
            "attest the manual check with what you observed, citing file:line or a journal entry",
        ),
        Some(EdgeKind::Governs) if shape.scannable_rule => Floor::new(
            CURRENT_QUALITY,
            "loom scans the rule's patterns itself — record the verdict and it anchors",
        ),
        Some(EdgeKind::Governs) => Floor::new(
            CURRENT_UNPATTERNED_QUALITY,
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

/// A grounding must POINT AT THE CODE — a sentence saying the behavior lives
/// here is not a grounding. Two ways to clear this floor, and loom grades them
/// differently on purpose:
///
/// - a `--locator` that resolves to a live symbol, which loom re-resolves
///   itself and records as a Run (`verified` — loom looked);
/// - a `file:line` citation, fingerprinted and re-checked on change (`cited`).
///
/// File-scoped groundings are legitimate, so the floor belongs at `cited`;
/// naming the symbol earns the stronger grade for free, and `deepen` can later
/// route on the difference.
///
/// STAGED at `claimed`. The probe above is live — a grounding whose locator
/// resolves already records `verified` — but raising the floor surfaces a long
/// tail of fixtures citing `file:line` for files that were never created, so
/// the citation was never evidence, it only looked like one. Raised with those
/// repairs together rather than half.
const CURRENT_GROUNDING: Verification = Verification::Claimed;
/// A consumer/config/verify seam is a weaker claim by nature — it says the file
/// USES the behavior, not that it performs it — but it still has to point
/// somewhere. Staged with the grounding floor above.
const CURRENT_SEAM_GROUNDING: Verification = Verification::Claimed;
/// A proof is `verified` or it is not a proof. There is exactly one way to
/// reach this floor: let loom run the command and observe the result. Reporting
/// an outcome is refused — that move is what made 54 of 59 proofs in loom's own
/// graph green without loom ever executing them.
const CURRENT_PROOF: Verification = Verification::Verified;
const CURRENT_MANUAL_PROOF: Verification = Verification::Cited;
/// A rule that carries `patterns` is checkable by loom itself, including when
/// what it asserts is an ABSENCE — loom scans and records finding nothing.
/// A rule with no patterns cannot be scanned, so it settles at `cited`: point
/// at what you inspected.
const CURRENT_QUALITY: Verification = Verification::Verified;
const CURRENT_UNPATTERNED_QUALITY: Verification = Verification::Claimed;
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
