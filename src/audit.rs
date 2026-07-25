//! Audit — does this graph's own record look like it was earned?
//!
//! Plane: statistical detection over asserted facts and the journal. Findings
//! route through ordinary triage; nothing here gates directly (INV-3).
//!
//! Contract — **built from the incident, then run on ourselves.** Every check
//! below is a signature loom's own graph carried:
//!
//! - 30 ratifications sharing one journal minute, 9 more paced 25–40 seconds
//!   apart. Nobody reads and judges 30 behaviors in a minute.
//! - 39 of 51 ratifications with no journal entry behind them at all — the
//!   facet was written directly, past the gate that was supposed to be the
//!   only way in.
//! - 54 of 59 proofs whose "passing" verdict cited prose about a run that loom
//!   never performed.
//!
//! The point is not that these are now impossible — the evidence spine makes
//! most of them impossible going forward. The point is that a graph can be
//! IMPORTED, or carried forward, or written by a version of loom without these
//! guards, and a tool whose whole claim is falsifiability has to be able to
//! turn that claim on its own records.

use crate::model::{Claim, NodeType, Verification};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::collections::BTreeMap;

/// Writes by one actor inside one minute that stop looking like judgment.
pub const BURST_THRESHOLD: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub kind: &'static str,
    pub subject: String,
    pub detail: String,
    /// What to do — every finding names its own remedy, because an audit that
    /// only accuses is a scoreboard.
    pub remedy: String,
}

/// Every self-fabrication signature in this graph.
pub fn run(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    out.extend(unjournaled_ratifications(store)?);
    out.extend(bursts(store)?);
    out.extend(unanchored_settled_facts(store)?);
    out.sort_by(|a, b| a.kind.cmp(b.kind).then(a.subject.cmp(&b.subject)));
    Ok(out)
}

/// A `ratified` fact with no journal entry behind it.
///
/// loom writes the entry BEFORE stamping the fact, so on a graph this version
/// produced the invariant holds by construction. A violation therefore means
/// one of two things, and both are worth knowing: the record predates the
/// spine, or something wrote past the boundary.
fn unjournaled_ratifications(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        let state = store.ratification(&intent.id)?;
        if state != "ratified" && state != "rejected" {
            continue;
        }
        let has_entry = crate::journal::read(store.root())?.iter().any(|e| {
            matches!(e.event.as_str(), "ratification" | "rejection") && e.target_id == intent.id
        });
        if !has_entry {
            out.push(AuditFinding {
                kind: "unjournaled_ratification",
                subject: intent.id.clone(),
                detail: format!(
                    "'{}' is {state} with no journal entry behind it — the act was never recorded",
                    intent.name
                ),
                remedy: format!(
                    "re-ratify it deliberately (`loom intent ratify {}`), or reject it",
                    &intent.id[..8.min(intent.id.len())]
                ),
            });
        }
    }
    Ok(out)
}

/// Many asserted writes by one actor inside one minute.
///
/// Statistical, and reported as such: a legitimate bulk import looks like this
/// too. What it says is "these were not individually judged", which is exactly
/// what the 30-at-19:20 cluster meant.
fn bursts(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut buckets: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for fact in store.all_facts()? {
        if fact.claim != Claim::Ratification && fact.claim != Claim::Adjudication {
            continue;
        }
        // Minute precision: the timestamps are ISO-8601, so truncating at the
        // colon before seconds is the whole grouping key.
        let minute: String = fact.asserted_at.chars().take(16).collect();
        buckets
            .entry((fact.asserted_by.clone(), minute))
            .or_default()
            .push(fact.subject_id.clone());
    }
    let mut out = Vec::new();
    for ((actor, minute), subjects) in buckets {
        if subjects.len() < BURST_THRESHOLD {
            continue;
        }
        out.push(AuditFinding {
            kind: "judgment_burst",
            subject: format!("{actor}@{minute}"),
            detail: format!(
                "{} judgments by '{actor}' inside one minute ({minute}) — \
                 too fast to have been made one at a time",
                subjects.len()
            ),
            remedy: "re-open them and judge them individually, or record that this was \
                     a bulk import and what authorized it"
                .into(),
        });
    }
    Ok(out)
}

/// A settled fact standing on nothing re-checkable.
///
/// The spine refuses these at write time now, so a hit means the fact arrived
/// some other way: an import, a carry-forward, or a graph written before the
/// floors existed.
fn unanchored_settled_facts(store: &Store) -> Result<Vec<AuditFinding>> {
    let mut out = Vec::new();
    for fact in store.all_facts()? {
        if !crate::anchor::is_settling(&fact.state) {
            continue;
        }
        if fact.verification != Verification::Expired {
            continue;
        }
        out.push(AuditFinding {
            kind: "unanchored_claim",
            subject: fact.subject_id.clone(),
            detail: format!(
                "{} is '{}' with no surviving anchor",
                fact.claim.as_str(),
                fact.state
            ),
            remedy: "re-establish it with evidence loom can re-check, or withdraw it".into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_burst_threshold_is_about_reading_speed() {
        // Ten judgments in sixty seconds is six seconds each, including
        // reading the behavior and its evidence. The number is a claim about
        // humans, not about the database.
        assert_eq!(BURST_THRESHOLD, 10);
    }
}
