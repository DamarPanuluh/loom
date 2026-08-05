//! Journey invariant-point commands.
//!
//! Plane: CLI surface over the judgment plane — asserted invariant points and
//! `Asserts` links; whether a point is verified is derived, never stored.

use super::{open, pulse};
use crate::cli::JourneyInvariantCmd;
use crate::model::{EdgeKind, Node, NodeType};
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};

// ---- invariant points ------------------------------------------------------

pub(super) fn invariant(
    graph: Option<&std::path::Path>,
    cmd: JourneyInvariantCmd,
    json: bool,
) -> Result<()> {
    match cmd {
        JourneyInvariantCmd::Add {
            name,
            intent,
            field,
            assertion,
            reason,
        } => invariant_add(graph, &name, &intent, &field, &assertion, &reason, json),
        JourneyInvariantCmd::Update {
            key,
            field,
            assertion,
            asserts,
            reason_text,
            reason,
        } => invariant_update(
            graph,
            &key,
            InvariantUpdate {
                field: field.as_deref(),
                assertion: assertion.as_deref(),
                asserts: asserts.as_deref(),
                reason_text: reason_text.as_deref(),
                reason: &reason,
            },
            json,
        ),
        JourneyInvariantCmd::Remove { key } => invariant_remove(graph, &key, json),
        JourneyInvariantCmd::List { limit, offset } => invariant_list(graph, limit, offset, json),
    }
}

fn invariant_add(
    graph: Option<&std::path::Path>,
    name: &str,
    intent_key: &str,
    field: &str,
    assertion: &str,
    reason: &str,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
    let body = json!({
        "field": field,
        "assertion": assertion,
        "reason": reason,
    });
    let node = store.add_node(
        NodeType::JourneyInvariantPoint,
        name,
        "",
        "unverified",
        body,
    )?;
    store.add_edge(
        EdgeKind::Asserts,
        &node.id,
        &intent.id,
        crate::model::TruthClass::Asserted,
    )?;
    let payload = json!({
        "id": node.id,
        "name": node.name,
        "field": field,
        "assertion": assertion,
        "asserts": intent.name,
        "asserts_id": intent.id,
    });
    let line = format!(
        "added journey invariant '{}' → asserts '{}' [{}]",
        node.name,
        intent.name,
        crate::model::short(&node.id)
    );
    pulse::emit_line(
        &store,
        json,
        payload,
        "run `loom journey prompt <intent>` to include this invariant in the proof design",
        line,
    )
}

fn invariant_list(
    graph: Option<&std::path::Path>,
    limit: usize,
    offset: usize,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let nodes = store.list_nodes_page(Some(NodeType::JourneyInvariantPoint), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::JourneyInvariantPoint))?;
    let mut rows: Vec<Value> = Vec::new();
    for n in &nodes {
        let asserts = store
            .edges_with(Some(EdgeKind::Asserts), Some(&n.id), None)?
            .into_iter()
            .next();
        let asserts_name = asserts
            .as_ref()
            .and_then(|e| store.get_node(&e.to_id).ok().flatten())
            .map(|i| i.name);
        if json {
            rows.push(json!({
                "id": n.id,
                "name": n.name,
                "field": n.body.get("field"),
                "assertion": n.body.get("assertion"),
                "asserts": asserts_name,
            }));
        } else {
            println!(
                "{}  {}  field={}  asserts={}",
                crate::model::short(&n.id),
                n.name,
                n.body.get("field").and_then(|v| v.as_str()).unwrap_or(""),
                asserts_name.as_deref().unwrap_or("—"),
            );
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::commands::pagination_envelope(
                &rows, offset, limit, total
            ))?
        );
    } else if let Some(footer) = crate::commands::page_footer(nodes.len(), offset, total) {
        println!("{footer}");
    }
    Ok(())
}
/// The optional payload of `journey invariant update` — grouped so the flag
/// list can grow without widening the helper's signature.
struct InvariantUpdate<'a> {
    field: Option<&'a str>,
    assertion: Option<&'a str>,
    asserts: Option<&'a str>,
    reason_text: Option<&'a str>,
    reason: &'a str,
}

fn invariant_update(
    graph: Option<&std::path::Path>,
    key: &str,
    update: InvariantUpdate<'_>,
    json: bool,
) -> Result<()> {
    let InvariantUpdate {
        field,
        assertion,
        asserts,
        reason_text,
        reason,
    } = update;
    if reason.trim().is_empty() {
        bail!("journey invariant update needs substantive --reason");
    }
    if field.is_none() && assertion.is_none() && asserts.is_none() && reason_text.is_none() {
        bail!("nothing to update — pass --field, --assertion, --asserts, and/or --reason-text");
    }
    let store = open(graph)?;
    let node = store.resolve_node(key, Some(NodeType::JourneyInvariantPoint))?;
    // Resolve the re-point target BEFORE any write so a bad --asserts key
    // cannot leave a half-applied update (body changed, edge untouched).
    let asserts_target: Option<Node> = asserts
        .map(|k| store.resolve_node(k, Some(NodeType::Intent)))
        .transpose()?;
    let mut body = node.body.clone();
    if let Some(v) = field {
        body["field"] = json!(v);
    }
    if let Some(v) = assertion {
        body["assertion"] = json!(v);
    }
    if let Some(v) = reason_text {
        body["reason"] = json!(v);
    }
    store.set_node_body(&node.id, &body)?;
    // Re-pointing lives on the asserts EDGE, not in the body: replace the edge
    // in place so the invariant node — and its note trail — keeps continuity.
    let mut moved: Option<(Vec<String>, Node)> = None;
    if let Some(intent) = asserts_target {
        let existing = store.edges_with(Some(EdgeKind::Asserts), Some(&node.id), None)?;
        let already = existing.iter().any(|e| e.to_id == intent.id);
        let mut old_names = Vec::new();
        for e in existing.iter().filter(|e| e.to_id != intent.id) {
            let old = store
                .get_node(&e.to_id)?
                .map(|n| n.name)
                .unwrap_or_else(|| e.to_id.clone());
            store.delete_edge(&e.id)?;
            old_names.push(old);
        }
        if !already {
            store.add_edge(
                EdgeKind::Asserts,
                &node.id,
                &intent.id,
                crate::model::TruthClass::Asserted,
            )?;
        }
        moved = Some((old_names, intent));
    }
    let note_text = match &moved {
        Some((old_names, intent)) if !old_names.is_empty() => format!(
            "re-pointed journey invariant: asserts '{}' (was '{}'): {reason}",
            intent.name,
            old_names.join("', '"),
        ),
        _ => format!("updated journey invariant point: {reason}"),
    };
    store.add_note(&node.id, "decision", &note_text)?;
    let mut payload = json!({
        "invariant": {
            "id": node.id,
            "name": node.name,
            "status": node.status,
            "body": body,
        },
        "reason": reason,
    });
    if let Some((old_names, intent)) = &moved {
        payload["invariant"]["asserts"] = json!(intent.name);
        payload["invariant"]["asserts_id"] = json!(intent.id);
        payload["moved_from"] = json!(old_names);
    }
    let line = match &moved {
        Some((_, intent)) => format!(
            "updated journey invariant '{}' → asserts '{}'",
            node.name, intent.name
        ),
        None => format!("updated journey invariant '{}'", node.name),
    };
    pulse::emit_line(&store, json, payload, "loom journey invariant list", line)
}

fn invariant_remove(graph: Option<&std::path::Path>, key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let node = store.resolve_node(key, Some(NodeType::JourneyInvariantPoint))?;
    store.delete_node(&node.id)?;
    pulse::emit_line(
        &store,
        json,
        json!({
            "removed": true,
            "invariant": {
                "id": node.id,
                "name": node.name,
                "status": node.status,
                "body": node.body,
            },
        }),
        "loom journey invariant list",
        format!("removed journey invariant '{}'", node.name),
    )
}
