//! Ratification — whether a behavior is WANTED, and who gets to say so.
//!
//! Plane: asserted judgment (human-authorized, INV-8) alongside a derived
//! technical witness.
//!
//! Contract — **technical evidence never establishes wantedness.** Grounding,
//! proof strength, verdicts, and recorded usage form an inspectable witness that
//! the behavior exists. They may expose a discovery or a conflict, but without
//! a direct or mediated human decision its ratification remains `unratified`.
//!
//! A REJECTION is absolute. No amount of evidence resurrects a behavior the
//! authority killed; a rejected intent that the code still performs is not
//! wanted-after-all, it is a `ZombieBehavior` and it blocks.
//!
//! Missing approval is projected explicitly without turning every unratified
//! behavior into an interruption: the human is asked only where evidence and
//! judgment actually diverge.

use crate::model::{EdgeKind, NodeType, TargetKind, Verification};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde::{Deserialize, Serialize};

/// The states a human may assert. `de_facto` is deliberately absent — it is
/// derived, and there is no path from caller input to it.
pub const ASSERTED_STATES: &[&str] =
    &["unratified", "ratified", "rejected", "needs_reconfirmation"];

/// Evidence that a human made the product decision.
///
/// This separates the authority from the executor. A direct decision is made
/// and recorded by the person running Loom. A mediated decision is made by a
/// person in the host conversation, then recorded mechanically by an LLM. The
/// latter is deliberately explicit: merely running in an LLM lane never
/// acquires product authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HumanDecision {
    Direct { presence: String },
    Mediated { response: String },
}

impl HumanDecision {
    /// A person made and recorded the decision in the same interaction.
    pub fn direct(presence: impl Into<String>) -> Result<Self> {
        let presence = presence.into();
        if presence.trim().is_empty() {
            bail!("ratification requires a human-presence descriptor");
        }
        Ok(Self::Direct { presence })
    }

    /// A person answered in the host conversation and an LLM is recording the
    /// answer. `response` is the human's actual answer, not the LLM's summary.
    pub fn mediated(response: impl Into<String>) -> Result<Self> {
        let response = response.into();
        if response.trim().is_empty() || crate::model::is_placeholder(&response) {
            bail!(
                "--human-decision must contain the human's actual answer; silence or a placeholder is not authority"
            );
        }
        Ok(Self::Mediated { response })
    }

    pub(crate) fn presence(&self) -> &str {
        match self {
            Self::Direct { presence } => presence,
            Self::Mediated { .. } => "host-mediated",
        }
    }

    pub(crate) fn permits_mediated_recording(&self) -> bool {
        matches!(self, Self::Mediated { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ratification {
    Unratified,
    /// Legacy serialized state retained for compatibility only. A technical
    /// witness is not authority, so [`effective`] normalizes this to
    /// [`Ratification::Unratified`] and it is never settled.
    DeFacto,
    Ratified,
    Rejected,
    NeedsReconfirmation,
}

impl Ratification {
    pub fn as_str(self) -> &'static str {
        match self {
            Ratification::Unratified => "unratified",
            Ratification::DeFacto => "de_facto",
            Ratification::Ratified => "ratified",
            Ratification::Rejected => "rejected",
            Ratification::NeedsReconfirmation => "needs_reconfirmation",
        }
    }

    pub fn parse(s: &str) -> Ratification {
        match s {
            "ratified" => Ratification::Ratified,
            "rejected" => Ratification::Rejected,
            "needs_reconfirmation" => Ratification::NeedsReconfirmation,
            "de_facto" => Ratification::DeFacto,
            _ => Ratification::Unratified,
        }
    }

    /// Does this state carry a current human approval?
    pub fn settled(self) -> bool {
        matches!(self, Ratification::Ratified)
    }
}

/// The three conjuncts, each recorded so the answer can be argued with.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeFactoWitness {
    /// D1 — a live realizing grounding whose anchors still hold.
    pub demonstrated_in: Option<String>,
    /// D2 — a proof loom itself ran, at S2 or better.
    pub proven_by: Option<String>,
    /// D3 — recorded usage naming this intent, and how far away it was.
    pub used_by: Option<String>,
    pub usage_hops: usize,
}

impl DeFactoWitness {
    /// All three make a complete technical witness. Each is falsifiable on its
    /// own, and two of the three (D1, D2) expire with the code, so the witness
    /// stops holding when the behavior stops being performed or proven.
    pub fn holds(&self) -> bool {
        self.demonstrated_in.is_some() && self.proven_by.is_some() && self.used_by.is_some()
    }
}

/// Project authoritative wantedness without deriving it from technical facts.
///
/// The witness remains available through [`witness`] for discovery and
/// diagnostics, but cannot settle this projection. `DeFacto` is a legacy wire
/// value and therefore fails closed to `Unratified`.
pub fn effective(asserted: Ratification, _witness: Option<&DeFactoWitness>) -> Ratification {
    match asserted {
        Ratification::DeFacto => Ratification::Unratified,
        state => state,
    }
}

/// D1 — the code demonstrably does this.
fn demonstrated(store: &Store, intent_id: &str) -> Result<Option<String>> {
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)?
            || store.grounding_role(&e.id)? != crate::model::GroundingRole::Realizes
        {
            continue;
        }
        if !matches!(
            e.status,
            crate::model::InspectionStatus::Passing | crate::model::InspectionStatus::Independent
        ) {
            continue;
        }
        // The anchors have to still hold — a grounding whose citations rotted
        // says the behavior USED to live there.
        if store.edge_verification(&e.id)?.counts() {
            if let Some(cf) = store.get_node(&e.to_id)? {
                return Ok(Some(cf.name));
            }
        }
    }
    Ok(None)
}

/// D2 — a proof loom itself ran passes, and establishes behavior rather than
/// liveness. S1 does not count: that clause is what stops a smoke test minting
/// wantedness.
fn proven(store: &Store, intent_id: &str) -> Result<Option<String>> {
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        if e.status != crate::model::InspectionStatus::Passing {
            continue;
        }
        if store.edge_verification(&e.id)? != Verification::Verified {
            continue;
        }
        if crate::proofstrength::of(store, &e.from_id)? < crate::proofstrength::Strength::MEANINGFUL
        {
            continue;
        }
        if let Some(v) = store.get_node(&e.from_id)? {
            return Ok(Some(v.name));
        }
    }
    Ok(None)
}

/// D3 — reachable from real recorded usage: a journal entry naming this intent,
/// or within two hops of one over sequence/triggers/hierarchy.
///
/// Historical rather than freshness-bearing (the journal is append-only,
/// INV-9). D1 or D2 falling after D3 held is not silent expiry — it is the
/// `PromiseBroken` divergence.
fn used(
    store: &Store,
    intent_id: &str,
    named: &std::collections::BTreeSet<String>,
) -> Result<(Option<String>, usize)> {
    if named.contains(intent_id) {
        return Ok((Some("exercised directly".into()), 0));
    }
    let mut frontier = vec![intent_id.to_string()];
    let mut seen: std::collections::BTreeSet<String> = frontier.iter().cloned().collect();
    for hop in 1..=2 {
        let mut next = Vec::new();
        for id in &frontier {
            for from in callers_of(store, id)? {
                if !seen.insert(from.clone()) {
                    continue;
                }
                if named.contains(&from) {
                    return Ok((
                        Some(format!("reached from recorded usage, {hop} hop(s)")),
                        hop,
                    ));
                }
                next.push(from);
            }
        }
        frontier = next;
    }
    Ok((None, 0))
}

/// Everything one hop upstream of an intent over the flow relationships.
///
/// Split out of `used` so the walk reads as a breadth-first search rather than
/// four nested loops — the shape loom flagged.
fn callers_of(store: &Store, intent_id: &str) -> Result<Vec<String>> {
    let mut callers = Vec::new();
    for kind in [EdgeKind::Sequence, EdgeKind::Triggers, EdgeKind::Hierarchy] {
        callers.extend(
            store
                .edges_with(Some(kind), None, Some(intent_id))?
                .into_iter()
                .map(|edge| edge.from_id),
        );
    }
    Ok(callers)
}

/// Intent ids with RECORDED USAGE: a validation or journey that validates
/// them has actually run — its validates edge carries run evidence.
///
/// This is a pure function of the graph, which travels in the export. The
/// previous implementation scanned the LOCAL journal's captured output
/// excerpts (a journey run embeds the step command's stdout), and captured
/// output changes as the graph changes — so a fresh import never reproduced
/// the same `named` set (the `used_by` witness churned on 10 of 763 nodes).
/// Recorded usage now means "exercised by a proof loom watched run", not
/// "mentioned in some captured output".
fn intents_with_recorded_usage(store: &Store) -> Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    for e in store.edges_with(Some(EdgeKind::Validates), None, None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        let Some(view) = store.fact(
            &crate::store::Subject::Edge(e.id.clone()),
            crate::model::Claim::Verdict,
        )?
        else {
            continue;
        };
        if view
            .evidence
            .iter()
            .any(|row| matches!(row.payload, crate::evidence::Evidence::Run(_)))
        {
            out.insert(e.to_id.clone());
        }
    }
    Ok(out)
}

/// Recompute every intent's technical witness. Derived, so sync owns it and
/// `wipe_derived` + `sync` reproduces it byte-identically. The historical
/// `de_facto` facet key is retained for export compatibility; its contents do
/// not carry ratification authority.
pub fn recompute(store: &Store) -> Result<usize> {
    let named = intents_with_recorded_usage(store)?;
    let mut complete = 0usize;
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated" {
            continue;
        }
        let (used_by, usage_hops) = used(store, &intent.id, &named)?;
        let witness = DeFactoWitness {
            demonstrated_in: demonstrated(store, &intent.id)?,
            proven_by: proven(store, &intent.id)?,
            used_by,
            usage_hops,
        };
        if witness.holds() {
            complete += 1;
        }
        store.set_facet(
            &intent.id,
            TargetKind::Node,
            "de_facto",
            &serde_json::to_string(&witness)?,
            crate::model::TruthClass::Derived,
        )?;
    }
    Ok(complete)
}

/// One intent's witness, read back.
pub fn witness(store: &Store, intent_id: &str) -> Result<Option<DeFactoWitness>> {
    Ok(store
        .get_facet(intent_id, TargetKind::Node, "de_facto")?
        .and_then(|j| serde_json::from_str(&j).ok()))
}

/// What this intent's authoritative wantedness is.
///
/// Technical evidence is deliberately not consulted here; callers that need
/// the independently derived observation use [`witness`].
pub fn effective_for(store: &Store, intent_id: &str) -> Result<Ratification> {
    let asserted = Ratification::parse(&store.ratification(intent_id)?);
    Ok(effective(asserted, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> DeFactoWitness {
        DeFactoWitness {
            demonstrated_in: Some("src/a.rs".into()),
            proven_by: Some("a test".into()),
            used_by: Some("exercised directly".into()),
            usage_hops: 0,
        }
    }

    #[test]
    fn a_rejection_is_absolute() {
        assert_eq!(
            effective(Ratification::Rejected, Some(&full())),
            Ratification::Rejected,
            "no amount of evidence resurrects a behavior the authority killed"
        );
    }

    #[test]
    fn technical_evidence_never_establishes_wantedness() {
        assert_eq!(
            effective(Ratification::Unratified, Some(&full())),
            Ratification::Unratified,
            "a complete technical witness is not human authority"
        );
        assert_eq!(
            effective(Ratification::Unratified, None),
            Ratification::Unratified
        );
        // A partial witness is not a witness. Each conjunct answers a different
        // question, and two of three is silence about the third.
        let mut partial = full();
        partial.proven_by = None;
        assert_eq!(
            effective(Ratification::Unratified, Some(&partial)),
            Ratification::Unratified
        );
        assert_eq!(
            effective(Ratification::Ratified, Some(&full())),
            Ratification::Ratified,
            "an explicit human ratification remains authoritative"
        );
    }

    #[test]
    fn legacy_de_facto_is_not_settled_or_authoritative() {
        assert_eq!(Ratification::parse("de_facto"), Ratification::DeFacto);
        assert_eq!(
            effective(Ratification::DeFacto, Some(&full())),
            Ratification::Unratified
        );
        assert!(!Ratification::DeFacto.settled());
        assert!(!Ratification::Unratified.settled());
        assert!(!Ratification::Rejected.settled());
        assert!(Ratification::Ratified.settled());
    }

    #[test]
    fn mediated_authority_requires_an_actual_human_answer() {
        for answer in ["", "  ", "…", "todo", "<answer>"] {
            assert!(
                HumanDecision::mediated(answer).is_err(),
                "{answer:?} is not a human decision"
            );
        }
        assert!(HumanDecision::mediated("Keep behavior").is_ok());
    }
}
