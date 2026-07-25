//! Ratification — whether a behavior is WANTED, and who gets to say so.
//!
//! Plane: asserted judgment (human-only, INV-8) over a derived witness.
//!
//! Contract — **asserted judgment always wins; evidence speaks only where the
//! human is silent.** An unratified intent that the code demonstrably performs,
//! that a proof loom ran covers, and that shows up in recorded usage is
//! `de_facto` wanted: it is happening, whether or not anyone said so. That is
//! not the same as approved, and loom never writes it as though it were — but
//! it is enough to stop asking, which is the point.
//!
//! A REJECTION is absolute. No amount of evidence resurrects a behavior the
//! authority killed; a rejected intent that the code still performs is not
//! wanted-after-all, it is a `ZombieBehavior` and it blocks.
//!
//! The problem this solves: the one rung requiring human attention used to
//! block the five above it, so a worker facing 51 challenge prompts forged 39
//! of them. Wantedness is now EARNED from evidence by default, and the human is
//! asked only where evidence and judgment actually diverge.

use crate::model::{EdgeKind, NodeType, TargetKind, Verification};
use crate::store::Store;
use crate::Result;
use serde::{Deserialize, Serialize};

/// The states a human may assert. `de_facto` is deliberately absent — it is
/// derived, and there is no path from caller input to it.
pub const ASSERTED_STATES: &[&str] =
    &["unratified", "ratified", "rejected", "needs_reconfirmation"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ratification {
    Unratified,
    /// Nobody said yes, but the code does it, a proof loom ran covers it, and
    /// it appears in real recorded usage.
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

    /// Does this state mean "stop asking"? `de_facto` counts; that is the whole
    /// reason it exists.
    pub fn settled(self) -> bool {
        matches!(self, Ratification::Ratified | Ratification::DeFacto)
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
    /// All three, or it is not de facto. Each is falsifiable on its own, and
    /// two of the three (D1, D2) expire with the code — so a behavior that
    /// stops being performed stops being de facto wanted.
    pub fn holds(&self) -> bool {
        self.demonstrated_in.is_some() && self.proven_by.is_some() && self.used_by.is_some()
    }
}

/// Asserted judgment always wins. Evidence speaks only into silence.
pub fn effective(asserted: Ratification, witness: Option<&DeFactoWitness>) -> Ratification {
    match asserted {
        // A rejection is absolute: evidence that the code still does it is a
        // divergence to fix, never a reason to re-approve.
        Ratification::Rejected => Ratification::Rejected,
        Ratification::Ratified => Ratification::Ratified,
        Ratification::NeedsReconfirmation => Ratification::NeedsReconfirmation,
        Ratification::Unratified | Ratification::DeFacto => match witness {
            Some(w) if w.holds() => Ratification::DeFacto,
            _ => Ratification::Unratified,
        },
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
) -> (Option<String>, usize) {
    if named.contains(intent_id) {
        return (Some("exercised directly".into()), 0);
    }
    let mut frontier = vec![intent_id.to_string()];
    let mut seen: std::collections::BTreeSet<String> = frontier.iter().cloned().collect();
    for hop in 1..=2 {
        let mut next = Vec::new();
        for id in &frontier {
            for kind in [EdgeKind::Sequence, EdgeKind::Triggers, EdgeKind::Hierarchy] {
                let Ok(edges) = store.edges_with(Some(kind), None, Some(id)) else {
                    continue;
                };
                for e in edges {
                    if !seen.insert(e.from_id.clone()) {
                        continue;
                    }
                    if named.contains(&e.from_id) {
                        return (
                            Some(format!("reached from recorded usage, {hop} hop(s)")),
                            hop,
                        );
                    }
                    next.push(e.from_id);
                }
            }
        }
        frontier = next;
    }
    (None, 0)
}

/// Intent ids named by journal entries that record real work.
///
/// One pass over the journal against one index of the intents, rather than a
/// scan of every intent per entry — a long-lived graph has both.
fn intents_in_journal(store: &Store) -> Result<std::collections::BTreeSet<String>> {
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let mut out = std::collections::BTreeSet::new();
    for entry in crate::journal::read(store.root())? {
        if !matches!(
            entry.event.as_str(),
            "drive_exchange" | "journey_run" | "validation_run" | "absorb"
        ) {
            continue;
        }
        // The target is the direct case; the payload catches an entry that
        // names the behavior without targeting it (a journey step, a drive
        // turn). Both are RECORDED usage — neither is inferred.
        if !entry.target_id.is_empty() {
            out.insert(entry.target_id.clone());
        }
        let text = entry.payload.to_string();
        for intent in &intents {
            if text.contains(&intent.id) || (!intent.name.is_empty() && text.contains(&intent.name))
            {
                out.insert(intent.id.clone());
            }
        }
    }
    Ok(out)
}

/// Recompute every intent's de-facto witness. Derived, so sync owns it and
/// `wipe_derived` + `sync` reproduces it byte-identically.
pub fn recompute(store: &Store) -> Result<usize> {
    let named = intents_in_journal(store)?;
    let mut earned = 0usize;
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated" {
            continue;
        }
        let (used_by, usage_hops) = used(store, &intent.id, &named);
        let witness = DeFactoWitness {
            demonstrated_in: demonstrated(store, &intent.id)?,
            proven_by: proven(store, &intent.id)?,
            used_by,
            usage_hops,
        };
        if witness.holds() {
            earned += 1;
        }
        store.set_facet(
            &intent.id,
            TargetKind::Node,
            "de_facto",
            &serde_json::to_string(&witness)?,
            crate::model::TruthClass::Derived,
        )?;
    }
    Ok(earned)
}

/// One intent's witness, read back.
pub fn witness(store: &Store, intent_id: &str) -> Result<Option<DeFactoWitness>> {
    Ok(store
        .get_facet(intent_id, TargetKind::Node, "de_facto")?
        .and_then(|j| serde_json::from_str(&j).ok()))
}

/// What this intent's wantedness effectively is, judgment and evidence combined.
pub fn effective_for(store: &Store, intent_id: &str) -> Result<Ratification> {
    let asserted = Ratification::parse(&store.ratification(intent_id)?);
    let w = witness(store, intent_id)?;
    Ok(effective(asserted, w.as_ref()))
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
    fn evidence_speaks_only_into_silence() {
        assert_eq!(
            effective(Ratification::Unratified, Some(&full())),
            Ratification::DeFacto
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
    }

    #[test]
    fn de_facto_settles_so_the_queue_stops_asking() {
        assert!(Ratification::DeFacto.settled());
        assert!(!Ratification::Unratified.settled());
        assert!(!Ratification::Rejected.settled());
    }
}
