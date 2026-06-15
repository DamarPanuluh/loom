//! RELATES_TO snapshot helpers.

use crate::types::RelatesTo;

use super::snapshot::QuerySnapshot;

pub fn unresolved_edges_for_intent_from_snapshot(
    snapshot: &QuerySnapshot,
    intent_id: &str,
) -> Vec<RelatesTo> {
    unresolved_edges(
        snapshot
            .relates
            .iter()
            .filter(|edge| edge.from_id == intent_id || edge.to_id == intent_id)
            .cloned()
            .collect(),
    )
}

fn unresolved_edges(edges: Vec<RelatesTo>) -> Vec<RelatesTo> {
    edges
        .into_iter()
        .filter(|e| {
            matches!(
                e.inspection_status.as_str(),
                "uninspected" | "failing" | "needs_reverification"
            )
        })
        .collect()
}
