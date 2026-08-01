//! Statistical debt feed: size outliers + optional co-change clusters.
//!
//! INV-3: debt is computed on demand, never stored as an edge or counted as
//! required work. Git unavailability skips co-change silently.

#[path = "debt/cochange.rs"]
mod cochange;
#[path = "debt/git.rs"]
mod git;

use crate::store::{Snapshot, Store};
use crate::Result;
use cochange::co_change_clusters;
use git::{read_git_history, HistoryAvailability};
use serde::Serialize;

/// A statistical debt signal: ranked, advisory, never stored.
#[derive(Debug, Clone, Serialize)]
pub struct DebtCluster {
    pub kind: String,
    pub message: String,
    pub impact: u32,
    pub confirm: String,
    /// Stable content-addressed id (`c` + 16 hex). Serialized last so existing
    /// JSON consumers that ignore unknown trailing fields stay compatible.
    pub cluster_id: String,
    /// Sorted, deduped CodeFile node ids this cluster is about. Never serialized
    /// into the feed — promotion re-reads them from the live cluster.
    #[serde(skip)]
    pub subject_ids: Vec<String>,
}

/// Content-addressed id for a debt cluster: `c` + FNV-1a digest over
/// `["debt-cluster", kind, sorted-deduped subjects…]`.
pub fn debt_cluster_id(kind: &str, subject_ids: &[String]) -> String {
    let kind = kind.trim();
    let mut subjects: Vec<&str> = subject_ids
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    subjects.sort_unstable();
    subjects.dedup();
    let mut parts: Vec<&str> = Vec::with_capacity(2 + subjects.len());
    parts.push("debt-cluster");
    parts.push(kind);
    parts.extend(subjects);
    format!("c{}", crate::store::fnv_hex_digest(&parts))
}

/// Statistical debt feed: size outliers plus optional co-change clusters from
/// git history. Never writes. Git unavailability skips co-change silently.
pub fn debt(store: &Store) -> Result<Vec<DebtCluster>> {
    let snap = store.snapshot()?;
    let mut out = size_outlier_clusters(&snap);
    if let HistoryAvailability::Available(history) = read_git_history(store.root()) {
        out.extend(co_change_clusters(&snap, &history));
    }
    out.sort_by(|a, b| {
        b.impact
            .cmp(&a.impact)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.cluster_id.cmp(&b.cluster_id))
    });
    Ok(out)
}

/// Size outliers: files whose loc exceeds the Tukey upper fence of the repo.
/// (a statistical signal computed on demand — never stored, never required.)
fn size_outlier_clusters(snap: &Snapshot) -> Vec<DebtCluster> {
    let mut out = Vec::new();
    // Only CodeFile `loc` counts. Other node kinds (and edges) can carry a
    // facet named `loc` for unrelated reasons; pooling them skews the Tukey
    // fence and, worse, emits non-CodeFile debt subjects that promotion cannot
    // resolve. Restrict the population to CodeFile-node facets.
    let code_files: std::collections::BTreeSet<&str> = snap
        .nodes
        .iter()
        .filter(|n| n.node_type == crate::model::NodeType::CodeFile)
        .map(|n| n.id.as_str())
        .collect();
    let locs: Vec<(String, f64)> = snap
        .facets
        .iter()
        .filter(|f| {
            f.key == "loc"
                && f.target_kind == crate::model::TargetKind::Node
                && code_files.contains(f.target_id.as_str())
        })
        .filter_map(|f| {
            f.value
                .parse::<f64>()
                .ok()
                .map(|v| (f.target_id.clone(), v))
        })
        .collect();
    if locs.len() >= 4 {
        let mut vals: Vec<f64> = locs.iter().map(|(_, v)| *v).collect();
        vals.sort_by(|a, b| a.total_cmp(b));
        let q1 = super::quantile(&vals, 0.25);
        let q3 = super::quantile(&vals, 0.75);
        let fence = q3 + 1.5 * (q3 - q1);
        for (id, v) in &locs {
            if *v > fence && *v > 200.0 {
                let subject_ids = vec![id.clone()];
                let cluster_id = debt_cluster_id("size_outlier", &subject_ids);
                out.push(DebtCluster {
                    kind: "size_outlier".into(),
                    message: format!(
                        "{} is {} loc (repo upper fence {:.0})",
                        super::node_name(snap, id),
                        *v as u64,
                        fence
                    ),
                    impact: *v as u32,
                    confirm:
                        "your call: split it if it's tangled, or justify the size as genuine cohesion — judge and act, don't defer to a human"
                            .into(),
                    cluster_id,
                    subject_ids,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod debt_tests {
    use super::*;
    use crate::model::{Facet, Node, NodeType, TargetKind, TruthClass};
    use crate::store::{Identity, Snapshot};
    use std::collections::BTreeMap;

    fn codefile(id: &str, path: &str) -> Node {
        Node {
            id: id.into(),
            node_type: NodeType::CodeFile,
            name: path.into(),
            description: String::new(),
            status: String::new(),
            truth_class: TruthClass::Derived,
            body: serde_json::json!({}),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn empty_snap(nodes: Vec<Node>) -> Snapshot {
        Snapshot {
            facts: Vec::new(),
            evidence: Vec::new(),
            identity: Identity {
                graph_id: "g".into(),
                name: "t".into(),
                schema_version: crate::SCHEMA_VERSION,
                observed: false,
            },
            nodes,
            edges: Vec::new(),
            facets: Vec::new(),
            tags: Vec::new(),
            config: BTreeMap::new(),
        }
    }

    #[test]
    fn debt_cluster_id_is_stable_fnv_fixture() {
        // Guards the FNV contract: same parts → same c-prefixed digest.
        let id = debt_cluster_id("size_outlier", &["file-a".into()]);
        assert_eq!(id, "c92ce4fda68f6b207");
        let id2 = debt_cluster_id("co_change", &["id_b".into(), "id_a".into(), "id_a".into()]);
        assert_eq!(id2, "ccb881abd66861e42");
        let id3 = debt_cluster_id("  size_outlier  ", &["file-a".into()]);
        assert_eq!(id3, id);
    }

    #[test]
    fn size_outlier_clusters_attach_ids() {
        let mut snap = empty_snap(vec![
            codefile("n1", "big.rs"),
            codefile("n2", "a.rs"),
            codefile("n3", "b.rs"),
            codefile("n4", "c.rs"),
            codefile("n5", "d.rs"),
        ]);
        for (id, loc) in [
            ("n1", "500"),
            ("n2", "10"),
            ("n3", "12"),
            ("n4", "11"),
            ("n5", "13"),
        ] {
            snap.facets.push(Facet {
                target_id: id.into(),
                target_kind: TargetKind::Node,
                key: "loc".into(),
                value: loc.into(),
                truth_class: TruthClass::Derived,
            });
        }
        let clusters = size_outlier_clusters(&snap);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].kind, "size_outlier");
        assert_eq!(clusters[0].subject_ids, vec!["n1".to_string()]);
        assert_eq!(
            clusters[0].cluster_id,
            debt_cluster_id("size_outlier", &["n1".into()])
        );
        assert_eq!(clusters[0].impact, 500);
    }
}
