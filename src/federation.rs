//! Federation — cross-graph staleness propagation.
//!
//! Plane: orchestration over upstream exports. A pre-sync pass reads each
//! linked upstream `loom.graph.json`, diffs it against cached derived facets
//! on local UpstreamIntent shadow nodes, and ripples staleness to any
//! `DependsOn` edges whose upstream target changed.
//!
//! Truth-class discipline (CodeFile-parallel):
//! - The `UpstreamIntent` node itself is **asserted** (created by `graph link`,
//!   body carries provenance `{graph_id, node_id, alias}`). Never machine-rewritten.
//! - Live upstream state (`upstream_description`, `upstream_status`,
//!   `upstream_lifecycle`, `upstream_content_hash`) lives as **derived facets**,
//!   rebuilt every sync from the upstream export. `wipe_derived + sync` converges.

use crate::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use std::collections::HashMap;
use std::path::Path;

// ---- upstream registry (shared with commands::graph_cmd) -------------------

/// Upstream graph registration stored in the `upstream_graphs` meta key.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UpstreamEntry {
    /// Filesystem path to the upstream `loom.graph.json`.
    pub path: String,
    /// Human alias (default: the upstream graph's name).
    pub alias: String,
    /// Stable graph-id read from the upstream export at link time.
    pub graph_id: String,
}

/// Read the linked-upstream registry from meta.
pub fn read_upstream_entries(store: &Store) -> Result<Vec<UpstreamEntry>> {
    Ok(store
        .get_meta("upstream_graphs")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

/// Write the linked-upstream registry to meta.
pub fn write_upstream_entries(store: &Store, entries: &[UpstreamEntry]) -> Result<()> {
    store.set_meta("upstream_graphs", &serde_json::to_string(entries)?)?;
    Ok(())
}

/// Summary of one federation pass.
#[derive(Debug, Default, Clone)]
pub struct FederationReport {
    /// Number of upstream graphs checked.
    pub upstreams_checked: usize,
    /// Shadow nodes created for newly-appeared upstream intents.
    pub shadows_created: usize,
    /// Shadow nodes whose upstream content changed (facets updated).
    pub shadows_updated: usize,
    /// DependsOn edges staled because their upstream target changed.
    pub edges_staled: usize,
    /// Upstream graphs that were skipped (unchanged hash).
    pub upstreams_unchanged: usize,
}

/// Run the federation pass for all linked upstreams.
///
/// For each linked upstream:
/// 1. Read the export file and compute a content hash.
/// 2. If the hash matches the cached `upstream_export_hash` meta, skip.
/// 3. Otherwise: parse, diff against shadow nodes, create new shadows,
///    update derived facets on changed ones, and stale DependsOn edges.
pub fn run(store: &Store, root: &Path) -> Result<FederationReport> {
    let mut report = FederationReport::default();

    let entries = read_upstream_entries(store)?;
    if entries.is_empty() {
        return Ok(report);
    }

    // Read cached per-upstream hashes.
    let cached_hashes: HashMap<String, String> = store
        .get_meta("upstream_hashes")?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut new_hashes = cached_hashes.clone();

    for entry in &entries {
        report.upstreams_checked += 1;

        // Resolve the export path relative to the graph root.
        let export_path = if Path::new(&entry.path).is_absolute() {
            std::path::PathBuf::from(&entry.path)
        } else {
            root.join(&entry.path)
        };

        // Read the raw file content for hashing; skip if unchanged.
        let content = match std::fs::read_to_string(&export_path) {
            Ok(c) => c,
            Err(_) => continue, // Missing export file — skip silently.
        };
        let hash = content_hash(&content);

        let hash_matches = cached_hashes.get(&entry.graph_id).map(|h| h.as_str()) == Some(&hash);
        if hash_matches && shadows_have_facets(store, &entry.alias)? {
            report.upstreams_unchanged += 1;
            continue;
        }

        // Parse the export.
        let export = match crate::travel::Export::from_json(&content) {
            Ok(e) => e,
            Err(_) => continue, // Malformed export — skip silently.
        };

        // Reconcile shadow nodes against the fresh export.
        reconcile_shadows(store, &export, &entry.alias, &mut report)?;

        new_hashes.insert(entry.graph_id.clone(), hash);
    }

    // Persist updated hashes.
    if new_hashes != cached_hashes {
        store.set_meta("upstream_hashes", &serde_json::to_string(&new_hashes)?)?;
    }

    Ok(report)
}

/// Reconcile local UpstreamIntent shadows against a fresh upstream export.
fn reconcile_shadows(
    store: &Store,
    export: &crate::travel::Export,
    alias: &str,
    report: &mut FederationReport,
) -> Result<()> {
    // Index existing shadows for this alias by their upstream node_id.
    let all_shadows = store.list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)?;
    let mut shadow_by_upstream_id: HashMap<String, crate::model::Node> = HashMap::new();
    for s in all_shadows {
        if s.body.get("alias").and_then(|v| v.as_str()) != Some(alias) {
            continue;
        }
        if let Some(uid) = s.body.get("node_id").and_then(|v| v.as_str()) {
            shadow_by_upstream_id.insert(uid.to_string(), s);
        }
    }

    // Build a set of upstream intent ids present in the fresh export.
    let mut upstream_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for node in &export.nodes {
        if node.node_type == NodeType::Intent {
            upstream_ids.insert(node.id.clone());
        }
    }

    // Walk upstream intents — update or create shadows.
    for node in &export.nodes {
        if node.node_type != NodeType::Intent {
            continue;
        }

        let upstream_hash = intent_hash(node);

        match shadow_by_upstream_id.get(&node.id) {
            Some(shadow) => {
                // Clear upstream_missing if it was previously set — this
                // upstream intent is present again. Must happen before the
                // hash-equality early return so a reappearing intent with
                // unchanged content still gets cleared.
                if store
                    .get_facet(&shadow.id, TargetKind::Node, "upstream_missing")?
                    .is_some()
                {
                    store.set_facet(
                        &shadow.id,
                        TargetKind::Node,
                        "upstream_missing",
                        "false",
                        TruthClass::Derived,
                    )?;
                }

                // Existing shadow — check if upstream changed.
                let cached =
                    store.get_facet(&shadow.id, TargetKind::Node, "upstream_content_hash")?;

                if cached.as_deref() == Some(&upstream_hash) {
                    continue; // No change.
                }

                // Upstream changed — update derived facets.
                write_upstream_facets(store, &shadow.id, node, &upstream_hash)?;
                report.shadows_updated += 1;

                // Stale all DependsOn edges pointing at this shadow.
                let edges = store.edges_with(Some(EdgeKind::DependsOn), None, Some(&shadow.id))?;
                for e in &edges {
                    if store
                        .stale_edge(&e.id, &format!("upstream/{alias}/{} changed", node.name))?
                    {
                        report.edges_staled += 1;
                    }
                }
            }
            None => {
                // New upstream intent — create shadow.
                let shadow_name = format!("upstream/{}/{}", alias, node.name);
                let body = serde_json::json!({
                    "graph_id": export.graph_id,
                    "node_id": node.id,
                    "alias": alias,
                });
                let created = store.add_node(
                    NodeType::UpstreamIntent,
                    &shadow_name,
                    &node.description,
                    &node.status,
                    body,
                )?;
                write_upstream_facets(store, &created.id, node, &upstream_hash)?;
                report.shadows_created += 1;
            }
        }
    }

    // Handle deleted upstream intents: shadows whose node_id is absent from
    // the fresh export. Mark them missing and stale their DependsOn edges.
    for (uid, shadow) in &shadow_by_upstream_id {
        if upstream_ids.contains(uid) {
            continue;
        }
        // Already marked missing — skip.
        if store
            .get_facet(&shadow.id, TargetKind::Node, "upstream_missing")?
            .as_deref()
            == Some("true")
        {
            continue;
        }
        store.set_facet(
            &shadow.id,
            TargetKind::Node,
            "upstream_missing",
            "true",
            TruthClass::Derived,
        )?;
        let edges = store.edges_with(Some(EdgeKind::DependsOn), None, Some(&shadow.id))?;
        for e in &edges {
            if store.stale_edge(
                &e.id,
                &format!("upstream/{alias}/{} removed from export", shadow.name),
            )? {
                report.edges_staled += 1;
            }
        }
    }

    Ok(())
}

/// Write the derived facets that mirror an upstream intent's live state.
fn write_upstream_facets(
    store: &Store,
    shadow_id: &str,
    upstream: &crate::model::Node,
    hash: &str,
) -> Result<()> {
    store.set_facet(
        shadow_id,
        TargetKind::Node,
        "upstream_description",
        &upstream.description,
        TruthClass::Derived,
    )?;
    store.set_facet(
        shadow_id,
        TargetKind::Node,
        "upstream_status",
        &upstream.status,
        TruthClass::Derived,
    )?;
    store.set_facet(
        shadow_id,
        TargetKind::Node,
        "upstream_content_hash",
        hash,
        TruthClass::Derived,
    )?;
    Ok(())
}

/// Content hash of an upstream intent for change detection.
/// Uses the project's deterministic FNV-1a fingerprint so the hash is stable
/// across Rust versions and platforms (unlike `DefaultHasher`).
fn intent_hash(node: &crate::model::Node) -> String {
    let combined = format!("{}\0{}\0{}", node.description, node.status, node.body);
    crate::artifact::fingerprint(&combined)
}

/// File content hash (FNV-1a 64-bit, matching the store's convention).
fn content_hash(content: &str) -> String {
    crate::artifact::fingerprint(content)
}

/// Check that at least one shadow for this alias still carries its derived
/// `upstream_content_hash` facet.  After `wipe_derived` the facets are gone
/// but the `upstream_hashes` meta persists (meta is not derived), so a bare
/// hash-match would skip reconciliation and leave shadows facet-less —
/// breaking INV-2 (`wipe + sync` must converge to the same derived plane).
fn shadows_have_facets(store: &Store, alias: &str) -> Result<bool> {
    for s in store.list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)? {
        if s.body.get("alias").and_then(|v| v.as_str()) != Some(alias) {
            continue;
        }
        // Found a shadow for this alias — check its facet.
        return Ok(store
            .get_facet(&s.id, TargetKind::Node, "upstream_content_hash")?
            .is_some());
    }
    // No shadows at all for this alias — need reconciliation.
    Ok(false)
}
