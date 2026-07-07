//! `loom graph` command family — linking upstream graphs (federation).
//!
//! Plane: CLI surface over the engine's federation layer. Owns the upstream
//! registry (link/unlink/list against another graph's export, with graph-id
//! and alias uniqueness enforced at link time) and the shadow UpstreamIntent
//! nodes that stand in for upstream intents locally. It never imports upstream
//! truth wholesale — shadows reference, they do not copy verdicts or edges.

use super::*;
use crate::federation::{read_upstream_entries, write_upstream_entries, UpstreamEntry};

pub(crate) fn dispatch(graph: Option<&Path>, cmd: crate::cli::GraphCmd, json: bool) -> Result<()> {
    match cmd {
        crate::cli::GraphCmd::Link { path, name } => link(graph, &path, name.as_deref(), json),
        crate::cli::GraphCmd::Unlink { key } => unlink(graph, &key, json),
        crate::cli::GraphCmd::List => list(graph, json),
    }
}

fn link(graph: Option<&Path>, export_path: &Path, alias: Option<&str>, json: bool) -> Result<()> {
    let root = resolve_root(graph)?;
    let store = Store::open(&root)?;

    // Read and parse the upstream export.
    let export = travel::read_export(export_path)?;
    let upstream_id = &export.graph_id;
    let upstream_name = &export.name;
    let alias = alias.unwrap_or(upstream_name).to_string();

    // Reject linking to ourselves.
    let local_id = store.identity()?;
    if local_id.graph_id == *upstream_id {
        bail!("cannot link a graph to itself");
    }

    // Check for duplicate links (same graph_id).
    let mut entries = read_upstream_entries(&store)?;
    if entries.iter().any(|e| e.graph_id == *upstream_id) {
        bail!(
            "upstream '{}' ({}) is already linked — unlink first to re-register",
            alias,
            &upstream_id[..8.min(upstream_id.len())]
        );
    }

    // Reject duplicate aliases (shadow names and unlink keys depend on uniqueness).
    if entries.iter().any(|e| e.alias == alias) {
        bail!(
            "alias '{}' is already in use — pick a different --name or unlink the existing one",
            alias
        );
    }

    // Make the path relative to the graph root if possible, for portability.
    let stored_path = match export_path.strip_prefix(&root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => export_path
            .canonicalize()
            .unwrap_or_else(|_| export_path.to_path_buf())
            .to_string_lossy()
            .into_owned(),
    };

    entries.push(UpstreamEntry {
        path: stored_path,
        alias: alias.clone(),
        graph_id: upstream_id.clone(),
    });
    write_upstream_entries(&store, &entries)?;

    // Create shadow UpstreamIntent nodes for each intent in the export.
    let created = create_shadow_nodes(&store, &export, &alias)?;

    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "linked": true,
            "alias": alias,
            "graph_id": upstream_id,
            "upstream_name": upstream_name,
            "shadow_nodes": created,
        }),
        "loom sync",
        format!(
            "linked upstream '{}' ({}) — {} shadow node(s) created",
            alias,
            &upstream_id[..8.min(upstream_id.len())],
            created
        ),
    )
}

fn unlink(graph: Option<&Path>, key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let mut entries = read_upstream_entries(&store)?;
    let before = entries.len();
    entries.retain(|e| e.alias != key && e.graph_id != key && !e.graph_id.starts_with(key));
    if entries.len() == before {
        bail!("no upstream matching '{key}' — check `loom graph list`");
    }
    write_upstream_entries(&store, &entries)?;

    // Shadow nodes are intentionally NOT deleted (orphan + doctor warning).
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "unlinked": true,
            "key": key,
            "remaining": entries.len(),
        }),
        "loom doctor",
        format!(
            "unlinked upstream '{key}' — shadow nodes kept (run `loom doctor` to check for orphans)"
        ),
    )
}

fn list(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let entries = read_upstream_entries(&store)?;
    if json {
        let rows: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "alias": e.alias,
                    "graph_id": e.graph_id,
                    "path": e.path,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if entries.is_empty() {
        println!("no upstream graphs linked");
    } else {
        for e in &entries {
            println!(
                "{} [{}]  {}",
                e.alias,
                &e.graph_id[..8.min(e.graph_id.len())],
                e.path
            );
        }
    }
    Ok(())
}

/// Create UpstreamIntent shadow nodes for each intent in the export.
/// Returns the count of nodes created (skips duplicates by name).
fn create_shadow_nodes(store: &Store, export: &travel::Export, alias: &str) -> Result<usize> {
    let existing: std::collections::HashSet<String> = store
        .list_nodes(Some(NodeType::UpstreamIntent), usize::MAX)?
        .into_iter()
        .map(|n| n.name)
        .collect();
    let mut created = 0usize;
    for node in &export.nodes {
        if node.node_type != NodeType::Intent {
            continue;
        }
        let shadow_name = format!("upstream/{}/{}", alias, node.name);
        if existing.contains(&shadow_name) {
            continue;
        }
        let body = serde_json::json!({
            "graph_id": export.graph_id,
            "node_id": node.id,
            "alias": alias,
        });
        store.add_node(
            NodeType::UpstreamIntent,
            &shadow_name,
            &node.description,
            &node.status,
            body,
        )?;
        created += 1;
    }
    Ok(created)
}
