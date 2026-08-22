use crate::model::{GroundingRole, TargetKind};
use crate::store::Snapshot;

/// Grounding role of an `implements` edge, read from the snapshot facets — a
/// pure mirror of `Store::grounding_role`. A missing `role` facet reads as
/// `Realizes` (pre-role default).
pub(crate) fn edge_role(snap: &Snapshot, edge_id: &str) -> GroundingRole {
    snap.facets
        .iter()
        .find(|f| f.target_kind == TargetKind::Edge && f.target_id == edge_id && f.key == "role")
        .and_then(|f| f.value.parse().ok())
        .unwrap_or(GroundingRole::Realizes)
}

/// Whether an edge was superseded by a `rehome` (bears a `superseded_by` facet).
pub(crate) fn edge_is_superseded(snap: &Snapshot, edge_id: &str) -> bool {
    snap.facets.iter().any(|f| {
        f.target_kind == TargetKind::Edge && f.target_id == edge_id && f.key == "superseded_by"
    })
}

pub(crate) fn node_name(snap: &Snapshot, id: &str) -> String {
    snap.nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| id.to_string())
}
