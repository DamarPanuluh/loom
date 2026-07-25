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
    /// This finding flags a file that exists, so the judge had somewhere to
    /// look. A smell about an INTENT (`vague_intent`) flags no code — demanding
    /// a span for it would only teach the judge to invent one.
    pub flagged_file: bool,
    /// Both endpoints of this relationship have a realizing grounding, so there
    /// is source on both sides where the relationship can be seen. Two intents
    /// with no code yet are a design claim, not a claim about code.
    pub endpoints_realized: bool,
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
    // An open problem needs no proof; a blocked one needs a reason, not an
    // anchor. `needs_reconfirmation` and `unratified` belong here too: both are
    // a LOSS of standing, which sync performs whenever meaning drifts. Demanding
    // evidence to withdraw a claim would mean stale claims could never be
    // withdrawn — the opposite of what the floor is for.
    if matches!(
        state,
        "blocked"
            | "needed"
            | "uninspected"
            | "needs_reverification"
            | "needs_reconfirmation"
            | "unratified"
    ) {
        return Floor::new(Verification::Claimed, "record the concrete blocker");
    }
    match claim {
        Claim::Verdict => verdict_floor(edge_kind, role, state, shape),
        Claim::Adjudication => adjudication_floor(state, shape),
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
    state: &str,
    shape: Shape,
) -> Floor {
    // `independent` on a relationship says the two behaviors DO NOT interact.
    // No span witnesses an absence — pointing at code that does not mention the
    // other endpoint proves nothing, and demanding one would only produce
    // decorative citations. It stays `claimed`: recorded, visible, never green.
    //
    // This is the one absence loom cannot yet check for itself. The prescreen
    // probe answers the same question for quality rules by scanning; the call
    // graph is what would answer it here (neither endpoint's realizing files
    // reach the other's), and raising this floor is what that probe is for.
    if state == "independent" && !matches!(edge_kind, Some(EdgeKind::Implements)) {
        return Floor::new(
            Verification::Claimed,
            "say what you compared and why they do not interact",
        );
    }
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
        // A relationship is only a claim about code once both ends HAVE code.
        // Before that it is a design statement — recorded, never green, and it
        // rises to the floor on its own the moment both endpoints ground.
        _ if !shape.endpoints_realized => Floor::new(
            Verification::Claimed,
            "ground both behaviors in code, then cite where the relationship shows",
        ),
        _ => Floor::new(
            CURRENT_RELATIONSHIP,
            "cite a span in a file realizing each endpoint",
        ),
    }
}

fn adjudication_floor(state: &str, shape: Shape) -> Floor {
    if !shape.flagged_file {
        // Nothing on disk to point at. Recorded honestly at whatever the
        // rationale earns; `doctor` names these as unanchorable.
        return Floor::new(
            Verification::Claimed,
            "this finding flags no file — say what you decided and why",
        );
    }
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
const CURRENT_GROUNDING: Verification = Verification::Cited;
/// A consumer/config/verify seam is a weaker claim by nature — it says the file
/// USES the behavior, not that it performs it — but it still has to point
/// somewhere.
const CURRENT_SEAM_GROUNDING: Verification = Verification::Cited;
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
/// A relationship between two behaviors is a claim about code even though it
/// names no file: "A requires B" is either visible somewhere in the source or it
/// is a guess. Cited, not verified — loom cannot re-derive intent from a span,
/// only re-check that the span still says what it said.
///
/// This floor is also what makes the ripple matrix deletable. A `claimed`
/// relationship is invisible to re-verification, so the only way to notice it
/// had gone stale was the hand-written dependency walk in `sync.rs`. Anchored,
/// it expires like everything else.
const CURRENT_RELATIONSHIP: Verification = Verification::Cited;
/// A judgment about a finding points at the flagged code or at the conversation
/// where it was decided. Either is re-checkable; a bare opinion is not.
const CURRENT_FINDING: Verification = Verification::Cited;
const CURRENT_RESOLVED_FINDING: Verification = Verification::Claimed;
/// A ratification points at the moment it happened. Two anchors, and the
/// distinction between them is the whole design: the PROSE anchors the want
/// (why this behavior is wanted), and the JOURNAL entry anchors the act (a
/// person, present, saying so). loom writes the journal entry itself before
/// stamping the fact, so the ref is real by construction — which is exactly why
/// prose is checked separately for substance, or every ratification would
/// self-anchor on the entry loom just wrote.
///
/// This is the floor that identifies the 39 fabricated records: a `ratified`
/// state with nothing but a caller's sentence behind it.
const CURRENT_RATIFICATION: Verification = Verification::Cited;

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

    /// The three carve-outs are not softness — each names a case where the
    /// anchor loom would demand does not exist, and demanding it would produce
    /// a decorative citation instead of a real one.
    #[test]
    fn a_floor_is_only_demanded_where_the_anchor_could_exist() {
        let realized = Shape {
            endpoints_realized: true,
            ..Default::default()
        };
        // An absence has no witness.
        assert_eq!(
            required_for(
                Claim::Verdict,
                Some(EdgeKind::Relates),
                None,
                "independent",
                realized
            )
            .required,
            Verification::Claimed
        );
        // Neither endpoint has code yet — a design claim, not a code claim.
        assert_eq!(
            required_for(
                Claim::Verdict,
                Some(EdgeKind::Relates),
                None,
                "passing",
                Shape::default()
            )
            .required,
            Verification::Claimed
        );
        // Both DO have code: now it must point at it.
        assert_eq!(
            required_for(
                Claim::Verdict,
                Some(EdgeKind::Relates),
                None,
                "passing",
                realized
            )
            .required,
            Verification::Cited
        );
        // A finding about an intent flags nothing on disk.
        assert_eq!(
            required_for(
                Claim::Adjudication,
                None,
                None,
                "justified",
                Shape::default()
            )
            .required,
            Verification::Claimed
        );
        assert_eq!(
            required_for(
                Claim::Adjudication,
                None,
                None,
                "justified",
                Shape {
                    flagged_file: true,
                    ..Default::default()
                }
            )
            .required,
            Verification::Cited
        );
    }

    #[test]
    fn hierarchy_is_taxonomy_not_a_claim_about_code() {
        assert_eq!(
            required(Claim::Verdict, Some(EdgeKind::Hierarchy), None, "passing").required,
            Verification::Claimed
        );
    }
}
