//! `loom edge` command family.

use super::{open, pulse, verdict_status};
use crate::cli::EdgeCmd;
use crate::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::workitem;
use crate::Result;
use anyhow::anyhow;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: EdgeCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        EdgeCmd::Implement {
            intent,
            codefile,
            locator,
        } => edge_implement(&store, intent, codefile, locator, json),
        EdgeCmd::Call {
            validation,
            surface,
        } => edge_call(&store, validation, surface, json),
        EdgeCmd::Remove { edge_id, reason } => edge_remove(&store, edge_id, reason, json),
        EdgeCmd::SetLocator { edge_id, locator } => {
            edge_set_locator(&store, edge_id, locator, json)
        }
        EdgeCmd::Relate { kind, from, to } => edge_relate(&store, kind, from, to, json),
        EdgeCmd::Verdict {
            edge_id,
            verdict,
            criterion,
            evidence,
            confidence,
        } => edge_verdict(
            &store, edge_id, verdict, criterion, evidence, confidence, json,
        ),
        EdgeCmd::Explore {
            a,
            b,
            verdict,
            criterion,
            evidence,
            confidence,
        } => edge_explore(&store, a, b, verdict, criterion, evidence, confidence, json),
        EdgeCmd::Show { edge_id } => edge_show(&store, edge_id, json),
        EdgeCmd::List { limit } => edge_list(&store, limit, json),
    }
}

fn edge_implement(
    store: &Store,
    intent: String,
    codefile: String,
    locator: Option<String>,
    json: bool,
) -> Result<()> {
    let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
    let cf = store.resolve_node(&codefile, Some(NodeType::CodeFile))?;
    let e = store.add_edge(EdgeKind::Implements, &i.id, &cf.id, TruthClass::Asserted)?;
    if let Some(loc) = &locator {
        store.set_facet(
            &e.id,
            TargetKind::Edge,
            "locator",
            loc,
            TruthClass::Asserted,
        )?;
    }
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "intent": {
                "id": i.id,
                "name": i.name,
            },
            "codefile": {
                "id": cf.id,
                "path": cf.name,
            },
            "locator": locator,
        }),
        "loom sync",
        format!("grounded '{}' in '{}' [{}]", i.name, cf.name, &e.id[..8]),
    )?;
    Ok(())
}

/// Bind a validation to an interface surface it exercises (a `calls` edge).
/// Idempotent: re-calling the same pair returns the existing edge.
fn edge_call(store: &Store, validation: String, surface: String, json: bool) -> Result<()> {
    let v = store.resolve_node(&validation, Some(NodeType::Validation))?;
    let s = store.resolve_node(&surface, Some(NodeType::InterfaceSurface))?;
    let existing = store.edges_with(Some(EdgeKind::Calls), Some(&v.id), Some(&s.id))?;
    let e = match existing.into_iter().next() {
        Some(e) => e,
        None => store.add_edge(EdgeKind::Calls, &v.id, &s.id, TruthClass::Asserted)?,
    };
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "validation": {
                "id": v.id,
                "name": v.name,
            },
            "surface": {
                "id": s.id,
                "name": s.name,
            },
        }),
        "loom status",
        format!("'{}' calls surface '{}' [{}]", v.name, s.name, &e.id[..8]),
    )?;
    Ok(())
}

/// Prune an edge. Refuses derived edges — those are recomputed by `loom sync`,
/// so deleting one is pointless (it returns on the next sync); the fix is to
/// remove its source. Asserted edges (a redundant grounding/relationship) go.
fn edge_remove(store: &Store, edge_id: String, reason: Option<String>, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    if e.truth_class == TruthClass::Derived {
        anyhow::bail!(
            "edge [{}] is a derived {} edge — it is rebuilt by `loom sync`; remove its source, not the edge",
            &e.id[..8],
            e.kind
        );
    }
    let ungrounded_intent = if e.kind == EdgeKind::Implements {
        Some(
            store
                .get_node(&e.from_id)?
                .map(|n| n.name)
                .unwrap_or_else(|| e.from_id.clone()),
        )
    } else {
        None
    };
    // Journal the prune on the source node before the edge id is gone.
    if let Some(r) = &reason {
        store.add_note(
            &e.from_id,
            "decision",
            &format!("removed {} edge: {r}", e.kind),
        )?;
    }
    store.delete_edge(&e.id)?;
    let warning = if let Some(intent_name) = &ungrounded_intent {
        if store
            .edges_with(Some(EdgeKind::Implements), Some(&e.from_id), None)?
            .is_empty()
        {
            Some(format!(
                "warning: intent '{intent_name}' now has zero implements edges; run `loom status` or re-ground it"
            ))
        } else {
            None
        }
    } else {
        None
    };
    pulse::emit(
        store,
        json,
        serde_json::json!({
            "removed": true,
            "edge": e,
            "reason": reason,
            "warning": warning,
        }),
        "loom status",
        || {
            println!(
                "removed {} edge [{}]  ({} → {})",
                e.kind,
                &e.id[..8],
                e.from_id,
                e.to_id
            );
            if let Some(w) = &warning {
                println!("{w}");
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Correct the `locator` (symbol) facet on an asserted edge — e.g. a grounding
/// whose target symbol moved or was misnamed. Upserts; refuses derived edges.
fn edge_set_locator(store: &Store, edge_id: String, locator: String, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    if e.truth_class == TruthClass::Derived {
        anyhow::bail!(
            "edge [{}] is derived — its facets are sync-owned",
            &e.id[..8]
        );
    }
    store.set_facet(
        &e.id,
        TargetKind::Edge,
        "locator",
        &locator,
        TruthClass::Asserted,
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "locator": locator,
        }),
        "loom sync",
        format!(
            "set locator on {} edge [{}] → {locator}",
            e.kind,
            &e.id[..8]
        ),
    )?;
    Ok(())
}

fn edge_relate(store: &Store, kind: String, from: String, to: String, json: bool) -> Result<()> {
    let k = workitem::relationship_kind(&kind)
        .ok_or_else(|| anyhow!("unknown relationship kind '{kind}'"))?;
    let a = store.resolve_node(&from, Some(NodeType::Intent))?;
    let b = store.resolve_node(&to, Some(NodeType::Intent))?;
    let e = store.add_edge(k, &a.id, &b.id, TruthClass::Asserted)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "kind": kind,
            "from": {
                "id": a.id,
                "name": a.name,
            },
            "to": {
                "id": b.id,
                "name": b.name,
            },
        }),
        "loom status",
        format!("{} '{}' → '{}' [{}]", kind, a.name, b.name, &e.id[..8]),
    )?;
    Ok(())
}

fn edge_verdict(
    store: &Store,
    edge_id: String,
    verdict: String,
    criterion: String,
    evidence: String,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let status = verdict_status(&verdict)?;
    let target = store.resolve_edge(&edge_id)?;
    let e = store.record_verdict(&target.id, status, &criterion, &evidence, confidence, "llm")?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "verdict": verdict,
            "criterion": criterion,
            "evidence": evidence,
            "confidence": confidence,
        }),
        "loom status",
        format!("recorded {} on edge [{}]", e.status, &e.id[..8]),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edge_explore(
    store: &Store,
    a: String,
    b: String,
    verdict: String,
    criterion: String,
    evidence: String,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let ia = store.resolve_node(&a, Some(NodeType::Intent))?;
    let ib = store.resolve_node(&b, Some(NodeType::Intent))?;
    let existing = store.edges_with(Some(EdgeKind::Relates), Some(&ia.id), Some(&ib.id))?;
    let edge = match existing.into_iter().next() {
        Some(e) => e,
        None => store.add_edge(EdgeKind::Relates, &ia.id, &ib.id, TruthClass::Asserted)?,
    };
    let status = verdict_status(&verdict)?;
    let verdict_edge =
        store.record_verdict(&edge.id, status, &criterion, &evidence, confidence, "llm")?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": verdict_edge,
            "a": {
                "id": ia.id,
                "name": ia.name,
            },
            "b": {
                "id": ib.id,
                "name": ib.name,
            },
            "verdict": verdict,
            "criterion": criterion,
            "evidence": evidence,
            "confidence": confidence,
        }),
        "loom status",
        format!("explored '{}' ~ '{}': {}", ia.name, ib.name, status),
    )?;
    Ok(())
}

fn edge_show(store: &Store, edge_id: String, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&e)?);
    } else {
        println!("{} [{}]", e.kind, e.id);
        println!("  {} → {}", e.from_id, e.to_id);
        println!("  truth_class: {}  status: {}", e.truth_class, e.status);
        if !e.criterion.is_empty() {
            println!("  criterion: {}", e.criterion);
        }
        if !e.evidence.is_empty() {
            println!("  evidence: {}", e.evidence);
        }
    }
    Ok(())
}

fn edge_list(store: &Store, limit: usize, json: bool) -> Result<()> {
    let edges = store.list_edges(None, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&edges)?);
    } else {
        if edges.is_empty() {
            println!("no edges");
        }
        for e in &edges {
            println!(
                "{:<10} {:<18} {} [{}]",
                e.truth_class,
                e.kind,
                e.status,
                &e.id[..8]
            );
        }
    }
    Ok(())
}
