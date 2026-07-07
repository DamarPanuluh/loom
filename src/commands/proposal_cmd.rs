//! `loom proposal` command family.
//!
//! A Proposal is one asserted node (NodeType::Proposal) whose `body` holds the
//! raw captured text, its source, and a numbered list of items. Items live
//! entirely inside `body.items` as JSON objects — they are NOT graph nodes.
//! Adoption marks an item `adopted` and optionally spawns an Intent or
//! TaskRecord node, recording the source proposal/item id in the spawned
//! node's body for traceability.

use super::{looks_like_symbol, node_json, open, pulse};
use crate::cli::{ProposalCmd, ProposalItemCmd};
use crate::model::{NodeType, TargetKind, TruthClass};
use crate::Result;
use anyhow::{anyhow, bail};
use serde_json::{json, Value};
use std::path::Path;

/// Dispatch entry point for the `loom proposal` family.
pub fn dispatch(graph: Option<&Path>, cmd: ProposalCmd, json: bool) -> Result<()> {
    match cmd {
        ProposalCmd::Add { title, file, text } => add(graph, title, file, text, json),
        ProposalCmd::List { limit, offset } => list(graph, limit, offset, json),
        ProposalCmd::Show { key } => show(graph, key, json),
        ProposalCmd::Remove { key } => remove(graph, key, json),
        ProposalCmd::Item { cmd } => item(graph, cmd, json),
    }
}

// ---------------------------------------------------------------------------
// proposal add / list / show
// ---------------------------------------------------------------------------

fn add(
    graph: Option<&Path>,
    title: String,
    file: Option<std::path::PathBuf>,
    text: Option<String>,
    json: bool,
) -> Result<()> {
    // Exactly one of --file / --text is required.
    let raw = match (&file, &text) {
        (Some(_), Some(_)) => bail!("pass exactly one of --file or --text, not both"),
        (None, None) => bail!("pass exactly one of --file or --text"),
        (Some(path), None) => std::fs::read_to_string(path)
            .map_err(|e| anyhow!("failed to read --file {}: {e}", path.display()))?,
        (None, Some(t)) => t.clone(),
    };
    let source = if file.is_some() { "file" } else { "text" };
    let source_path = file.as_ref().map(|path| path.display().to_string());

    let store = open(graph)?;
    let body = json!({
        "raw": raw,
        "source": source,
        "source_path": source_path,
        "items": [],
    });
    // `name` is the title; `description` is a short summary (first line / truncated raw).
    let description = summary(&raw);
    let node = store.add_node(NodeType::Proposal, &title, &description, "captured", body)?;

    if json {
        // Full proposal JSON including body shape per contract.
        let out = json!({
            "id": node.id,
            "name": node.name,
            "status": node.status,
            "description": node.description,
            "body": node.body,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("added proposal '{}' [{}]", node.name, &node.id[..8]);
    }
    Ok(())
}

fn list(graph: Option<&Path>, limit: usize, offset: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let proposals = store.list_nodes_page(Some(NodeType::Proposal), limit, offset)?;
    if json {
        let rows: Vec<Value> = proposals
            .iter()
            .map(|n| {
                json!({
                    "id": n.id,
                    "name": n.name,
                    "status": n.status,
                    "description": n.description,
                    "body": n.body,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if proposals.is_empty() && offset == 0 {
            println!("no proposals");
        }
        for n in &proposals {
            let count = n
                .body
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            println!(
                "{:<10} {} [{}] ({} items)",
                n.status,
                n.name,
                &n.id[..8],
                count
            );
        }
        if let Some(footer) = super::page_footer(
            proposals.len(),
            offset,
            store.count_nodes(Some(NodeType::Proposal))?,
        ) {
            println!("{footer}");
        }
    }
    Ok(())
}

fn show(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Proposal))?;
    if json {
        let out = json!({
            "id": n.id,
            "name": n.name,
            "status": n.status,
            "description": n.description,
            "body": n.body,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("{} [{}]", n.name, n.id);
        println!("  status: {}", n.status);
        if !n.description.is_empty() {
            println!("  description: {}", n.description);
        }
        let items = n.body.get("items").and_then(|v| v.as_array());
        match items {
            Some(arr) if !arr.is_empty() => {
                println!("  items:");
                for it in arr {
                    let num = it.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                    let text = it.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    let kind = it.get("kind").and_then(|v| v.as_str()).unwrap_or("item");
                    let status = it.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    println!("    #{num} [{kind}/{status}] {text}");
                }
            }
            _ => println!("  items: (none)"),
        }
        if let Some(raw) = n.body.get("raw").and_then(|v| v.as_str()) {
            if !raw.is_empty() {
                println!("  raw:");
                for line in raw.lines().take(20) {
                    println!("    {line}");
                }
            }
        }
    }
    Ok(())
}
fn remove(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Proposal))?;
    let item_count = n
        .body
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| items.len())
        .unwrap_or(0);
    store.delete_node(&n.id)?;
    pulse::emit_line(
        &store,
        json,
        json!({
            "removed": true,
            "proposal": node_json(&n),
            "items_removed": item_count,
        }),
        "loom status",
        format!(
            "removed mistaken proposal '{}' ({} item(s))",
            n.name, item_count
        ),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// proposal item add / adopt / defer / reject
// ---------------------------------------------------------------------------

fn item(graph: Option<&Path>, cmd: ProposalItemCmd, json: bool) -> Result<()> {
    match cmd {
        ProposalItemCmd::Add {
            proposal,
            text,
            kind,
        } => item_add(graph, proposal, text, kind, json),
        ProposalItemCmd::Adopt {
            proposal,
            number,
            r#as,
            name,
            description,
        } => item_adopt(graph, proposal, number, r#as, name, description, json),
        ProposalItemCmd::Defer {
            proposal,
            number,
            reason,
        } => item_defer(graph, proposal, number, reason, json),
        ProposalItemCmd::Reject {
            proposal,
            number,
            reason,
        } => item_reject(graph, proposal, number, reason, json),
    }
}

fn item_add(
    graph: Option<&Path>,
    proposal: String,
    text: String,
    kind: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let mut node = store.resolve_node(&proposal, Some(NodeType::Proposal))?;
    let mut body = node.body.clone();
    let items = body
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow!("proposal '{}' body has no items array", node.name))?;
    let number = items.len() + 1;
    let kind_str = kind.as_deref().unwrap_or("item");
    let item_obj = json!({
        "number": number,
        "text": text,
        "kind": kind_str,
        "status": "open",
    });
    items.push(item_obj.clone());
    store.set_node_body(&node.id, &body)?;
    node.body = body;

    if json {
        // Per test contract: item add returns the item object directly.
        println!("{}", serde_json::to_string_pretty(&item_obj)?);
    } else {
        println!("added item #{} to '{}'", number, node.name);
    }
    Ok(())
}

fn item_adopt(
    graph: Option<&Path>,
    proposal: String,
    number: usize,
    r#as: Option<String>,
    name: Option<String>,
    description: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let mut node = store.resolve_node(&proposal, Some(NodeType::Proposal))?;

    // Validate --as value early.
    let as_kind: Option<&str> = match r#as.as_deref() {
        None => None,
        Some("intent") => Some("intent"),
        Some("task") => Some("task"),
        Some(other) => bail!("unsupported --as value '{other}' (use intent or task)"),
    };

    let mut body = node.body.clone();
    let items = body
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow!("proposal '{}' body has no items array", node.name))?;
    let item = items
        .iter_mut()
        .find(|it| it.get("number").and_then(|v| v.as_u64()) == Some(number as u64))
        .ok_or_else(|| anyhow!("proposal '{}' has no item #{}", node.name, number))?;
    let item_status = item.get("status").and_then(Value::as_str).unwrap_or("open");
    if item_status != "open" {
        if let Some(spawned) = item.get("spawned").and_then(Value::as_str) {
            bail!(
                "proposal '{}' item #{} is already {item_status} and spawned {}",
                node.name,
                number,
                spawned
            );
        }
        bail!(
            "proposal '{}' item #{} is already {item_status}",
            node.name,
            number
        );
    }

    let item_text = item
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Mark the item adopted.
    item["status"] = json!("adopted");

    // Optionally spawn a node.
    let spawned: Option<Value> = if let Some(kind) = as_kind {
        let spawn_name = name.as_deref().unwrap_or(&item_text);
        let spawn_desc = description.as_deref().unwrap_or(&item_text);
        let spawned_node = match kind {
            "intent" => {
                if looks_like_symbol(spawn_name) {
                    bail!(
                        "intent name '{spawn_name}' looks like a code symbol. Proposal adoption \
                         can only spawn behavioral planned intents; choose a behavioral --name."
                    );
                }
                let intent_body = json!({
                    "level": "feature",
                    "source_proposal": node.id.clone(),
                    "source_item_number": number,
                });
                let spawned_node = store.add_node(
                    NodeType::Intent,
                    spawn_name,
                    spawn_desc,
                    "planned",
                    intent_body,
                )?;
                store.set_facet(
                    &spawned_node.id,
                    TargetKind::Node,
                    "level",
                    "feature",
                    TruthClass::Asserted,
                )?;
                spawned_node
            }
            "task" => {
                let task_body = json!({
                    "kind": "proposal",
                    "source_proposal": node.id.clone(),
                    "source_item_number": number,
                });
                store.add_node(
                    NodeType::TaskRecord,
                    spawn_name,
                    spawn_desc,
                    "proposed",
                    task_body,
                )?
            }
            _ => unreachable!(),
        };
        // Record the spawned id on the item.
        item["spawned"] = json!(spawned_node.id.clone());
        Some(node_json(&spawned_node))
    } else {
        None
    };

    store.set_node_body(&node.id, &body)?;
    node.body = body;

    // Re-read the updated item for output.
    let updated_item = node
        .body
        .get("items")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|it| it.get("number").and_then(|v| v.as_u64()) == Some(number as u64))
                .cloned()
        })
        .unwrap_or_else(|| json!({}));

    if json {
        let mut out = json!({
            "proposal": {
                "id": node.id,
                "name": node.name,
                "status": node.status,
                "description": node.description,
                "body": node.body,
            },
            "item": updated_item,
        });
        if let Some(s) = &spawned {
            out["spawned"] = s.clone();
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        match &spawned {
            Some(s) => {
                let sid = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
                println!(
                    "adopted item #{} of '{}' → spawned {} [{}]",
                    number,
                    node.name,
                    s["type"].as_str().unwrap_or(""),
                    &sid[..8]
                );
            }
            None => println!("adopted item #{} of '{}'", number, node.name),
        }
    }
    Ok(())
}

fn item_defer(
    graph: Option<&Path>,
    proposal: String,
    number: usize,
    reason: String,
    json: bool,
) -> Result<()> {
    item_dispose(graph, proposal, number, "deferred", reason, json)
}

fn item_reject(
    graph: Option<&Path>,
    proposal: String,
    number: usize,
    reason: String,
    json: bool,
) -> Result<()> {
    item_dispose(graph, proposal, number, "rejected", reason, json)
}

fn item_dispose(
    graph: Option<&Path>,
    proposal: String,
    number: usize,
    status: &str,
    reason: String,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let mut node = store.resolve_node(&proposal, Some(NodeType::Proposal))?;
    let mut body = node.body.clone();
    let items = body
        .get_mut("items")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow!("proposal '{}' body has no items array", node.name))?;
    let item = items
        .iter_mut()
        .find(|it| it.get("number").and_then(|v| v.as_u64()) == Some(number as u64))
        .ok_or_else(|| anyhow!("proposal '{}' has no item #{}", node.name, number))?;
    let item_status = item.get("status").and_then(Value::as_str).unwrap_or("open");
    if item_status != "open" {
        bail!(
            "proposal '{}' item #{} is already {item_status}",
            node.name,
            number
        );
    }
    item["status"] = json!(status);
    item["reason"] = json!(reason);
    let updated_item = item.clone();
    store.set_node_body(&node.id, &body)?;
    node.body = body;

    if json {
        let out = json!({
            "proposal": {
                "id": node.id,
                "name": node.name,
                "status": node.status,
                "description": node.description,
                "body": node.body,
            },
            "item": updated_item,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("item #{} of '{}' → {status}", number, node.name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// First meaningful non-empty line of raw, truncated to ~100 chars. Markdown
/// proposals often start with YAML frontmatter; skip it so `description` carries
/// the proposal title instead of `---`.
fn summary(raw: &str) -> String {
    let mut lines = raw.lines();
    let mut in_frontmatter = false;
    let mut first_seen = false;
    let first = lines
        .find(|line| {
            let trimmed = line.trim();
            if !first_seen && trimmed == "---" {
                first_seen = true;
                in_frontmatter = true;
                return false;
            }
            first_seen = true;
            if in_frontmatter {
                if trimmed == "---" {
                    in_frontmatter = false;
                }
                return false;
            }
            !trimmed.is_empty()
        })
        .unwrap_or("");
    if first.chars().count() > 100 {
        let mut s: String = first.chars().take(97).collect();
        s.push_str("...");
        s
    } else {
        first.to_string()
    }
}
