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
use anyhow::Context;
use std::collections::HashMap;
use std::path::Path;

// ---- upstream registry (shared with commands::graph_cmd) -------------------

/// Upstream graph registration stored in the `upstream_graphs` meta key.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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
    let Some(raw) = store.get_meta("upstream_graphs")? else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&raw).context("parsing upstream_graphs registry")
}

/// Write the linked-upstream registry to meta.
pub fn write_upstream_entries(store: &Store, entries: &[UpstreamEntry]) -> Result<()> {
    store.set_meta("upstream_graphs", &serde_json::to_string(entries)?)?;
    Ok(())
}

/// Drop cached export hashes for graph ids that are no longer registered.
/// Local-only meta (`upstream_hashes` is not portable); leaving stale keys after
/// unlink is harmless for sync but confuses operators reading the store.
pub fn drop_upstream_hash(store: &Store, graph_id: &str) -> Result<()> {
    let Some(raw) = store.get_meta("upstream_hashes")? else {
        return Ok(());
    };
    let mut hashes: HashMap<String, String> =
        serde_json::from_str(&raw).context("parsing upstream_hashes cache")?;
    if hashes.remove(graph_id).is_some() {
        store.set_meta("upstream_hashes", &serde_json::to_string(&hashes)?)?;
    }
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
    let cached_hashes: HashMap<String, String> = match store.get_meta("upstream_hashes")? {
        Some(raw) => serde_json::from_str(&raw).context("parsing upstream_hashes cache")?,
        None => HashMap::new(),
    };

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
        let content = std::fs::read_to_string(&export_path)
            .with_context(|| format!("reading upstream export '{}'", export_path.display()))?;
        let hash = content_hash(&content);

        let hash_matches = cached_hashes.get(&entry.graph_id).map(|h| h.as_str()) == Some(&hash);
        if hash_matches && shadows_have_facets(store, &entry.alias)? {
            report.upstreams_unchanged += 1;
            continue;
        }

        // Parse the export.
        let export = crate::travel::Export::from_json(&content)
            .with_context(|| format!("parsing upstream export '{}'", export_path.display()))?;

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

                stale_dependents(
                    store,
                    &shadow.id,
                    &format!("upstream/{alias}/{} changed", node.name),
                    report,
                )?;
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
        stale_dependents(
            store,
            &shadow.id,
            &format!("upstream/{alias}/{} removed from export", shadow.name),
            report,
        )?;
    }

    Ok(())
}

fn stale_dependents(
    store: &Store,
    shadow_id: &str,
    cause: &str,
    report: &mut FederationReport,
) -> Result<()> {
    for edge in store.edges_with(Some(EdgeKind::DependsOn), None, Some(shadow_id))? {
        if store.stale_edge(&edge.id, cause)? {
            report.edges_staled += 1;
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

// ---- orphan shadow disposal (after intentional permanent unlink) -----------

/// One UpstreamIntent shadow whose alias is no longer in the registry.
#[derive(Debug, Clone)]
pub struct OrphanShadow {
    pub id: String,
    pub name: String,
    pub alias: String,
    /// Local intents that still assert `DependsOn` → this shadow.
    pub dependent_intents: Vec<String>,
    /// The DependsOn edge ids that would cascade if pruned with cascade=true.
    pub depends_on_edge_ids: Vec<String>,
}

/// Result of disposing orphan UpstreamIntent shadows.
#[derive(Debug, Default, Clone)]
pub struct OrphanPruneReport {
    /// Shadows hard-deleted.
    pub pruned: Vec<OrphanShadow>,
    /// DependsOn edges removed as part of cascade (0 without cascade).
    pub cascade_edges: usize,
    /// Orphans still held by DependsOn claims (only when cascade=false).
    pub blocked: Vec<OrphanShadow>,
}

/// List UpstreamIntent shadows whose `body.alias` is not in the upstream registry.
///
/// Optional `alias_filter` restricts to one former alias (e.g. just-unlinked).
pub fn list_orphan_shadows(store: &Store, alias_filter: Option<&str>) -> Result<Vec<OrphanShadow>> {
    let entries = read_upstream_entries(store)?;
    let linked: std::collections::BTreeSet<&str> =
        entries.iter().map(|e| e.alias.as_str()).collect();

    let mut orphans = Vec::new();
    for n in store.list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)? {
        let alias = n
            .body
            .get("alias")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if linked.contains(alias.as_str()) {
            continue;
        }
        if let Some(want) = alias_filter {
            if alias != want {
                continue;
            }
        }
        let edges = store.edges_with(Some(EdgeKind::DependsOn), None, Some(&n.id))?;
        let mut dependent_intents = Vec::new();
        let mut depends_on_edge_ids = Vec::new();
        for e in edges {
            depends_on_edge_ids.push(e.id.clone());
            if let Some(from) = store.get_node(&e.from_id)? {
                dependent_intents.push(from.name);
            } else {
                dependent_intents.push(e.from_id);
            }
        }
        orphans.push(OrphanShadow {
            id: n.id,
            name: n.name,
            alias,
            dependent_intents,
            depends_on_edge_ids,
        });
    }
    orphans.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(orphans)
}

/// Hard-delete orphan UpstreamIntent shadows after an intentional permanent unlink.
///
/// Default (`cascade = false`): deletes only orphans with no remaining
/// `DependsOn` edges; orphans still claimed by local intents are listed in
/// `blocked` and left in place (operator must `edge remove` or re-run with
/// `cascade = true`).
///
/// With `cascade = true`: deletes every matching orphan; `Store::delete_node`
/// cascades incident edges (including DependsOn) so no dangling claims remain.
///
/// `alias_filter` limits disposal to one former alias (used by `unlink --prune`).
pub fn prune_orphan_shadows(
    store: &Store,
    alias_filter: Option<&str>,
    cascade: bool,
) -> Result<OrphanPruneReport> {
    let orphans = list_orphan_shadows(store, alias_filter)?;
    let mut report = OrphanPruneReport::default();

    for orphan in orphans {
        let has_deps = !orphan.depends_on_edge_ids.is_empty();
        if has_deps && !cascade {
            report.blocked.push(orphan);
            continue;
        }
        let edge_count = orphan.depends_on_edge_ids.len();
        store.delete_node(&orphan.id)?;
        report.cascade_edges += edge_count;
        report.pruned.push(orphan);
    }
    Ok(report)
}
