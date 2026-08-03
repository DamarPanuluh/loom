//! `loom wiki` — reader-first documentation as a tracked projection of the graph.
//!
//! A `WikiPage` is NOT prose loom writes. It is a node recording which intents a
//! human/agent-authored page documents, so the graph can (a) hand the writer a
//! verified brief and (b) mark the page stale precisely when something it covers
//! changes. loom curates the facts and tracks freshness; an agent writes the
//! prose. The page's form is editorial (reader-first: overview, architecture,
//! flows) — the graph governs truth and freshness, never layout, and prose is
//! never a competing source of truth.
//!
//! The loop: `wiki plan <title>` grounds a draft page in the intents it will
//! cover → `wiki next` emits its verified brief → an agent writes the prose →
//! `wiki record <title>` stamps the scope fingerprint and marks it fresh. When a
//! documented intent's meaning, code, or proof drifts, `sync` flips the page back
//! to stale and it re-enters `wiki next`.
//!
//! Plane: CLI surface over the judgment plane — asserted page scope and
//! freshness stamps; staleness itself is derived by `sync`, never asserted here.

use super::{open, pulse};
use crate::cli::WikiCmd;
use crate::model::{EdgeKind, InspectionStatus, NodeType};
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: WikiCmd, json: bool) -> Result<()> {
    match cmd {
        WikiCmd::Plan {
            title,
            path,
            covers,
        } => wiki_plan(graph, &title, &path, &covers, json),
        WikiCmd::Record { title } => wiki_record(graph, &title, json),
        WikiCmd::Next => wiki_next(graph, json),
        WikiCmd::List { limit, offset } => wiki_list(graph, limit, offset, json),
        WikiCmd::Remove { title } => wiki_remove(graph, &title, json),
    }
}

/// Plan (create or re-ground) a draft wiki page: link it to the intents it will
/// document and record its output path. A page must document at least one intent
/// — an ungrounded page has no verifiable subject. Leaves it `draft` for
/// `wiki next` to brief; prose is written before `wiki record` marks it fresh.
fn wiki_plan(
    graph: Option<&Path>,
    title: &str,
    path: &str,
    covers: &[String],
    json: bool,
) -> Result<()> {
    if covers.is_empty() {
        bail!("a wiki page must document at least one intent (--covers <intent>)");
    }
    let store = open(graph)?;
    let mut intent_ids = Vec::new();
    for c in covers {
        intent_ids.push(store.resolve_node(c, Some(NodeType::Intent))?.id);
    }
    let page = match store.resolve_node(title, Some(NodeType::WikiPage)) {
        Ok(p) => p,
        Err(_) => store.add_node(
            NodeType::WikiPage,
            title,
            "",
            "draft",
            json!({ "path": path }),
        )?,
    };
    // Reconcile documents edges to exactly the covered intents.
    let wanted: std::collections::BTreeSet<&str> = intent_ids.iter().map(|s| s.as_str()).collect();
    for iid in &intent_ids {
        store.ensure_edge(EdgeKind::Documents, &page.id, iid)?;
    }
    for e in store.edges_with(Some(EdgeKind::Documents), Some(&page.id), None)? {
        if !wanted.contains(e.to_id.as_str()) {
            store.delete_edge(&e.id)?;
        }
    }
    // Re-grounding invalidates any prior authored content: back to draft.
    let mut body = store
        .get_node(&page.id)?
        .map(|n| n.body)
        .unwrap_or_else(|| json!({}));
    body["path"] = json!(path);
    body.as_object_mut().map(|o| o.remove("scope_hash"));
    store.set_node_body(&page.id, &body)?;
    // loom-stability-exempt: moves a wiki page to draft
    store.set_node_status(&page.id, "draft")?;
    pulse::emit_line(
        &store,
        json,
        json!({ "page": title, "path": path, "documents": intent_ids, "status": "draft" }),
        "loom wiki next",
        format!(
            "planned wiki page '{title}' → {path} (documents {} intent(s)); write it, then `loom wiki record '{title}'`",
            intent_ids.len()
        ),
    )
}

/// Mark an authored page fresh: stamp the scope fingerprint of everything it
/// documents. The prose must already be written at the page's path (loom tracks
/// the page, not its bytes). Errors if the page was never planned.
fn wiki_record(graph: Option<&Path>, title: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let page = match store.resolve_node(title, Some(NodeType::WikiPage)) {
        Ok(p) => p,
        Err(_) => bail!("no wiki page '{title}' — plan it first with `loom wiki plan`"),
    };
    if store
        .edges_with(Some(EdgeKind::Documents), Some(&page.id), None)?
        .is_empty()
    {
        bail!("wiki page '{title}' documents no intent — re-plan it with --covers");
    }
    // record is the freshness gate: the prose must actually be written, or a
    // typo/skipped write would silently mark the wiki fresh against nothing.
    let path = page
        .body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if path.is_empty() {
        bail!("wiki page '{title}' has no output path — re-plan it with --path");
    }
    match std::fs::metadata(store.root().join(path)) {
        Ok(m) if m.len() > 0 => {}
        Ok(_) => bail!(
            "wiki page '{title}' at {path} is empty — write the prose before recording it fresh"
        ),
        Err(_) => bail!(
            "wiki page '{title}' expects prose at {path}, but it is not there — write it first"
        ),
    }
    let hash = crate::sync::wiki_scope_hash(&store, &page.id)?;
    let mut body = page.body.clone();
    body["scope_hash"] = json!(hash);
    store.set_node_body(&page.id, &body)?;
    // loom-stability-exempt: moves a wiki page to fresh
    store.set_node_status(&page.id, "fresh")?;
    pulse::emit_line(
        &store,
        json,
        json!({ "page": title, "path": path, "status": "fresh" }),
        "loom status",
        format!("recorded wiki page '{title}' fresh"),
    )
}

/// Serve the next page needing prose — a `draft` (never authored) or `stale`
/// (its subject drifted) page — with a brief: the verified facts to draw on and
/// the mindset to write it reader-first. loom emits the instruction; an agent
/// writes the prose and runs the write-back.
fn wiki_next(graph: Option<&Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let mut pages: Vec<_> = store
        .list_nodes(Some(NodeType::WikiPage), usize::MAX)?
        .into_iter()
        .filter(|p| p.status != "fresh")
        .collect();
    // draft before stale, then by name — author missing pages before refreshing.
    pages.sort_by(|a, b| {
        page_rank(&a.status)
            .cmp(&page_rank(&b.status))
            .then(a.name.cmp(&b.name))
    });
    let Some(page) = pages.into_iter().next() else {
        return pulse::emit_line(
            &store,
            json,
            json!({ "page": Value::Null }),
            "loom status",
            "no wiki page needs writing — every recorded page is fresh",
        );
    };
    let path = page
        .body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut facts = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Documents), Some(&page.id), None)? {
        let Some(intent) = store.get_node(&e.to_id)? else {
            continue;
        };
        let realized_in: Vec<Value> = store
            .realizing_groundings(&intent.id)?
            .into_iter()
            .filter_map(|g| {
                store
                    .get_node(&g.to_id)
                    .ok()
                    .flatten()
                    .map(|f| json!(f.name))
            })
            .collect();
        let proven = store
            .edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))?
            .iter()
            .any(|v| v.status == InspectionStatus::Passing);
        facts.push(json!({
            "intent": intent.name,
            "description": intent.description,
            "lifecycle": intent.status,
            "realized_in": realized_in,
            "proven": proven,
        }));
    }
    let payload = json!({
        "page": page.name,
        "path": path,
        "status": page.status,
        "mindset": "Write this page reader-first — prose and diagrams a person understands, \
                    organized for comprehension (overview / architecture / flows), NOT as a dump \
                    of the facts below. The facts are your verified source of truth: do not \
                    contradict them, but cite nothing inline. When the prose is written, run the \
                    write_back to mark the page fresh.",
        "facts": facts,
        "write_back": format!("loom wiki record '{}'", page.name),
    });
    let line = format!(
        "{} wiki page '{}' ({} documented intent(s)) → write {path}",
        page.status,
        page.name,
        facts.len()
    );
    pulse::emit_line(
        &store,
        json,
        payload,
        &format!("loom wiki record '{}'", page.name),
        line,
    )
}

fn page_rank(status: &str) -> u8 {
    match status {
        "draft" => 0,
        "stale" => 1,
        _ => 2,
    }
}

fn wiki_list(graph: Option<&Path>, limit: usize, offset: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let pages: Vec<_> = store.list_nodes_page(Some(NodeType::WikiPage), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::WikiPage))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(
                &pages, offset, limit, total
            ))?
        );
    } else {
        for p in &pages {
            let path = p.body.get("path").and_then(|v| v.as_str()).unwrap_or("");
            println!("{:<8} {} [{}]  {path}", p.status, p.name, &p.id[..8]);
        }
        if let Some(footer) = super::page_footer(pages.len(), offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

fn wiki_remove(graph: Option<&Path>, title: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let page = match store.resolve_node(title, Some(NodeType::WikiPage)) {
        Ok(p) => p,
        Err(_) => bail!("no wiki page '{title}'"),
    };
    store.delete_node(&page.id)?;
    pulse::emit_line(
        &store,
        json,
        json!({ "removed": title }),
        "loom status",
        format!("removed wiki page '{title}'"),
    )
}
