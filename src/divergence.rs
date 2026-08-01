//! Divergence — where recorded judgment and observed reality disagree.
//!
//! Plane: derived query over asserted judgment and the de-facto witness.
//!
//! Contract — **an unratified intent is not a divergence.** It is ordinary
//! work-in-progress, and it belongs to build and validate, not to the human.
//! This module exists because the `wanted` rung used to be a completeness wall:
//! it counted every intent nobody had said yes to yet, which on this repo meant
//! 51 challenge prompts, of which 39 were answered by fabrication.
//!
//! A divergence is narrower and always concrete. Judgment says one thing and
//! the code says another, or two behaviors are the same behavior, or something
//! is happening that no one has ever been asked about.
//!
//! The decision that keeps the wall from reappearing is [`Kind::DiscoveredBehavior`]:
//! it blocks **only** for behavior users can see. Internal and refactor work
//! goes green on evidence alone, permanently. A solo agent can build all night;
//! the human wakes to a handful of real scope questions with the evidence
//! already attached.

use crate::model::{EdgeKind, NodeType, TargetKind};
use crate::ratification::{self, Ratification};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::collections::BTreeMap;

/// How long a ratified behavior may go unwitnessed before its silence is a
/// broken promise rather than a gap in the evidence.
pub const PROMISE_GRACE_DAYS: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Known-unwanted code is live. The authority said no and the code does it
    /// anyway — the most urgent thing in the graph.
    ZombieBehavior,
    /// Ratified, but nothing demonstrates it any more.
    PromiseBroken,
    /// Redefined after ratification: the words changed under the yes.
    MeaningDrifted,
    /// Two intents describing the same behavior.
    DuplicateIntent,
    /// Happening, and never spoken to. Blocks ONLY when users can see it.
    DiscoveredBehavior,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ZombieBehavior => "zombie_behavior",
            Kind::PromiseBroken => "promise_broken",
            Kind::MeaningDrifted => "meaning_drifted",
            Kind::DuplicateIntent => "duplicate_intent",
            Kind::DiscoveredBehavior => "discovered_behavior",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Divergence {
    pub kind: Kind,
    pub intent_id: String,
    pub intent_name: String,
    /// What is true, stated so the human can answer without opening the code.
    pub evidence: String,
    /// What to do about it — prefilled, both directions.
    pub ratify_command: String,
    pub reject_command: String,
    /// Whether this one holds the ladder. A non-blocking divergence is a
    /// visible counter, not a gate.
    pub blocking: bool,
    /// How much depends on the code behind this behavior; ranks the queue.
    pub blast_radius: usize,
}

/// Every divergence in the graph, ranked: kind first (a zombie outranks a
/// question), then blast radius, then name.
pub fn all(store: &Store) -> Result<Vec<Divergence>> {
    let mut out = Vec::new();
    let graph = crate::callgraph::build(store)?;
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;

    for intent in &intents {
        if intent.status == "deprecated" && !is_rejected(store, &intent.id)? {
            continue;
        }
        let asserted = Ratification::parse(&store.ratification(&intent.id)?);
        let witness = ratification::witness(store, &intent.id)?;
        let demonstrated = witness.as_ref().and_then(|w| w.demonstrated_in.clone());
        let holds = witness.as_ref().map(|w| w.holds()).unwrap_or(false);
        let radius = blast_radius(store, &graph, &intent.id)?;

        let found = match asserted {
            // The code still performs something the authority killed.
            Ratification::Rejected if demonstrated.is_some() => Some((
                Kind::ZombieBehavior,
                format!(
                    "rejected, but still realized in {}",
                    demonstrated.clone().unwrap_or_default()
                ),
                true,
            )),
            // Someone said yes, and nothing backs it any more.
            Ratification::Ratified if !holds && past_grace(store, &intent.id)? => Some((
                Kind::PromiseBroken,
                missing_conjunct(witness.as_ref()),
                true,
            )),
            Ratification::NeedsReconfirmation => Some((
                Kind::MeaningDrifted,
                "redefined after ratification — the words changed under the yes".into(),
                true,
            )),
            // Happening, never spoken to. The one row that is usually silent.
            Ratification::Unratified | Ratification::DeFacto if holds => {
                let visible = store
                    .get_facet(&intent.id, TargetKind::Node, "visibility")?
                    .as_deref()
                    == Some("user_visible");
                Some((
                    Kind::DiscoveredBehavior,
                    witness
                        .as_ref()
                        .map(describe_witness)
                        .unwrap_or_else(|| "in evidence".into()),
                    visible,
                ))
            }
            _ => None,
        };

        if let Some((kind, evidence, blocking)) = found {
            out.push(make(intent, kind, evidence, blocking, radius));
        }
    }

    out.extend(duplicates(store, &intents)?);
    out.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then(b.blast_radius.cmp(&a.blast_radius))
            .then(a.intent_name.cmp(&b.intent_name))
    });
    Ok(out)
}

/// Only blocking divergences gate the ladder.
pub fn blocking_count(store: &Store) -> Result<usize> {
    Ok(all(store)?.iter().filter(|d| d.blocking).count())
}

fn make(
    intent: &crate::model::Node,
    kind: Kind,
    evidence: String,
    blocking: bool,
    blast_radius: usize,
) -> Divergence {
    let short = &intent.id[..8.min(intent.id.len())];
    Divergence {
        kind,
        intent_id: intent.id.clone(),
        intent_name: intent.name.clone(),
        evidence,
        ratify_command: format!("loom intent ratify {short} --evidence '<why this is wanted>'"),
        reject_command: format!("loom intent reject {short} --reason '<why this is not>'"),
        blocking,
        blast_radius,
    }
}

fn is_rejected(store: &Store, intent_id: &str) -> Result<bool> {
    Ok(store.ratification(intent_id)? == "rejected")
}

/// Name the conjunct that fell, so a broken promise says WHAT broke.
fn missing_conjunct(witness: Option<&ratification::DeFactoWitness>) -> String {
    match witness {
        None => "nothing in the graph demonstrates this behavior".into(),
        Some(w) => {
            let mut missing = Vec::new();
            if w.demonstrated_in.is_none() {
                missing.push("no live grounding");
            }
            if w.proven_by.is_none() {
                missing.push("no proof loom ran");
            }
            if w.used_by.is_none() {
                missing.push("no recorded usage");
            }
            format!("ratified, but {}", missing.join(", "))
        }
    }
}

fn describe_witness(w: &ratification::DeFactoWitness) -> String {
    format!(
        "realized in {}, proven by {}, {}",
        w.demonstrated_in.as_deref().unwrap_or("?"),
        w.proven_by.as_deref().unwrap_or("?"),
        w.used_by.as_deref().unwrap_or("?")
    )
}

/// Has a ratified behavior been unwitnessed long enough to call it broken?
///
/// The grace period matters: a behavior whose proof is merely stale after
/// today's edit is not a broken promise, it is a proof to re-run, and calling
/// it a divergence would put ordinary build churn in front of the human.
fn past_grace(store: &Store, intent_id: &str) -> Result<bool> {
    let Some((_, at)) = store.ratified_by(intent_id)? else {
        return Ok(true);
    };
    let now = crate::journal::now_iso();
    Ok(days_between(&at, &now) > PROMISE_GRACE_DAYS)
}

/// Whole days between two ISO-8601 stamps, by date part only. Deliberately
/// coarse — the grace period is a week, not a duration to be precise about.
fn days_between(a: &str, b: &str) -> i64 {
    fn ord(s: &str) -> Option<i64> {
        let d = s.split('T').next()?;
        let mut parts = d.split('-');
        let y: i64 = parts.next()?.parse().ok()?;
        let m: i64 = parts.next()?.parse().ok()?;
        let day: i64 = parts.next()?.parse().ok()?;
        // Days since an arbitrary epoch, good enough for a week-scale window.
        Some(y * 365 + m * 31 + day)
    }
    match (ord(a), ord(b)) {
        (Some(x), Some(y)) => y - x,
        _ => 0,
    }
}

/// How much of the codebase reaches the code behind this behavior.
fn blast_radius(
    store: &Store,
    graph: &crate::callgraph::CallGraph,
    intent_id: &str,
) -> Result<usize> {
    let mut total = 0usize;
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? else {
            continue;
        };
        let Some(tok) = loc.split_whitespace().next_back() else {
            continue;
        };
        let symbol = tok.split(':').next().unwrap_or(tok);
        let symbol = symbol.rsplit("::").next().unwrap_or(symbol);
        total += graph.impact(symbol, 3).callers.len();
    }
    Ok(total)
}

/// Two intents describing the same behavior: shared realizing files, a tag
/// overlap, and a shared locator. All three, because any one alone is common
/// and harmless — two behaviors in one file is normal, and so is a shared tag.
fn duplicates(store: &Store, intents: &[crate::model::Node]) -> Result<Vec<Divergence>> {
    let mut files: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut locators: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for intent in intents {
        if intent.status == "deprecated" {
            continue;
        }
        for e in store.edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)? {
            if store.edge_superseded(&e.id)? {
                continue;
            }
            files
                .entry(intent.id.clone())
                .or_default()
                .push(e.to_id.clone());
            if let Some(loc) = store.get_facet(&e.id, TargetKind::Edge, "locator")? {
                if !loc.trim().is_empty() {
                    locators.entry(intent.id.clone()).or_default().push(loc);
                }
            }
        }
        tags.insert(
            intent.id.clone(),
            store.tags_of(&intent.id, TargetKind::Node)?,
        );
    }

    let mut out = Vec::new();
    // Scenario siblings (children of one parent, or a parent and its child)
    // are distinct surroundings of one behavior, not duplicates of each
    // other — a sad path and its edge case share the parent's file and often
    // its locator by construction. Exempt the pair before the heuristic
    // flags it.
    let scenario_children: BTreeMap<String, Vec<String>> = store
        .edges_with(Some(EdgeKind::ScenarioOf), None, None)?
        .into_iter()
        .fold(BTreeMap::new(), |mut m, e| {
            m.entry(e.from_id.clone())
                .or_default()
                .push(e.to_id.clone());
            m
        });
    for (i, a) in intents.iter().enumerate() {
        for b in intents.iter().skip(i + 1) {
            // Related surroundings are not duplicates: a parent and its
            // scenario, or two scenarios of one parent.
            let a_parents = scenario_children.get(&a.id).cloned().unwrap_or_default();
            let b_parents = scenario_children.get(&b.id).cloned().unwrap_or_default();
            let parent_child = a_parents.contains(&b.id) || b_parents.contains(&a.id);
            let siblings = a_parents.iter().any(|p| b_parents.contains(p));
            if parent_child || siblings {
                continue;
            }
            let (Some(fa), Some(fb)) = (files.get(&a.id), files.get(&b.id)) else {
                continue;
            };
            if !fa.iter().any(|f| fb.contains(f)) {
                continue;
            }
            let (Some(la), Some(lb)) = (locators.get(&a.id), locators.get(&b.id)) else {
                continue;
            };
            if !la.iter().any(|l| lb.contains(l)) {
                continue;
            }
            let (ta, tb) = (
                tags.get(&a.id).cloned().unwrap_or_default(),
                tags.get(&b.id).cloned().unwrap_or_default(),
            );
            if jaccard(&ta, &tb) < 0.5 {
                continue;
            }
            out.push(make(
                b,
                Kind::DuplicateIntent,
                format!("describes the same behavior as '{}'", a.name),
                true,
                0,
            ));
        }
    }
    Ok(out)
}

fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.iter().filter(|t| b.contains(t)).count() as f64;
    let union = (a.len() + b.len()) as f64 - inter;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Intents that are not divergences but are worth a visible number: behavior
/// loom discovered that users cannot see. Green on evidence alone, forever.
pub fn silent_discoveries(store: &Store) -> Result<usize> {
    Ok(all(store)?
        .iter()
        .filter(|d| d.kind == Kind::DiscoveredBehavior && !d.blocking)
        .count())
}

/// Intents whose type makes them a divergence subject at all — used by the
/// queue to avoid re-deriving the list.
pub fn is_subject(node: &crate::model::Node) -> bool {
    node.node_type == NodeType::Intent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zombie_outranks_a_question() {
        assert!(Kind::ZombieBehavior < Kind::DiscoveredBehavior);
        assert!(Kind::PromiseBroken < Kind::DuplicateIntent);
    }

    #[test]
    fn the_grace_window_is_days_not_edits() {
        assert_eq!(
            days_between("2026-07-01T00:00:00Z", "2026-07-09T00:00:00Z"),
            8
        );
        assert!(days_between("2026-07-01T00:00:00Z", "2026-07-05T00:00:00Z") <= PROMISE_GRACE_DAYS);
    }

    #[test]
    fn duplicate_similarity_needs_real_overlap() {
        let a = vec!["payments".to_string(), "checkout".to_string()];
        let b = vec!["payments".to_string(), "checkout".to_string()];
        assert_eq!(jaccard(&a, &b), 1.0);
        let c = vec!["payments".to_string(), "refunds".to_string(), "x".into()];
        assert!(jaccard(&a, &c) < 0.5, "one shared tag is not a duplicate");
    }
}
