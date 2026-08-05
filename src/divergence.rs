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

use crate::model::{EdgeKind, NodeType, TargetKind, TruthClass};
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
    // One journal parse for the whole walk. Store::ratification re-reads the
    // events file per intent; with a multi-MB journal that turns O(intents)
    // into gigabytes of repeated JSONL parsing on every status/next call.
    let journal = crate::journal::read(store.root())?;

    for intent in &intents {
        if intent.status == "deprecated" && !is_rejected(store, &intent.id, &journal)? {
            continue;
        }
        let asserted = Ratification::parse(&store.ratification_with_journal(&intent.id, &journal)?);
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

/// Facet key: when set to `escalated`, a discovered behavior leaves the
/// rectify lane and enters human ratify — the LLM inspected it and could not
/// clear the friction without a product decision.
pub const RECTIFY_FACET: &str = "rectify";
pub const RECTIFY_ESCALATED: &str = "escalated";

/// A false-duplicate decision is scoped to one peer and the exact two intent
/// descriptions inspected. The peer stays in the key; the value is the
/// description-pair fingerprint, so rewording either intent reopens the
/// heuristic without losing the prior decision note.
const RECTIFY_DUPLICATE_CLEAR_PREFIX: &str = "rectify_duplicate_clear:";

fn duplicate_clear_key(peer_id: &str) -> String {
    format!("{RECTIFY_DUPLICATE_CLEAR_PREFIX}{peer_id}")
}

fn duplicate_description_hash(a: &crate::model::Node, b: &crate::model::Node) -> String {
    let (left, right) = if a.id <= b.id { (a, b) } else { (b, a) };
    crate::artifact::fingerprint(&format!(
        "{}:{}\0{}:{}",
        left.description.len(),
        left.description,
        right.description.len(),
        right.description
    ))
}

fn duplicate_pair_is_cleared(
    store: &Store,
    a: &crate::model::Node,
    b: &crate::model::Node,
) -> Result<bool> {
    let expected = duplicate_description_hash(a, b);
    for (target, peer) in [(a, b), (b, a)] {
        if store
            .get_facet(&target.id, TargetKind::Node, &duplicate_clear_key(&peer.id))?
            .as_deref()
            == Some(expected.as_str())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Record that every currently-derived duplicate pair containing `intent_id`
/// is distinct. Returns zero when the intent is not in a live duplicate pair.
/// The decision automatically expires when either description changes.
pub fn clear_duplicate_pairs(store: &Store, intent_id: &str, reason: &str) -> Result<usize> {
    let intents = store.list_nodes(Some(NodeType::Intent), usize::MAX)?;
    let mut cleared = 0usize;
    for (a, b) in duplicate_pairs(store, &intents)? {
        let peer = if a.id == intent_id {
            b
        } else if b.id == intent_id {
            a
        } else {
            continue;
        };
        let target = if a.id == intent_id { a } else { b };
        store.set_facet(
            &target.id,
            TargetKind::Node,
            &duplicate_clear_key(&peer.id),
            &duplicate_description_hash(a, b),
            TruthClass::Asserted,
        )?;
        store.add_note(
            &target.id,
            "decision",
            &format!(
                "duplicate pair with '{}' cleared for the current descriptions: {reason}",
                peer.name
            ),
        )?;
        cleared += 1;
    }
    Ok(cleared)
}

/// Whether a blocking divergence is structural friction an LLM may clear
/// without deciding wantedness (INV-8 stays on the human ratify lane).
pub fn is_rectifiable(store: &Store, d: &Divergence) -> Result<bool> {
    if !d.blocking {
        return Ok(false);
    }
    match d.kind {
        Kind::DuplicateIntent => Ok(true),
        Kind::DiscoveredBehavior => {
            // Escalated discoveries need the human; everything else in this
            // kind is "demote visibility / relate / reword" prep work.
            let escalated = store
                .get_facet(&d.intent_id, TargetKind::Node, RECTIFY_FACET)?
                .as_deref()
                == Some(RECTIFY_ESCALATED);
            Ok(!escalated)
        }
        Kind::ZombieBehavior | Kind::PromiseBroken | Kind::MeaningDrifted => Ok(false),
    }
}

/// Blocking divergences the `rectify` lane serves.
pub fn rectifiable_count(store: &Store) -> Result<usize> {
    let mut n = 0;
    for d in all(store)? {
        if is_rectifiable(store, &d)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Blocking divergences the human `ratify` lane serves (not rectifiable).
pub fn human_blocking_count(store: &Store) -> Result<usize> {
    let mut n = 0;
    for d in all(store)? {
        if d.blocking && !is_rectifiable(store, &d)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Next rectifiable divergence, kind-first (same order as [`all`]).
pub fn next_rectifiable(store: &Store) -> Result<Option<Divergence>> {
    for d in all(store)? {
        if is_rectifiable(store, &d)? {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

/// Next human-facing blocking divergence (skips rectifiable prep work).
pub fn next_human_blocking(store: &Store) -> Result<Option<Divergence>> {
    for d in all(store)? {
        if d.blocking && !is_rectifiable(store, &d)? {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

fn make(
    intent: &crate::model::Node,
    kind: Kind,
    evidence: String,
    blocking: bool,
    blast_radius: usize,
) -> Divergence {
    let short = crate::model::short(&intent.id);
    Divergence {
        kind,
        intent_id: intent.id.clone(),
        intent_name: intent.name.clone(),
        evidence,
        ratify_command: format!(
            "loom intent ratify {short} --evidence '<why this is wanted>' --human-decision '<exact human answer>'"
        ),
        reject_command: format!(
            "loom intent reject {short} --reason '<why this is not>' --human-decision '<exact human answer>'"
        ),
        blocking,
        blast_radius,
    }
}

fn is_rejected(store: &Store, intent_id: &str, journal: &[crate::journal::Entry]) -> Result<bool> {
    Ok(store.ratification_with_journal(intent_id, journal)? == "rejected")
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
    for symbol in crate::locator::realizing_symbols(store, intent_id)? {
        total += graph.impact(&symbol, 3).callers.len();
    }
    Ok(total)
}

/// Two intents describing the same behavior: a shared realizing (file, symbol)
/// key and a tag overlap. Verifies/consumes/configures edges are seams, not
/// the behavior's home — sharing a test helper must not route as a duplicate.
/// Locator comparison uses canonical symbols so `;`-reordered members still match.
fn duplicate_pairs<'a>(
    store: &Store,
    intents: &'a [crate::model::Node],
) -> Result<Vec<(&'a crate::model::Node, &'a crate::model::Node)>> {
    use crate::model::GroundingRole;
    use std::collections::BTreeSet;

    // intent → realizing (file_id, symbol) keys. Empty symbol = module scope.
    let mut realizing: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for intent in intents {
        if intent.status == "deprecated" {
            continue;
        }
        for e in store.edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)? {
            if store.edge_superseded(&e.id)? {
                continue;
            }
            if store.grounding_role(&e.id)? != GroundingRole::Realizes {
                continue;
            }
            let file = e.to_id.clone();
            let keys = realizing.entry(intent.id.clone()).or_default();
            match store.get_facet(&e.id, TargetKind::Edge, "locator")? {
                Some(loc) if crate::locator::is_module_scope(&loc) => {
                    keys.insert((file, String::new()));
                }
                Some(loc) => {
                    for sym in crate::locator::symbols(&loc) {
                        keys.insert((file.clone(), sym));
                    }
                }
                None => {
                    keys.insert((file, String::new()));
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
            let (Some(ka), Some(kb)) = (realizing.get(&a.id), realizing.get(&b.id)) else {
                continue;
            };
            if ka.is_empty() || kb.is_empty() || ka.is_disjoint(kb) {
                continue;
            }
            let (ta, tb) = (
                tags.get(&a.id).cloned().unwrap_or_default(),
                tags.get(&b.id).cloned().unwrap_or_default(),
            );
            if jaccard(&ta, &tb) < 0.5 {
                continue;
            }
            out.push((a, b));
        }
    }
    Ok(out)
}

fn duplicates(store: &Store, intents: &[crate::model::Node]) -> Result<Vec<Divergence>> {
    let mut out = Vec::new();
    for (a, b) in duplicate_pairs(store, intents)? {
        if duplicate_pair_is_cleared(store, a, b)? {
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
    use crate::model::NodeType;
    use crate::store::Store;
    use std::fs;

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "loom-div-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let _ = fs::remove_dir_all(&p);
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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

    #[test]
    fn sharing_a_verifier_is_not_a_duplicate() {
        use crate::model::{EdgeKind, GroundingRole, TargetKind, TruthClass};

        let tmp = Tmp::new("dup-verifies");
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let a = store
            .add_node(
                NodeType::Intent,
                "behavior a",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let b = store
            .add_node(
                NodeType::Intent,
                "behavior b",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        for id in [&a.id, &b.id] {
            store.set_tag(id, TargetKind::Node, "payments").unwrap();
            store.set_tag(id, TargetKind::Node, "checkout").unwrap();
        }
        let test = store
            .add_node(
                NodeType::CodeFile,
                "tests/shared.rs",
                "",
                "present",
                serde_json::json!({}),
            )
            .unwrap();
        for intent in [&a.id, &b.id] {
            let e = store
                .add_edge(EdgeKind::Implements, intent, &test.id, TruthClass::Asserted)
                .unwrap();
            store
                .set_grounding_role(&e.id, GroundingRole::Verifies)
                .unwrap();
            store
                .set_facet(
                    &e.id,
                    TargetKind::Edge,
                    "locator",
                    "helper_assert",
                    TruthClass::Asserted,
                )
                .unwrap();
        }
        let intents = store.list_nodes(Some(NodeType::Intent), 100).unwrap();
        let dups: Vec<_> = duplicates(&store, &intents)
            .unwrap()
            .into_iter()
            .filter(|d| d.kind == Kind::DuplicateIntent)
            .collect();
        assert!(
            dups.is_empty(),
            "shared verifies grounding must not be a duplicate: {dups:?}"
        );
    }

    #[test]
    fn reordered_locator_symbols_still_match_as_duplicates() {
        use crate::model::{EdgeKind, TargetKind, TruthClass};

        let tmp = Tmp::new("dup-reorder");
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        let a = store
            .add_node(
                NodeType::Intent,
                "behavior a",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        let b = store
            .add_node(
                NodeType::Intent,
                "behavior b",
                "d",
                "implemented",
                serde_json::json!({}),
            )
            .unwrap();
        for id in [&a.id, &b.id] {
            store.set_tag(id, TargetKind::Node, "payments").unwrap();
            store.set_tag(id, TargetKind::Node, "checkout").unwrap();
        }
        let cf = store
            .add_node(
                NodeType::CodeFile,
                "src/pay.rs",
                "",
                "present",
                serde_json::json!({}),
            )
            .unwrap();
        let e_a = store
            .add_edge(EdgeKind::Implements, &a.id, &cf.id, TruthClass::Asserted)
            .unwrap();
        let e_b = store
            .add_edge(EdgeKind::Implements, &b.id, &cf.id, TruthClass::Asserted)
            .unwrap();
        store
            .set_facet(
                &e_a.id,
                TargetKind::Edge,
                "locator",
                "charge; refund",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_facet(
                &e_b.id,
                TargetKind::Edge,
                "locator",
                "refund; charge",
                TruthClass::Asserted,
            )
            .unwrap();
        let intents = store.list_nodes(Some(NodeType::Intent), 100).unwrap();
        let dups: Vec<_> = duplicates(&store, &intents)
            .unwrap()
            .into_iter()
            .filter(|d| d.kind == Kind::DuplicateIntent)
            .collect();
        assert_eq!(
            dups.len(),
            1,
            "reordered realizing symbols must still match: {dups:?}"
        );
    }

    /// `divergence::all` must parse the journal once, not once per intent.
    ///
    /// Asserted via the test-only journal read counter rather than a wall-clock
    /// bench: a bench pins the machine; the bug is the N×reread shape.
    #[test]
    fn all_reads_the_journal_once() {
        let tmp = Tmp::new("journal-once");
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        for i in 0..24 {
            let id = store
                .add_node(
                    NodeType::Intent,
                    &format!("behavior {i}"),
                    "a behavior",
                    "implemented",
                    serde_json::json!({}),
                )
                .unwrap()
                .id;
            if i % 3 == 0 {
                // loom-stability-exempt: deprecates an Intent fixture, not a proof outcome
                store.set_node_status(&id, "deprecated").unwrap();
            }
        }
        for i in 0..40 {
            crate::journal::append(
                tmp.path(),
                "note",
                &format!("target-{i}"),
                serde_json::json!({ "i": i }),
            )
            .unwrap();
        }

        crate::journal::reset_full_read_count();
        let _ = all(&store).unwrap();
        let reads = crate::journal::full_read_count();
        assert_eq!(
            reads, 1,
            "divergence::all must fully parse the journal exactly once, got {reads}"
        );
    }
}
