//! `loom graph` command family — linking upstream graphs (federation).
//!
//! Plane: CLI surface over the engine's federation layer. Owns the upstream
//! registry (link/unlink/list against another graph's export, with graph-id
//! and alias uniqueness enforced at link time) and the shadow UpstreamIntent
//! nodes that stand in for upstream intents locally. It never imports upstream
//! truth wholesale — shadows reference, they do not copy verdicts or edges.
//!
//! Unlink keeps shadows by default so an accidental unlink is recoverable via
//! re-link; permanent disposal is explicit (`unlink --prune` or
//! `prune-orphans`) so doctor has a first-class remediation path.

use super::*;
use crate::federation::{
    drop_upstream_hash, prune_orphan_shadows, read_upstream_entries, write_upstream_entries,
    OrphanPruneReport, UpstreamEntry,
};

pub(crate) fn dispatch(graph: Option<&Path>, cmd: crate::cli::GraphCmd, json: bool) -> Result<()> {
    match cmd {
        crate::cli::GraphCmd::Link { path, name } => link(graph, &path, name.as_deref(), json),
        crate::cli::GraphCmd::Unlink {
            key,
            prune,
            cascade,
        } => unlink(graph, &key, prune, cascade, json),
        crate::cli::GraphCmd::PruneOrphans { alias, cascade } => {
            prune_orphans(graph, alias.as_deref(), cascade, json)
        }
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
            crate::model::short(upstream_id)
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
            crate::model::short(upstream_id),
            created
        ),
    )
}

fn unlink(graph: Option<&Path>, key: &str, prune: bool, cascade: bool, json: bool) -> Result<()> {
    let store = open(graph)?;
    let mut entries = read_upstream_entries(&store)?;
    let key = key.trim();
    if key.is_empty() {
        bail!("unlink needs a non-empty alias or graph-id — see `loom graph list`");
    }
    // An exact alias or graph-id hit is unambiguous. Only fall back to a
    // graph-id prefix, and refuse a prefix that names more than one upstream —
    // an empty or short key must never sweep several links at once.
    let removed: Vec<UpstreamEntry> = {
        let exact: Vec<UpstreamEntry> = entries
            .iter()
            .filter(|e| e.alias == key || e.graph_id == key)
            .cloned()
            .collect();
        if !exact.is_empty() {
            exact
        } else {
            let prefix: Vec<UpstreamEntry> = entries
                .iter()
                .filter(|e| e.graph_id.starts_with(key))
                .cloned()
                .collect();
            if prefix.len() > 1 {
                bail!(
                    "'{key}' matches {} upstreams by graph-id prefix — use the full graph-id or the alias",
                    prefix.len()
                );
            }
            prefix
        }
    };
    if removed.is_empty() {
        bail!("no upstream matching '{key}' — check `loom graph list`");
    }
    entries.retain(|e| !removed.iter().any(|r| r.graph_id == e.graph_id));
    write_upstream_entries(&store, &entries)?;

    // Drop local hash cache for each unlinked graph (not portable meta).
    for r in &removed {
        drop_upstream_hash(&store, &r.graph_id)?;
    }

    // Default: keep shadows orphaned so re-link can reattach; doctor flags them.
    // Permanent disposal is opt-in via --prune (same policy as prune-orphans).
    let prune_report = if prune {
        // Filter to the just-unlinked alias(es). Multi-match is rare (prefix
        // graph-id) but dispose each removed alias's shadows.
        let mut combined = OrphanPruneReport::default();
        for r in &removed {
            let part = prune_orphan_shadows(&store, Some(&r.alias), cascade)?;
            combined.pruned.extend(part.pruned);
            combined.blocked.extend(part.blocked);
            combined.cascade_edges += part.cascade_edges;
        }
        Some(combined)
    } else {
        None
    };

    let (next_step, line) = match &prune_report {
        None => (
            "loom doctor".to_string(),
            format!(
                "unlinked upstream '{key}' — shadow nodes kept (dispose with `loom graph prune-orphans` when permanently gone; `loom doctor` flags orphans)"
            ),
        ),
        Some(report) if !report.blocked.is_empty() && report.pruned.is_empty() => (
            "loom edge remove <edge-id> --reason '…'".to_string(),
            format!(
                "unlinked upstream '{key}' — 0 shadow(s) pruned, {} blocked by DependsOn (remove those edges or re-run with --prune --cascade)",
                report.blocked.len()
            ),
        ),
        Some(report) if !report.blocked.is_empty() => (
            "loom graph prune-orphans --cascade".to_string(),
            format!(
                "unlinked upstream '{key}' — pruned {} shadow(s) ({} DependsOn cascaded); {} still blocked by DependsOn",
                report.pruned.len(),
                report.cascade_edges,
                report.blocked.len()
            ),
        ),
        Some(report) => (
            "loom doctor".to_string(),
            format!(
                "unlinked upstream '{key}' — pruned {} shadow node(s){}",
                report.pruned.len(),
                if report.cascade_edges > 0 {
                    format!(" ({} DependsOn cascaded)", report.cascade_edges)
                } else {
                    String::new()
                }
            ),
        ),
    };

    let blocked_prune = prune_report
        .as_ref()
        .is_some_and(|r| r.pruned.is_empty() && !r.blocked.is_empty());

    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "unlinked": true,
            // The unlink succeeded; `ok` reports the command as a whole, which a
            // fully-blocked prune fails. This keeps the payload honest so a
            // --json consumer need not scrape stderr to learn the exit code.
            "ok": !blocked_prune,
            "key": key,
            "remaining": entries.len(),
            "pruned": prune_report.as_ref().map(prune_json),
        }),
        &next_step,
        line,
    )?;

    // Partial/blocked prune after unlink is still success for the unlink itself,
    // but surface non-zero when the operator asked to prune and nothing moved
    // because of DependsOn claims — same contract as standalone prune-orphans.
    if blocked_prune {
        bail!(
            "unlinked but could not prune: {} orphan shadow(s) still have DependsOn edges — remove them (`loom edge remove`) or pass --cascade",
            prune_report.as_ref().map(|r| r.blocked.len()).unwrap_or(0)
        );
    }
    Ok(())
}

fn prune_orphans(
    graph: Option<&Path>,
    alias: Option<&str>,
    cascade: bool,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let report = prune_orphan_shadows(&store, alias, cascade)?;

    if report.pruned.is_empty() && report.blocked.is_empty() {
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "pruned": prune_json(&report),
            }),
            "loom doctor",
            "no orphan UpstreamIntent shadows to prune",
        )?;
        return Ok(());
    }

    if report.pruned.is_empty() && !report.blocked.is_empty() {
        let detail = blocked_summary(&report);
        let reason = format!(
            "could not prune: {} orphan shadow(s) still have DependsOn edges — remove them (`loom edge remove`) or pass --cascade",
            report.blocked.len()
        );
        // Fail closed (non-zero exit), but put the blocked state in the payload
        // so a `--json` consumer reads the reason from stdout instead of an
        // unparseable stderr line paired with a success-shaped object.
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "ok": false,
                "error": reason,
                "pruned": prune_json(&report),
            }),
            "loom edge remove <edge-id> --reason '…'",
            format!(
                "0 orphan shadow(s) pruned; {} blocked by DependsOn — remove those edges or re-run with --cascade\n{detail}",
                report.blocked.len()
            ),
        )?;
        bail!("{reason}");
    }

    let line = if report.blocked.is_empty() {
        format!(
            "pruned {} orphan UpstreamIntent shadow(s){}",
            report.pruned.len(),
            if report.cascade_edges > 0 {
                format!(" ({} DependsOn cascaded)", report.cascade_edges)
            } else {
                String::new()
            }
        )
    } else {
        format!(
            "pruned {} orphan shadow(s) ({} DependsOn cascaded); {} still blocked by DependsOn — re-run with --cascade or edge remove",
            report.pruned.len(),
            report.cascade_edges,
            report.blocked.len()
        )
    };

    let next = if report.blocked.is_empty() {
        "loom doctor"
    } else {
        "loom graph prune-orphans --cascade"
    };

    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "pruned": prune_json(&report),
        }),
        next,
        line,
    )
}

fn prune_json(report: &OrphanPruneReport) -> serde_json::Value {
    serde_json::json!({
        "count": report.pruned.len(),
        "cascade_edges": report.cascade_edges,
        "shadows": report.pruned.iter().map(|o| serde_json::json!({
            "id": o.id,
            "name": o.name,
            "alias": o.alias,
        })).collect::<Vec<_>>(),
        "blocked": report.blocked.iter().map(|o| serde_json::json!({
            "id": o.id,
            "name": o.name,
            "alias": o.alias,
            "depends_on_edge_ids": o.depends_on_edge_ids,
            "dependent_intents": o.dependent_intents,
        })).collect::<Vec<_>>(),
    })
}

fn blocked_summary(report: &OrphanPruneReport) -> String {
    report
        .blocked
        .iter()
        .take(8)
        .map(|o| {
            format!(
                "  {} ← depends_on from {} (edge {})",
                o.name,
                o.dependent_intents.join(", "),
                o.depends_on_edge_ids
                    .first()
                    .map(|id| crate::model::short(id))
                    .unwrap_or("?")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let entries = read_upstream_entries(&store)?;
    let orphans = crate::federation::list_orphan_shadows(&store, None)?;
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
        // Additive envelope when orphans exist so agents see the cleanup path
        // without a separate doctor pass; a clean graph stays a bare array so
        // there is no false orphan envelope (tests pin both shapes).
        if orphans.is_empty() {
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "linked": rows,
                    "orphan_shadows": orphans.len(),
                    "orphan_aliases": orphans.iter().map(|o| &o.alias).collect::<std::collections::BTreeSet<_>>(),
                    "hint": "loom graph prune-orphans",
                }))?
            );
        }
    } else if entries.is_empty() {
        if orphans.is_empty() {
            println!("no upstream graphs linked");
        } else {
            println!(
                "no upstream graphs linked ({} orphan UpstreamIntent shadow(s) — run `loom graph prune-orphans`)",
                orphans.len()
            );
        }
    } else {
        for e in &entries {
            println!(
                "{} [{}]  {}",
                e.alias,
                crate::model::short(&e.graph_id),
                e.path
            );
        }
        if !orphans.is_empty() {
            println!(
                "({} orphan UpstreamIntent shadow(s) from prior unlinks — `loom graph prune-orphans`)",
                orphans.len()
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
        crate::federation::add_shadow_node(store, &export.graph_id, alias, node)?;
        created += 1;
    }
    Ok(created)
}
